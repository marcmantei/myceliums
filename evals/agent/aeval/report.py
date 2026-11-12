"""Export a run's compact results and render the markdown report.

``export`` copies the final row of every run from ``runs/<label>/results.jsonl``
(git-ignored, next to the raw transcripts) to ``results/<label>/results.jsonl``
and writes ``results/<label>/meta.json`` (versions, pins, index sizes, session
counts). ``render`` turns those two committed files into ``REPORT.md`` and
``summary.json``; it is deterministic, so CI can check that a committed report
matches its data.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from pathlib import Path

from . import config
from .grading import grade, is_success
from .stats import mean, median, paired_bootstrap
from .stream import parse_transcript

COMPACT_FIELDS = [
    "run_id", "task_id", "repo", "type", "condition", "rep", "attempt", "status",
    "answer", "f1", "precision", "recall", "success", "tp", "fp", "neutral", "n_gold",
    "n_answer", "missed", "false_positives", "num_turns", "tool_calls",
    "tool_calls_by_name", "mcp_tool_calls", "mcp_tools_used", "tool_errors", "first_tool",
    "n_tools_offered", "n_mcp_tools_offered",
    "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens",
    "total_input_tokens", "first_turn_input_tokens", "output_tokens", "cost_usd", "duration_ms",
    "duration_api_ms",
    "wall_s", "model", "models_used", "claude_version", "finished_at",
]

METRICS = [
    # key, label, format, per-run extractor
    ("success", "Success rate (F1 ≥ {thr})", "pct", lambda r: 1.0 if r["success"] else 0.0),
    ("f1", "Mean F1", "f3", lambda r: r["f1"]),
    ("precision", "Mean precision", "f3", lambda r: r["precision"]),
    ("recall", "Mean recall", "f3", lambda r: r["recall"]),
    ("total_input_tokens", "Input tokens per run (incl. cache)", "int", lambda r: r["total_input_tokens"]),
    ("first_turn_input_tokens", "Prompt size of the first model call (tokens)", "int",
     lambda r: r.get("first_turn_input_tokens") or 0),
    ("output_tokens", "Output tokens per run", "int", lambda r: r["output_tokens"]),
    ("cost_usd", "Cost per run (USD, CLI estimate)", "usd", lambda r: r["cost_usd"] or 0.0),
    ("tool_calls", "Tool calls per run", "f1", lambda r: r["tool_calls"]),
    ("num_turns", "Turns per run", "f1", lambda r: r["num_turns"] or 0),
    ("wall_s", "Wall time per run (s)", "f1", lambda r: r["wall_s"]),
]


def export(label: str, extra_meta: dict) -> Path:
    src = config.RUNS_DIR / label / "results.jsonl"
    rows: dict[str, dict] = {}
    attempts: Counter = Counter()
    for line in src.read_text().splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        attempts[row["run_id"]] += 1
        if row.get("final"):
            rows[row["run_id"]] = row
    # Grades are re-derived from the stored answers with the current gold sets,
    # so an amended label applies to every session of the run alike.
    tasks = {t["id"]: t for t in config.load_tasks()}
    threshold = extra_meta.get("success_threshold", 0.8)
    for row in rows.values():
        g = grade(tasks[row["task_id"]], row["answer"], threshold)
        row.update(f1=round(g.f1, 4), precision=round(g.precision, 4), recall=round(g.recall, 4),
                   success=is_success(g.f1, threshold), tp=g.tp, fp=g.fp, neutral=g.neutral,
                   n_gold=g.n_gold, n_answer=g.n_answer, missed=g.missed,
                   false_positives=g.false_positives)
    # Fields added to the runner after a run started are re-derived from the
    # transcripts, which are the source of truth for every per-session number.
    for run_id, row in rows.items():
        if row.get("first_turn_input_tokens") is None:
            path = config.RUNS_DIR / label / "sessions" / run_id / f"attempt{row['attempt']}.jsonl"
            if path.exists():
                row["first_turn_input_tokens"] = parse_transcript(str(path)).first_turn_input_tokens
    out_dir = config.RESULTS_DIR / label
    out_dir.mkdir(parents=True, exist_ok=True)
    with open(out_dir / "results.jsonl", "w", encoding="utf-8") as fh:
        for run_id in sorted(rows):
            compact = {k: rows[run_id].get(k) for k in COMPACT_FIELDS}
            fh.write(json.dumps(compact, sort_keys=True) + "\n")
    # The report is rendered from this snapshot, not from the live tasks.json,
    # so later edits to the task set cannot silently change a committed report.
    snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                 "n_gold": len(t["gold"]), "question": t["question"]} for t in config.load_tasks()]
    (out_dir / "tasks_snapshot.json").write_text(json.dumps(snapshot, indent=1, ensure_ascii=False) + "\n")
    meta = dict(extra_meta)
    meta["tasks_sha256"] = config.sha256_file(config.TASKS_FILE)
    meta["attempts_total"] = sum(attempts.values())
    meta["runs_final"] = len(rows)
    meta["retried_runs"] = sorted(k for k, v in attempts.items() if v > 1)
    (out_dir / "meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
    return out_dir


def _fmt(value: float, kind: str) -> str:
    if value != value:  # NaN
        return "–"
    if kind == "pct":
        return f"{100 * value:.0f} %"
    if kind == "f3":
        return f"{value:.3f}"
    if kind == "int":
        return f"{value:,.0f}"
    if kind == "usd":
        return f"{value:.3f}"
    return f"{value:.1f}"


def _fmt_diff(value: float, kind: str) -> str:
    if value != value:
        return "–"
    sign = "+" if value >= 0 else "−"
    v = abs(value)
    if kind == "pct":
        return f"{sign}{100 * v:.0f} pp"
    if kind == "f3":
        return f"{sign}{v:.3f}"
    if kind == "int":
        return f"{sign}{v:,.0f}"
    if kind == "usd":
        return f"{sign}{v:.3f}"
    return f"{sign}{v:.1f}"


def _fmt_ratio(value: float) -> str:
    return "–" if value != value else f"{value:.2f}×"


def _per_task(rows: list[dict], extractor) -> dict[str, dict[str, float]]:
    acc: dict[str, dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
    for r in rows:
        acc[r["task_id"]][r["condition"]].append(float(extractor(r)))
    return {tid: {c: mean(v) for c, v in conds.items()} for tid, conds in acc.items()}


def _pairs(per_task: dict, task_ids: list[str]) -> list[tuple[float, float]]:
    return [(per_task[t]["with"], per_task[t]["without"]) for t in task_ids
            if "with" in per_task.get(t, {}) and "without" in per_task.get(t, {})]


def analyse(rows: list[dict], tasks: list[dict], threshold: float, n_resamples: int = 10_000) -> dict:
    task_ids = [t["id"] for t in tasks]
    types = sorted({t["type"] for t in tasks})
    type_of = {t["id"]: t["type"] for t in tasks}
    out: dict = {"overall": {}, "by_type": {}, "per_task": {}}
    for key, _label, _kind, fn in METRICS:
        pt = _per_task(rows, fn)
        out["overall"][key] = paired_bootstrap(_pairs(pt, task_ids), n_resamples).as_dict()
        for typ in types:
            ids = [t for t in task_ids if type_of[t] == typ]
            out["by_type"].setdefault(typ, {})[key] = paired_bootstrap(_pairs(pt, ids), n_resamples).as_dict()
        for tid in task_ids:
            out["per_task"].setdefault(tid, {})[key] = pt.get(tid, {})
    # Relative token / cost / time change with a CI on the ratio of means.
    return out


def render(label: str, n_resamples: int = 10_000) -> tuple[str, dict]:
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    rows = [json.loads(line) for line in (res_dir / "results.jsonl").read_text().splitlines() if line.strip()]
    tasks = config.load_json(res_dir / "tasks_snapshot.json")
    threshold = meta.get("success_threshold", 0.8)
    a = analyse(rows, tasks, threshold, n_resamples)
    with_rows = [r for r in rows if r["condition"] == "with"]
    without_rows = [r for r in rows if r["condition"] == "without"]

    L: list[str] = []
    L.append(f"# Agent benchmark: {label}")
    L.append("")
    L.append("Does a coding agent answer code-understanding questions better, cheaper or faster "
             "when it has the myceliums MCP server? Generated by "
             "`python3 evals/agent/agent_eval.py report " + label + "` from `results.jsonl` "
             "and `meta.json` in this directory; method in [`../../README.md`](../../README.md).")
    L.append("")
    L.append("## Setup")
    L.append("")
    L.append(f"- **Date:** {meta.get('date')}")
    L.append(f"- **Agent:** Claude Code {meta.get('claude_version')}, `--model {meta.get('model_alias')}` "
             f"(resolved: `{meta.get('model')}`), `--effort {meta.get('effort')}`, "
             f"`--max-turns {meta.get('max_turns')}`, timeout {meta.get('timeout_s')} s per session")
    L.append(f"- **myceliums:** `{meta.get('myc_version')}` built from commit `{meta.get('myc_commit')}` "
             f"(binary sha256 `{str(meta.get('myc_sha256'))[:16]}…`)")
    L.append(f"- **Conditions:** *without* = the built-in tools `Read`, `Grep`, `Glob` and `ToolSearch`; "
             f"*with* = the same plus the myceliums MCP server ({meta.get('mcp_tools_offered')} tools, "
             f"deferred behind `ToolSearch` as Claude Code does by default). Same prompt, model and limits; "
             f"user settings, CLAUDE.md, hooks, plugins and skills disabled and checked in every transcript.")
    L.append(f"- **Tasks:** {len(tasks)} over {len(meta.get('repos', {}))} pinned repositories, "
             f"{meta.get('reps')} repetitions per task and condition, "
             f"{len(rows)} graded sessions ({len(with_rows)} with, {len(without_rows)} without).")
    for name, r in sorted(meta.get("repos", {}).items()):
        idx = meta.get("index", {}).get(name, {})
        L.append(f"  - `{name}` {r.get('ref')} (`{r.get('commit', '')[:12]}`, {r.get('language_name')}): "
                 f"index {idx.get('symbols', '?'):,} symbols in {idx.get('files', '?'):,} files"
                 if isinstance(idx.get("symbols"), int) else f"  - `{name}` {r.get('ref')}")
    L.append(f"- **Grading:** set-based precision / recall / F1 of the final JSON answer against "
             f"hand-verified gold sets; success = F1 ≥ {threshold}.")
    L.append(f"- **Statistics:** repetitions are averaged within each task first; 95 % paired "
             f"bootstrap intervals over tasks ({n_resamples:,} resamples, percentile method).")
    L.append("")

    L.append("## Headline")
    L.append("")
    L.append(f"Paired over {a['overall']['f1']['n_tasks']} tasks. Brackets: 95 % CI.")
    L.append("")
    L.append("| Metric | without myceliums | with myceliums | with − without | with ÷ without |")
    L.append("|---|---:|---:|---:|---:|")
    for key, label_, kind, _fn in METRICS:
        s = a["overall"][key]
        ratio = ("–" if kind in ("pct", "f3") else
                 f"{_fmt_ratio(s['ratio_mean'])} [{_fmt_ratio(s['ratio_lo'])}, {_fmt_ratio(s['ratio_hi'])}]")
        L.append(f"| {label_.format(thr=threshold)} | {_fmt(s['b_mean'], kind)} "
                 f"[{_fmt(s['b_lo'], kind)}, {_fmt(s['b_hi'], kind)}] | {_fmt(s['a_mean'], kind)} "
                 f"[{_fmt(s['a_lo'], kind)}, {_fmt(s['a_hi'], kind)}] | **{_fmt_diff(s['diff_mean'], kind)}** "
                 f"[{_fmt_diff(s['diff_lo'], kind)}, {_fmt_diff(s['diff_hi'], kind)}] | {ratio} |")
    L.append("")

    L.append("## By task type")
    L.append("")
    L.append("Few tasks per type, so these intervals are wide; read them as direction, not size.")
    L.append("")
    L.append("| Type | Tasks | F1 without | F1 with | Δ F1 [95 % CI] | Success without | Success with "
             "| Δ input tokens | Δ tool calls |")
    L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for typ, m in sorted(a["by_type"].items()):
        f, s, tok, tc = m["f1"], m["success"], m["total_input_tokens"], m["tool_calls"]
        L.append(f"| {typ} | {f['n_tasks']} | {f['b_mean']:.3f} | {f['a_mean']:.3f} | "
                 f"{_fmt_diff(f['diff_mean'], 'f3')} [{_fmt_diff(f['diff_lo'], 'f3')}, {_fmt_diff(f['diff_hi'], 'f3')}] | "
                 f"{_fmt(s['b_mean'], 'pct')} | {_fmt(s['a_mean'], 'pct')} | "
                 f"{_fmt_diff(tok['diff_mean'], 'int')} | {_fmt_diff(tc['diff_mean'], 'f1')} |")
    L.append("")

    L.append("## Per task")
    L.append("")
    L.append("Mean over repetitions. F1 per repetition in brackets, in run order.")
    L.append("")
    L.append("| Task | Type | Gold | F1 without | F1 with | Tool calls without → with | myceliums tools used (calls) |")
    L.append("|---|---|---:|---:|---:|---:|---|")
    runs_by = defaultdict(list)
    for r in sorted(rows, key=lambda r: (r["task_id"], r["condition"], r["rep"])):
        runs_by[(r["task_id"], r["condition"])].append(r)
    for t in tasks:
        tid = t["id"]
        w, wo = runs_by[(tid, "with")], runs_by[(tid, "without")]
        if not w and not wo:
            continue
        used = Counter()
        for r in w:
            used.update({k[len("mcp__myceliums__"):]: v for k, v in (r["tool_calls_by_name"] or {}).items()
                         if k.startswith("mcp__myceliums__")})
        used_s = ", ".join(f"{k} ({v})" for k, v in used.most_common(4)) or "none"
        f1_wo = " ".join("%.2f" % r["f1"] for r in wo)
        f1_w = " ".join("%.2f" % r["f1"] for r in w)
        L.append(f"| `{tid}` | {t['type']} | {t['n_gold']} | "
                 f"{mean([r['f1'] for r in wo]):.2f} ({f1_wo}) | "
                 f"{mean([r['f1'] for r in w]):.2f} ({f1_w}) | "
                 f"{mean([r['tool_calls'] for r in wo]):.0f} → {mean([r['tool_calls'] for r in w]):.0f} | {used_s} |")
    L.append("")

    L.append("## How the agent used myceliums")
    L.append("")
    n_with = len(with_rows)
    used_any = sum(1 for r in with_rows if r["mcp_tool_calls"])
    calls = Counter()
    errors = Counter()
    for r in with_rows:
        for k, v in (r["tool_calls_by_name"] or {}).items():
            if k.startswith("mcp__myceliums__"):
                calls[k[len("mcp__myceliums__"):]] += v
        for k, v in (r["tool_errors"] or {}).items():
            if k.startswith("mcp__myceliums__"):
                errors[k[len("mcp__myceliums__"):]] += v
    share = [r["mcp_tool_calls"] / r["tool_calls"] for r in with_rows if r["tool_calls"]]
    L.append(f"- Sessions that called at least one myceliums tool: {used_any} of {n_with}.")
    L.append(f"- Share of tool calls that went to myceliums (median over *with* sessions): "
             f"{100 * median(share):.0f} %." if share else "- No tool calls recorded.")
    first = Counter(r["first_tool"] for r in with_rows)
    L.append("- First tool called in *with* sessions: "
             + ", ".join(f"`{k}` {v}" for k, v in first.most_common()) + ".")
    L.append("")
    if calls:
        L.append("| myceliums tool | Calls | Calls that returned an error |")
        L.append("|---|---:|---:|")
        for k, v in calls.most_common():
            L.append(f"| `{k}` | {v} | {errors.get(k, 0)} |")
    else:
        searches = sum((r["tool_calls_by_name"] or {}).get("ToolSearch", 0) for r in with_rows)
        L.append(f"No myceliums tool was called in any session (`ToolSearch`, which loads deferred "
                 f"tool definitions, was called {searches} times).")
    L.append("")

    L.append("## Sessions")
    L.append("")
    st = Counter((r["condition"], r["status"]) for r in rows)
    L.append("| Condition | " + " | ".join(sorted({s for _, s in st})) + " |")
    L.append("|---|" + "---:|" * len({s for _, s in st}))
    for cond in ("without", "with"):
        L.append(f"| {cond} | " + " | ".join(str(st.get((cond, s), 0)) for s in sorted({s for _, s in st})) + " |")
    L.append("")
    L.append(f"- Graded sessions: {len(rows)}. Agent sessions launched for this run, including "
             f"retries of transient API errors: {meta.get('attempts_total')}.")
    if meta.get("retried_runs"):
        L.append(f"- Retried after a transient error: {', '.join('`' + x + '`' for x in meta['retried_runs'])}.")
    L.append(f"- `timeout` and `max_turns` sessions are graded like any other (usually F1 = 0); "
             f"`no_answer` means the final message had no parseable JSON answer (F1 = 0).")
    if meta.get("notes"):
        L.append("")
        L.append("## Notes")
        L.append("")
        for n in meta["notes"]:
            L.append(f"- {n}")
    L.append("")

    summary = {"label": label, "threshold": threshold, "analysis": a,
               "n_rows": len(rows), "statuses": {f"{c}/{s}": n for (c, s), n in sorted(st.items())}}
    return "\n".join(L) + "\n", summary


def write_report(label: str, check: bool = False) -> bool:
    text, summary = render(label)
    res_dir = config.RESULTS_DIR / label
    report_path = res_dir / "REPORT.md"
    summary_text = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if check:
        ok = (report_path.exists() and report_path.read_text() == text
              and (res_dir / "summary.json").exists()
              and (res_dir / "summary.json").read_text() == summary_text)
        return ok
    report_path.write_text(text)
    (res_dir / "summary.json").write_text(summary_text)
    return True
