"""Report for the outcome eval at scale (#163): two conditions (``none``, ``C``)
on the same tasks at several turn budgets.

Each budget is its own run (``--max-turns`` is part of a run's config);
``export-scale`` merges the runs into one results directory whose rows carry
their ``budget``. The report compares ``C`` with ``none`` per budget and over
all budgets: means over tasks with 95 % paired bootstrap intervals, and for
success an exact sign (McNemar) test over the discordant tasks. Like the other
reports it is a deterministic function of the committed ``results.jsonl``,
``meta.json`` and ``tasks_snapshot.json``.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from pathlib import Path

from . import config
from .grading import grade, is_success
from .hints import hint_stats
from .report import COMPACT_FIELDS, _fmt, _fmt_diff, _fmt_ratio
from .stats import exact_sign_test, mean, paired_bootstrap
from .stream import parse_transcript

PREFIX = "mcp__myceliums__"
BASE, ARM = "none", "C"


def myc_calls(r: dict) -> int:
    """myceliums calls of a session: MCP tool calls plus ``myc`` CLI runs in Bash."""
    return (r.get("mcp_tool_calls") or 0) + (r.get("myc_cli_calls") or 0)


METRICS = [
    # key, label, format, extractor, ratio shown
    ("success", "Success (F1 ≥ {thr})", "pct", lambda r: 1.0 if r["success"] else 0.0, False),
    ("f1", "Mean F1", "f3", lambda r: r["f1"], False),
    ("myc_used", "Sessions with ≥ 1 myceliums call (MCP or CLI)", "pct", lambda r: 1.0 if myc_calls(r) else 0.0,
     False),
    ("total_input_tokens", "Input tokens (incl. cache)", "int", lambda r: r["total_input_tokens"], True),
    ("output_tokens", "Output tokens", "int", lambda r: r["output_tokens"], True),
    ("cost_usd", "Cost, USD (CLI estimate)", "usd", lambda r: r["cost_usd"] or 0.0, True),
    ("wall_s", "Wall time, s", "f1", lambda r: r["wall_s"], True),
    ("tool_calls", "Tool calls", "f1", lambda r: r["tool_calls"], True),
]
HEADLINE = ("success", "f1", "myc_used", "total_input_tokens", "output_tokens", "cost_usd", "wall_s")

DESCRIPTIONS = {
    "none": "no myceliums at all: no MCP server, no routing block, no hooks, no `myc` on PATH; the "
            "built-in `Read`, `Grep`, `Glob` and `Bash`",
    "C": "the same plus the myceliums MCP server (default core toolset, 9 tools loaded up front) and what "
         "`myc setup-claude --scope project --no-agent` writes: the routing block (in CLAUDE.md; since #200 in .claude/rules), the SessionStart "
         "(context line), PostToolUse and PreToolUse grep-redirect hooks in .claude/settings.json; `myc` (bound "
         "to the session's index copy) is first on PATH, so Bash can run the CLI too",
}


# --------------------------------------------------------------------------
# Export: merge the per-budget runs
# --------------------------------------------------------------------------


def export(label: str, runs: list[str], extra_meta: dict) -> Path:
    tasks = {t["id"]: t for t in config.load_tasks()}
    thr = extra_meta.get("success_threshold", 0.8)
    rows_out, per_run, budgets = [], {}, []
    ledger = config.RUNS_DIR / "ledger.jsonl"
    ledger_rows = ([json.loads(x) for x in ledger.read_text().splitlines() if x.strip()]
                   if ledger.exists() else [])
    for run in runs:
        run_dir = config.RUNS_DIR / run
        cfg = config.load_json(run_dir / "config.json")
        budget = int(cfg["max_turns"])
        budgets.append(budget)
        final, attempts = {}, Counter()
        for line in (run_dir / "results.jsonl").read_text().splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            attempts[row["run_id"]] += 1
            if row.get("final"):
                final[row["run_id"]] = row
        for run_id, row in sorted(final.items()):
            g = grade(tasks[row["task_id"]], row["answer"], thr)
            row.update(f1=round(g.f1, 4), precision=round(g.precision, 4), recall=round(g.recall, 4),
                       success=is_success(g.f1, thr), tp=g.tp, fp=g.fp, neutral=g.neutral, n_gold=g.n_gold,
                       n_answer=g.n_answer, missed=g.missed, false_positives=g.false_positives)
            path = run_dir / "sessions" / run_id / f"attempt{row['attempt']}.jsonl"
            if path.exists():
                t = parse_transcript(str(path))
                if row.get("first_turn_input_tokens") is None:
                    row["first_turn_input_tokens"] = t.first_turn_input_tokens
                if config.VARIANTS[row["condition"]].mcp:
                    row.update(hint_stats(t))
            compact = {k: row.get(k) for k in COMPACT_FIELDS}
            compact["run_id"] = f"t{budget}/{run_id}"
            compact["budget"] = budget
            rows_out.append(compact)
        per_run[str(budget)] = {
            "label": run, "max_turns": budget, "timeout_s": cfg["timeout_s"],
            "prompt_budget": bool(cfg.get("prompt_budget")), "bash": bool(cfg.get("bash")),
            "sessions_launched_ledger": sum(1 for x in ledger_rows if x["label"] == run and not x.get("probe")),
            "attempts_total": sum(attempts.values()), "runs_final": len(final),
            "retried_runs": sorted(k for k, v in attempts.items() if v > 1),
        }
    out_dir = config.RESULTS_DIR / label
    out_dir.mkdir(parents=True, exist_ok=True)
    with open(out_dir / "results.jsonl", "w", encoding="utf-8") as fh:
        for r in sorted(rows_out, key=lambda r: (r["budget"], r["task_id"], r["condition"], r["rep"])):
            fh.write(json.dumps(r, sort_keys=True) + "\n")
    snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                 "n_gold": len(t["gold"]), "question": t["question"],
                 **({"calibration": t["calibration"]} if t.get("calibration") else {})}
                for t in config.load_tasks()]
    (out_dir / "tasks_snapshot.json").write_text(json.dumps(snapshot, indent=1, ensure_ascii=False) + "\n")
    meta = dict(extra_meta)
    meta["budgets"] = sorted(budgets)
    meta["runs"] = per_run
    meta["tasks_sha256"] = config.sha256_file(config.tasks_file())
    meta["runs_final"] = len(rows_out)
    (out_dir / "meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
    return out_dir


# --------------------------------------------------------------------------
# Analysis
# --------------------------------------------------------------------------


def _per_task(rows, fn):
    acc = defaultdict(lambda: defaultdict(list))
    for r in rows:
        acc[r["task_id"]][r["condition"]].append(float(fn(r)))
    return {t: {c: mean(v) for c, v in cs.items()} for t, cs in acc.items()}


def compare(rows, ids, n_resamples=10_000) -> dict:
    """C vs none over ``ids`` for every metric, and the sign test for success."""
    out = {}
    for key, _l, _k, fn, _r in METRICS:
        pt = _per_task(rows, fn)
        pairs = [(pt[t][ARM], pt[t][BASE]) for t in ids if ARM in pt.get(t, {}) and BASE in pt.get(t, {})]
        out[key] = paired_bootstrap(pairs, n_resamples).as_dict()
    pt = _per_task(rows, lambda r: 1.0 if r["success"] else 0.0)
    diffs = [pt[t][ARM] - pt[t][BASE] for t in ids if ARM in pt.get(t, {}) and BASE in pt.get(t, {})]
    wins, losses = sum(1 for d in diffs if d > 0), sum(1 for d in diffs if d < 0)
    out["sign_test"] = {"c_better": wins, "none_better": losses, "ties": len(diffs) - wins - losses,
                        "p": exact_sign_test(wins, losses)}
    return out


def analyse(rows, tasks, budgets, n_resamples=10_000) -> dict:
    ids = [t["id"] for t in tasks]
    by_budget = {b: [r for r in rows if r["budget"] == b] for b in budgets}
    out = {"by_budget": {}, "overall": None, "by_repo": {}, "by_type": {}}
    for b in budgets:
        out["by_budget"][str(b)] = compare(by_budget[b], ids, n_resamples)
    # Over all budgets: each task's mean over its budgets, so a task stays one unit.
    out["overall"] = compare(rows, ids, n_resamples)
    for dim, key in (("by_repo", "repo"), ("by_type", "type")):
        groups = sorted({t[key] for t in tasks})
        for gname in groups:
            gids = [t["id"] for t in tasks if t[key] == gname]
            out[dim][gname] = {str(b): compare(by_budget[b], gids, n_resamples) for b in budgets}
            out[dim][gname]["all"] = compare(rows, gids, n_resamples)
    return out


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------


def _cell(s: dict, kind: str, side: str) -> str:
    p = "b" if side == BASE else "a"
    return f"{_fmt(s[p + '_mean'], kind)} [{_fmt(s[p + '_lo'], kind)}, {_fmt(s[p + '_hi'], kind)}]"


def _diff(s: dict, kind: str, ratio: bool) -> str:
    if ratio and s["ratio_mean"] == s["ratio_mean"]:
        return f"{_fmt_ratio(s['ratio_mean'])} [{_fmt_ratio(s['ratio_lo'])}, {_fmt_ratio(s['ratio_hi'])}]"
    return f"{_fmt_diff(s['diff_mean'], kind)} [{_fmt_diff(s['diff_lo'], kind)}, {_fmt_diff(s['diff_hi'], kind)}]"


def _sign(st: dict) -> str:
    return (f"{st['c_better']} : {st['none_better']} (ties {st['ties']}), "
            f"p = {st['p']:.3g}" if st["c_better"] + st["none_better"] else f"no discordant tasks (ties {st['ties']})")


def _budget_name(b: int, meta: dict) -> str:
    run = meta.get("runs", {}).get(str(b), {})
    return f"{b} turns" + ("" if run.get("prompt_budget", True) else " (cap; no budget stated)")


def render(label: str, n_resamples: int = 10_000) -> tuple[str, dict]:
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    rows = [json.loads(x) for x in (res_dir / "results.jsonl").read_text().splitlines() if x.strip()]
    tasks = config.load_json(res_dir / "tasks_snapshot.json")
    ran = {r["task_id"] for r in rows}
    tasks = [t for t in tasks if t["id"] in ran]
    budgets = meta["budgets"]
    thr = meta.get("success_threshold", 0.8)
    # The primary analysis leaves out tasks that do not compare the conditions
    # fairly (meta["primary_exclude"]: task id -> reason); every table also
    # gives the numbers over all tasks.
    excluded = {k: v for k, v in (meta.get("primary_exclude") or {}).items() if k in ran}
    primary = [t for t in tasks if t["id"] not in excluded]
    a = analyse([r for r in rows if r["task_id"] not in excluded], primary, budgets, n_resamples)
    a_all = analyse(rows, tasks, budgets, n_resamples) if excluded else a
    bnames = {b: _budget_name(b, meta) for b in budgets}
    np_, na = len(primary), len(tasks)
    L: list[str] = []
    L.append(f"# Agent benchmark: {label}")
    L.append("")
    L.append("Does myceliums change the outcome of a coding agent's work, and how does the effect depend on the turn "
             f"budget? Generated by `python3 evals/agent/agent_eval.py report {label}` from `results.jsonl`, "
             "`meta.json` and `tasks_snapshot.json` in this directory; method in [`../../README.md`](../../README.md).")
    L.append("")
    L.append("## Setup")
    L.append("")
    L.append(f"- **Date:** {meta.get('date')}")
    L.append(f"- **Agent:** Claude Code {meta.get('claude_version')}, `--model {meta.get('model_alias')}` (resolved: "
             f"`{meta.get('model')}`), `--effort {meta.get('effort')}`")
    runs = meta.get("runs", {})
    parts = []
    for b in budgets:
        r = runs.get(str(b), {})
        parts.append(f"`--max-turns {b}` ({'stated in the prompt' if r.get('prompt_budget') else 'a safety cap, not stated in the prompt'}"
                     f", timeout {r.get('timeout_s')} s)")
    L.append("- **Budgets:** " + "; ".join(parts) + ". A turn is one reply of the agent; a reply with tool calls "
             "(parallel calls included) is one turn, so N turns allow N − 1 rounds of tool calls before the answer.")
    L.append(f"- **myceliums:** `{meta.get('myc_version')}` built from commit `{meta.get('myc_commit')}` (binary sha256 "
             f"`{str(meta.get('myc_sha256'))[:16]}…`)"
             + (f"; {meta['mcp_tools_offered']} MCP tools offered" if meta.get("mcp_tools_offered") else ""))
    L.append(f"- **Tasks:** {na} from `{meta.get('tasks_file')}` (sha256 `{str(meta.get('tasks_sha256'))[:16]}…`) "
             f"over {len({t['repo'] for t in tasks})} pinned repositories, {meta.get('reps')} repetition per task, "
             f"condition and budget; {len(rows)} graded sessions.")
    for name in sorted({t["repo"] for t in tasks}):
        r = meta.get("repos", {}).get(name, {})
        idx = meta.get("index", {}).get(name, {})
        n = sum(1 for t in tasks if t["repo"] == name)
        L.append(f"  - `{name}` {r.get('ref')} (`{r.get('commit', '')[:12]}`, {r.get('language_name')}): {n} tasks; "
                 + (f"index {idx['symbols']:,} symbols in {idx.get('files', 0):,} files, "
                    f"{idx.get('embedded', 0):,} embedded" if isinstance(idx.get("symbols"), int) else "no index")
                 + (f" (not indexed: {', '.join('`' + x + '`' for x in idx['index_exclude_extra'])})"
                    if idx.get("index_exclude_extra") else ""))
    if excluded:
        L.append(f"- **Primary analysis:** the {np_} tasks without " + ", ".join(f"`{k}`" for k in sorted(excluded))
                 + ". " + " ".join(sorted(set(excluded.values()))) + f" Every table gives the {na}-task numbers "
                 "alongside.")
    L.append("- **Same in every condition:** prompt, model, effort, turn limit, timeout; built-in tools `Read`, `Grep`, "
             "`Glob`, `Bash` (+ `ToolSearch`); `--setting-sources project`; a fresh working copy (and in C a fresh "
             "index copy) per session; every transcript checked for the expected tools and MCP server.")
    L.append(f"- **Grading:** set-based precision / recall / F1 against gold sets derived with Python's `ast` module "
             f"and the TypeScript checker, never from myceliums; success = F1 ≥ {thr}.")
    L.append(f"- **Statistics:** tasks are the unit. Means over tasks with 95 % percentile bootstrap intervals "
             f"({n_resamples:,} resamples over tasks, paired). Per budget, success is compared with an exact two-sided "
             "McNemar test (a sign test over the tasks that succeeded in one condition only). *All budgets* averages "
             "each task over its budgets first, so a task still counts once, and compares success with an exact "
             "two-sided sign test on those per-task differences. **No interval or p-value is corrected for multiple "
             "comparisons** (three budgets, the pooled comparison, several metrics, repositories and task types); "
             "the repository and task-type tables are exploratory.")
    L.append("")
    L.append("| Condition | What the session has |")
    L.append("|---|---|")
    for c in (BASE, ARM):
        L.append(f"| **{c}** | {DESCRIPTIONS[c]} |")
    L.append("")

    # ---- headline per budget
    L.append("## Per budget")
    L.append("")
    L.append(f"Means over the {np_} tasks of the primary analysis; brackets: 95 % CI (uncorrected). Differences are "
             "C − none (percentage points or absolute), ratios C ÷ none."
             + (f" Below each table the same comparison over all {na} tasks." if excluded else ""))
    L.append("")
    for b in [*budgets, "all"]:
        s = a["overall"] if b == "all" else a["by_budget"][str(b)]
        s_all = a_all["overall"] if b == "all" else a_all["by_budget"][str(b)]
        L.append(f"### {'All budgets' if b == 'all' else bnames[b]}")
        L.append("")
        L.append("| Metric | none | C | C − none or C ÷ none |")
        L.append("|---|---:|---:|---:|")
        for key, lab, kind, _fn, ratio in METRICS:
            if key not in HEADLINE:
                continue
            L.append(f"| {lab.format(thr=thr)} | {_cell(s[key], kind, BASE)} | {_cell(s[key], kind, ARM)} | "
                     f"{_diff(s[key], kind, ratio)} |")
        test = "sign test" if b == "all" else "McNemar"
        L.append(f"| Success, {test} (tasks C better : none better) | | | {_sign(s['sign_test'])} |")
        L.append("")
        if excluded:
            L.append(f"All {na} tasks: success {_fmt(s_all['success']['b_mean'], 'pct')} → "
                     f"{_fmt(s_all['success']['a_mean'], 'pct')} ({_diff(s_all['success'], 'pct', False)}; "
                     f"{test} {_sign(s_all['sign_test'])}), mean F1 {s_all['f1']['b_mean']:.3f} → "
                     f"{s_all['f1']['a_mean']:.3f} ({_diff(s_all['f1'], 'f3', False)}), cost "
                     f"{_diff(s_all['cost_usd'], 'usd', True)}, wall time {_diff(s_all['wall_s'], 'f1', True)}.")
            L.append("")

    # ---- budget curve
    L.append("## Budget curve")
    L.append("")
    L.append("| Tasks | Budget | Success none | Success C | Δ success [95 % CI] | Mean F1 none | Mean F1 C | "
             "Δ F1 [95 % CI] | McNemar |")
    L.append("|---|---|---:|---:|---:|---:|---:|---:|---|")
    for tag, an in ((f"{np_} (primary)", a), *([(f"all {na}", a_all)] if excluded else [])):
        for b in budgets:
            s = an["by_budget"][str(b)]
            L.append(f"| {tag} | {bnames[b]} | {_fmt(s['success']['b_mean'], 'pct')} | "
                     f"{_fmt(s['success']['a_mean'], 'pct')} | {_diff(s['success'], 'pct', False)} | "
                     f"{s['f1']['b_mean']:.3f} | {s['f1']['a_mean']:.3f} | {_diff(s['f1'], 'f3', False)} | "
                     f"{_sign(s['sign_test'])} |")
    L.append("")

    # ---- per repo and per type
    for dim, title in (("by_repo", "By repository"), ("by_type", "By task type")):
        L.append(f"## {title} (exploratory)")
        L.append("")
        L.append(f"Primary analysis ({np_} tasks). Success none → C per budget, then the difference over all budgets "
                 "with its 95 % CI and sign test, uncorrected; few tasks per group, so read these as direction, not "
                 "size.")
        L.append("")
        L.append("| " + ("Repository" if dim == "by_repo" else "Type") + " | Tasks | "
                 + " | ".join(f"{b} turns" for b in budgets) + " | All budgets: Δ success | Δ F1 | Sign test |")
        L.append("|---|---:|" + "---:|" * len(budgets) + "---:|---:|---|")
        for gname, m in sorted(a[dim].items()):
            n = m["all"]["success"]["n_tasks"]
            cells = [f"{_fmt(m[str(b)]['success']['b_mean'], 'pct')} → {_fmt(m[str(b)]['success']['a_mean'], 'pct')}"
                     for b in budgets]
            L.append(f"| {gname} | {n} | " + " | ".join(cells) + f" | {_diff(m['all']['success'], 'pct', False)} | "
                     f"{_diff(m['all']['f1'], 'f3', False)} | {_sign(m['all']['sign_test'])} |")
        L.append("")

    # ---- per task
    L.append("## Per task")
    L.append("")
    L.append("F1 none / C at each budget; myceliums calls (MCP + CLI) in C summed over the budgets."
             + (" Tasks marked † are not in the primary analysis." if excluded else ""))
    L.append("")
    L.append("| Task | Type | Gold | " + " | ".join(f"{b} turns" for b in budgets) + " | myc calls C | Calibration |")
    L.append("|---|---|---:|" + "---|" * len(budgets) + "---:|---|")
    idx = {(r["task_id"], r["condition"], r["budget"]): r for r in rows}
    for t in tasks:
        cells = []
        for b in budgets:
            n, c = idx.get((t["id"], BASE, b)), idx.get((t["id"], ARM, b))
            cells.append(f"{n['f1']:.2f} / {c['f1']:.2f}" if n and c else "–")
        calls = sum(myc_calls(r) for r in rows if r["task_id"] == t["id"] and r["condition"] == ARM)
        mark = " †" if t["id"] in excluded else ""
        L.append(f"| `{t['id']}`{mark} | {t['type']} | {t['n_gold']} | " + " | ".join(cells) + f" | {calls} | "
                 f"{t.get('calibration', '–')} |")
    L.append("")

    # ---- myceliums use
    L.append("## How the agent used myceliums")
    L.append("")
    L.append("| Budget | C sessions | ≥ 1 myc call | via MCP | via CLI (Bash) | MCP calls | CLI runs | First tool in C "
             "(most common) | none sessions running `myc` |")
    L.append("|---|---:|---:|---:|---:|---:|---:|---|---:|")
    for b in budgets:
        cs = [r for r in rows if r["budget"] == b and r["condition"] == ARM]
        ns = [r for r in rows if r["budget"] == b and r["condition"] == BASE]
        first = Counter(r["first_tool"] for r in cs).most_common(2)
        L.append(f"| {bnames[b]} | {len(cs)} | {sum(1 for r in cs if myc_calls(r))} | "
                 f"{sum(1 for r in cs if r.get('mcp_tool_calls'))} | {sum(1 for r in cs if r.get('myc_cli_calls'))} | "
                 f"{sum(r.get('mcp_tool_calls') or 0 for r in cs)} | {sum(r.get('myc_cli_calls') or 0 for r in cs)} | "
                 + ", ".join(f"`{k}` {v}" for k, v in first) + f" | {sum(1 for r in ns if r.get('myc_cli_calls'))} |")
    L.append("")
    tools = Counter()
    errors = Counter()
    cli = Counter()
    for r in rows:
        if r["condition"] != ARM:
            continue
        for k, v in (r.get("tool_calls_by_name") or {}).items():
            if k.startswith(PREFIX):
                tools[k[len(PREFIX):]] += v
        for k, v in (r.get("tool_errors") or {}).items():
            if k.startswith(PREFIX):
                errors[k[len(PREFIX):]] += v
        cli.update(r.get("myc_cli_subcommands") or [])
    if tools:
        L.append("MCP tool calls in C over all budgets (errors in brackets): " + ", ".join(
            f"`{k}` {v}" + (f" ({errors[k]})" if errors.get(k) else "") for k, v in sorted(tools.items())) + ".")
        L.append("")
    if cli:
        L.append("`myc` CLI subcommands run through Bash in C (sessions using each): " + ", ".join(
            f"`{k}` {v}" for k, v in sorted(cli.items())) + ".")
        L.append("")
    builtin = sorted({k for r in rows for k in (r.get("tool_calls_by_name") or {}) if not k.startswith(PREFIX)})
    for b in budgets:
        per = []
        for c in (BASE, ARM):
            rs = [r for r in rows if r["budget"] == b and r["condition"] == c]
            per.append(f"**{c}** " + ", ".join(
                f"`{k}` {mean([(r.get('tool_calls_by_name') or {}).get(k, 0) for r in rs]):.1f}" for k in builtin))
        L.append(f"- Built-in tool calls per session at {bnames[b]} (mean): " + "; ".join(per) + ".")
    hook_c = [r for r in rows if r["condition"] == ARM]
    if hook_c:
        calls = sum(r.get("hook_grep_calls") or 0 for r in hook_c)
        hints = sum(r.get("hook_grep_hints") or 0 for r in hook_c)
        ctx = [r.get("session_hook") or "" for r in hook_c if "additionalContext" in (r.get("session_hook") or "")]
        L.append(f"- Hooks in C (all budgets): the grep hook ran on {calls} Grep or Bash `grep`/`rg` calls and added a "
                 f"hint to {hints}; the PostToolUse formatter ran {sum(r.get('format_hook_calls') or 0 for r in hook_c)} "
                 f"times; the SessionStart hook put its context line into {len(ctx)} of {len(hook_c)} sessions, naming "
                 f"the index current in {sum(1 for x in ctx if '= HEAD' in x)}.")
    L.append("")

    # ---- sessions
    L.append("## Sessions")
    L.append("")
    st = Counter((r["budget"], r["condition"], r["status"]) for r in rows)
    stats = sorted({s for _, _, s in st})
    L.append("| Budget | Condition | " + " | ".join(stats) + " |")
    L.append("|---|---|" + "---:|" * len(stats))
    for b in budgets:
        for c in (BASE, ARM):
            L.append(f"| {b} | {c} | " + " | ".join(str(st.get((b, c, s), 0)) for s in stats) + " |")
    L.append("")
    for b in budgets:
        r = runs.get(str(b), {})
        L.append(f"- `{r.get('label')}` ({bnames[b]}): {r.get('runs_final')} graded, {r.get('sessions_launched_ledger')} "
                 f"agent sessions launched (ledger, retries included)"
                 + (f"; retried: {', '.join('`' + x + '`' for x in r['retried_runs'])}" if r.get("retried_runs") else "")
                 + ".")
    if meta.get("notes"):
        L.append("")
        L.append("## Notes")
        L.append("")
        for n in meta["notes"]:
            L.append(f"- {n}")
    L.append("")
    summary = {"label": label, "threshold": thr, "budgets": budgets, "analysis": a,
               **({"primary_exclude": excluded, "analysis_all_tasks": a_all} if excluded else {}), "n_rows": len(rows),
               "statuses": {f"{b}/{c}/{s}": n for (b, c, s), n in sorted(st.items())}}
    return "\n".join(L) + "\n", summary
