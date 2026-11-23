"""Report for runs with conditions other than the pilot's with/without (#125).

The variant matrix compares integration layers of myceliums (``A``: MCP server
only; ``C``: what ``myc setup-claude --scope project --no-agent`` installs;
``E``: the same plus the code-explorer subagent) on the same tasks. Every
variant is compared with ``A`` by a paired bootstrap over tasks. A run with a
single condition (the calibration run, ``none``) gets the per-task table and
the means only.

Like :mod:`aeval.report`, the output is a deterministic function of the
committed ``results.jsonl``, ``meta.json`` and ``tasks_snapshot.json``.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict

from . import config
from .report import _fmt, _fmt_diff, _fmt_ratio
from .stats import mean, median, paired_bootstrap

PREFIX = "mcp__myceliums__"

DESCRIPTIONS = {
    "none": "no MCP server (calibration baseline); project settings and CLAUDE.md files load, none exist",
    "A": "the myceliums MCP server only (default core toolset, 9 tools loaded up front)",
    "C": "A + what `myc setup-claude --scope project --no-agent` writes: the routing block in CLAUDE.md, "
         "the SessionStart, PostToolUse and PreToolUse grep-redirect hooks in .claude/settings.json",
    "D": "myceliums only (#178): the MCP server and `Read`; `Grep` and `Glob` denied, search commands in Bash "
         "(grep, rg, find, ag, git grep, ...) blocked by a PreToolUse hook; edit tools on edit tasks",
    "E": "C + the `code-explorer` subagent (`myc setup-claude --scope project`) and the Agent tool, "
         "with Claude Code's built-in subagents denied",
}

# A `none` vs myceliums run (#153): `none` is the comparison arm, not a
# calibration run, and the report has no condition A to refer to.
OUTCOME = {
    "none": "no myceliums at all: no MCP server, no CLAUDE.md routing block, no hooks; the built-in read and "
            "search tools only",
    "C": "the myceliums MCP server (default core toolset, 9 tools loaded up front) + what `myc setup-claude "
         "--scope project --no-agent` writes: the routing block in CLAUDE.md, the SessionStart (context line), "
         "PostToolUse and PreToolUse grep-redirect hooks in .claude/settings.json",
}

METRICS = [
    # key, label, format, per-run extractor
    ("myc_used", "Sessions with ≥ 1 myceliums call", "pct", lambda r: 1.0 if r["mcp_tool_calls"] else 0.0),
    ("myc_calls", "myceliums calls per session", "f1", lambda r: r["mcp_tool_calls"]),
    ("success", "Success rate (F1 ≥ {thr})", "pct", lambda r: 1.0 if r["success"] else 0.0),
    ("f1", "Mean F1", "f3", lambda r: r["f1"]),
    ("precision", "Mean precision", "f3", lambda r: r["precision"]),
    ("recall", "Mean recall", "f3", lambda r: r["recall"]),
    ("total_input_tokens", "Input tokens per session (incl. cache)", "int", lambda r: r["total_input_tokens"]),
    ("first_turn_input_tokens", "Prompt size of the first model call (tokens)", "int",
     lambda r: r.get("first_turn_input_tokens") or 0),
    ("output_tokens", "Output tokens per session", "int", lambda r: r["output_tokens"]),
    ("cost_usd", "Cost per session (USD, CLI estimate)", "usd", lambda r: r["cost_usd"] or 0.0),
    ("tool_calls", "Tool calls per session (all agents)", "f1", lambda r: r["tool_calls"]),
    ("num_turns", "Turns per session (main agent)", "f1", lambda r: r["num_turns"] or 0),
    ("wall_s", "Wall time per session (s)", "f1", lambda r: r["wall_s"]),
]
RATIO_KINDS = ("int", "usd", "f1")


def _per_task(rows, fn):
    acc = defaultdict(lambda: defaultdict(list))
    for r in rows:
        acc[r["task_id"]][r["condition"]].append(float(fn(r)))
    return {tid: {c: mean(v) for c, v in conds.items()} for tid, conds in acc.items()}


def _pairs(pt, ids, a, b):
    return [(pt[t][a], pt[t][b]) for t in ids if a in pt.get(t, {}) and b in pt.get(t, {})]


def analyse(rows, tasks, conditions, baseline, n_resamples=10_000):
    ids = [t["id"] for t in tasks]
    type_of = {t["id"]: t["type"] for t in tasks}
    types = sorted({t["type"] for t in tasks})
    out = {"means": {}, "vs_baseline": {}, "by_type": {}}
    for key, _l, _k, fn in METRICS:
        pt = _per_task(rows, fn)
        for c in conditions:
            ci = paired_bootstrap(_pairs(pt, ids, c, c), n_resamples)
            out["means"].setdefault(c, {})[key] = {"n_tasks": ci.n_tasks, "mean": ci.a_mean,
                                                  "lo": ci.a_lo, "hi": ci.a_hi}
            if c != baseline:
                out["vs_baseline"].setdefault(c, {})[key] = paired_bootstrap(
                    _pairs(pt, ids, c, baseline), n_resamples).as_dict()
            for typ in types:
                tids = [t for t in ids if type_of[t] == typ]
                vals = [pt[t][c] for t in tids if c in pt.get(t, {})]
                out["by_type"].setdefault(typ, {}).setdefault(c, {})[key] = mean(vals)
                if c != baseline and key in ("f1", "success", "total_input_tokens"):
                    out["by_type"][typ].setdefault(f"{c}-{baseline}", {})[key] = paired_bootstrap(
                        _pairs(pt, tids, c, baseline), n_resamples).as_dict()
    return out


def render(label: str, n_resamples: int = 10_000) -> tuple[str, dict]:
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    rows = [json.loads(x) for x in (res_dir / "results.jsonl").read_text().splitlines() if x.strip()]
    tasks = config.load_json(res_dir / "tasks_snapshot.json")
    ran = {r["task_id"] for r in rows}
    tasks = [t for t in tasks if t["id"] in ran]
    thr = meta.get("success_threshold", 0.8)
    conditions = [c for c in meta["conditions"] if any(r["condition"] == c for r in rows)]
    baseline = "A" if "A" in conditions else conditions[0]
    a = analyse(rows, tasks, conditions, baseline, n_resamples)
    by_cond = {c: [r for r in rows if r["condition"] == c] for c in conditions}

    L: list[str] = []
    outcome = baseline == "none" and len(conditions) > 1  # #153: myceliums against no myceliums
    L.append(f"# Agent benchmark: {label}")
    L.append("")
    L.append(("Does myceliums change the outcome of a coding agent's work, not just its tool use? " if outcome else
              "Which integration layer makes a coding agent use myceliums, and does it help? ") + "Generated by "
             f"`python3 evals/agent/agent_eval.py report {label}` from `results.jsonl`, `meta.json` and "
             "`tasks_snapshot.json` in this directory; method in [`../../README.md`](../../README.md).")
    L.append("")
    L.append("## Setup")
    L.append("")
    L.append(f"- **Date:** {meta.get('date')}")
    L.append(f"- **Agent:** Claude Code {meta.get('claude_version')}, `--model {meta.get('model_alias')}` "
             f"(resolved: `{meta.get('model')}`), `--effort {meta.get('effort')}`, "
             f"`--max-turns {meta.get('max_turns')}`, timeout {meta.get('timeout_s')} s per session"
             + ("; the prompt states the turn limit (the same text in every condition)"
                if meta.get("prompt_budget") else ""))
    L.append(f"- **myceliums:** `{meta.get('myc_version')}` built from commit `{meta.get('myc_commit')}` "
             f"(binary sha256 `{str(meta.get('myc_sha256'))[:16]}…`)"
             + (f"; {meta['mcp_tools_offered']} MCP tools offered" if meta.get("mcp_tools_offered") else ""))
    L.append(f"- **Tasks:** {len(tasks)} from `{meta.get('tasks_file')}` over "
             f"{len({t['repo'] for t in tasks})} pinned repositories, {meta.get('reps')} repetition(s) per task "
             f"and condition, {len(rows)} graded sessions.")
    for name, r in sorted(meta.get("repos", {}).items()):
        idx = meta.get("index", {}).get(name, {})
        if isinstance(idx.get("symbols"), int):
            L.append(f"  - `{name}` {r.get('ref')} (`{r.get('commit', '')[:12]}`, {r.get('language_name')}): "
                     f"index {idx['symbols']:,} symbols in {idx.get('files', 0):,} files")
    L.append("- **Same in every condition:** prompt, model, effort, turn limit, timeout; built-in tools `Read`, "
             + ("`Grep`, `Glob`, `Bash` (+ `ToolSearch`)" if meta.get("bash") else "`Grep`, `Glob` (+ `ToolSearch`)")
             + "; `--setting-sources project` (the working copy's "
             "`.claude/settings.json` and `CLAUDE.md` load, the user's do not); fresh working copy and index "
             "copy per session; transcripts checked for the expected tools, MCP server and subagents.")
    L.append(f"- **Grading:** set-based precision / recall / F1 against gold sets derived with compiler tooling; "
             f"success = F1 ≥ {thr}.")
    L.append(f"- **Statistics:** repetitions averaged within each task; 95 % percentile bootstrap intervals over "
             f"tasks ({n_resamples:,} resamples), paired with condition `{baseline}` for the differences.")
    L.append("")
    L.append("| Condition | What the session has |")
    L.append("|---|---|")
    for c in conditions:
        desc = OUTCOME.get(c, DESCRIPTIONS.get(c, c)) if outcome else DESCRIPTIONS.get(c, c)
        L.append(f"| **{c}** | {desc} |")
    L.append("")

    L.append("## Per condition")
    L.append("")
    L.append(f"Means over {a['means'][baseline]['f1']['n_tasks']} tasks. Brackets: 95 % CI.")
    L.append("")
    L.append("| Metric | " + " | ".join(conditions) + " |")
    L.append("|---|" + "---:|" * len(conditions))
    for key, lab, kind, _fn in METRICS:
        cells = []
        for c in conditions:
            m = a["means"][c][key]
            cells.append(f"{_fmt(m['mean'], kind)} [{_fmt(m['lo'], kind)}, {_fmt(m['hi'], kind)}]")
        L.append(f"| {lab.format(thr=thr)} | " + " | ".join(cells) + " |")
    L.append("")

    others = [c for c in conditions if c != baseline]
    if others:
        L.append(f"## Paired differences vs {baseline}")
        L.append("")
        L.append("Variant minus " + baseline + " per task, then averaged; ratio = mean(variant) / mean(" + baseline
                 + "). Brackets: 95 % CI.")
        L.append("")
        L.append("| Metric | " + " | ".join(f"{c} − {baseline}" for c in others) + " | "
                 + " | ".join(f"{c} ÷ {baseline}" for c in others) + " |")
        L.append("|---|" + "---:|" * (2 * len(others)))
        for key, lab, kind, _fn in METRICS:
            diffs, ratios = [], []
            for c in others:
                s = a["vs_baseline"][c][key]
                diffs.append(f"**{_fmt_diff(s['diff_mean'], kind)}** [{_fmt_diff(s['diff_lo'], kind)}, "
                             f"{_fmt_diff(s['diff_hi'], kind)}]")
                no_ratio = kind not in RATIO_KINDS or key.startswith("myc") or s["ratio_mean"] != s["ratio_mean"]
                ratios.append("–" if no_ratio else
                              f"{_fmt_ratio(s['ratio_mean'])} [{_fmt_ratio(s['ratio_lo'])}, {_fmt_ratio(s['ratio_hi'])}]")
            L.append(f"| {lab.format(thr=thr)} | " + " | ".join(diffs) + " | " + " | ".join(ratios) + " |")
        L.append("")

    L.append("## By task type")
    L.append("")
    L.append("Four tasks per type at most: read these as direction, not size.")
    L.append("")
    head = "| Type | Tasks | " + " | ".join(f"F1 {c}" for c in conditions) + " | " + \
        " | ".join(f"Success {c}" for c in conditions) + " | " + " | ".join(f"myc used {c}" for c in conditions)
    if others:
        head += " | " + " | ".join(f"Δ F1 {c} − {baseline} [95 % CI]" for c in others)
    L.append(head + " |")
    L.append("|---|---:|" + "---:|" * (3 * len(conditions) + len(others)))
    type_n = Counter(t["type"] for t in tasks)
    for typ, m in sorted(a["by_type"].items()):
        cells = [f"{m[c]['f1']:.3f}" for c in conditions] + [_fmt(m[c]["success"], "pct") for c in conditions] \
            + [_fmt(m[c]["myc_used"], "pct") for c in conditions]
        for c in others:
            d = m[f"{c}-{baseline}"]["f1"]
            cells.append(f"{_fmt_diff(d['diff_mean'], 'f3')} [{_fmt_diff(d['diff_lo'], 'f3')}, "
                         f"{_fmt_diff(d['diff_hi'], 'f3')}]")
        L.append(f"| {typ} | {type_n[typ]} | " + " | ".join(cells) + " |")
    L.append("")

    L.append("## Per task")
    L.append("")
    L.append("F1 per repetition, in repetition order; myceliums calls summed over the repetitions.")
    L.append("")
    L.append("| Task | Type | Gold | " + " | ".join(f"F1 {c}" for c in conditions) + " | "
             + " | ".join(f"myc calls {c}" for c in conditions) + " |")
    L.append("|---|---|---:|" + "---|" * len(conditions) + "---:|" * len(conditions))
    runs = defaultdict(list)
    for r in sorted(rows, key=lambda r: (r["task_id"], r["condition"], r["rep"])):
        runs[(r["task_id"], r["condition"])].append(r)
    for t in tasks:
        f1s = [" ".join("%.2f" % r["f1"] for r in runs[(t["id"], c)]) or "–" for c in conditions]
        calls = [str(sum(r["mcp_tool_calls"] for r in runs[(t["id"], c)])) for c in conditions]
        L.append(f"| `{t['id']}` | {t['type']} | {t['n_gold']} | " + " | ".join(f1s) + " | "
                 + " | ".join(calls) + " |")
    L.append("")

    L.append("## How the agent used myceliums")
    L.append("")
    L.append("| Condition | Sessions | ≥ 1 myc call | myc calls | of which by the subagent | Share of tool calls "
             "(median) | First tool (most common) |")
    L.append("|---|---:|---:|---:|---:|---:|---|")
    for c in conditions:
        rs = by_cond[c]
        used = sum(1 for r in rs if r["mcp_tool_calls"])
        calls = sum(r["mcp_tool_calls"] for r in rs)
        sub = sum(r.get("subagent_mcp_tool_calls") or 0 for r in rs)
        share = [r["mcp_tool_calls"] / r["tool_calls"] for r in rs if r["tool_calls"]]
        first = Counter(r["first_tool"] for r in rs).most_common(2)
        L.append(f"| {c} | {len(rs)} | {used} | {calls} | {sub} | "
                 f"{(100 * median(share)):.0f} % | " + ", ".join(f"`{k}` {v}" for k, v in first) + " |"
                 if share else f"| {c} | {len(rs)} | {used} | {calls} | {sub} | – | – |")
    L.append("")
    tools = sorted({k for r in rows for k in (r["tool_calls_by_name"] or {}) if k.startswith(PREFIX)})
    if tools:
        L.append("Calls per myceliums tool (errors in brackets):")
        L.append("")
        L.append("| Tool | " + " | ".join(conditions) + " |")
        L.append("|---|" + "---:|" * len(conditions))
        for tool in tools:
            cells = []
            for c in conditions:
                n = sum((r["tool_calls_by_name"] or {}).get(tool, 0) for r in by_cond[c])
                e = sum((r["tool_errors"] or {}).get(tool, 0) for r in by_cond[c])
                cells.append(f"{n} ({e})" if e else str(n))
            L.append(f"| `{tool[len(PREFIX):]}` | " + " | ".join(cells) + " |")
        L.append("")
    builtin = sorted({k for r in rows for k in (r["tool_calls_by_name"] or {}) if not k.startswith(PREFIX)})
    if builtin:
        L.append("Built-in tool calls per session (mean): " + "; ".join(
            f"**{c}** " + ", ".join(f"`{k}` {mean([(r['tool_calls_by_name'] or {}).get(k, 0) for r in by_cond[c]]):.1f}"
                                    for k in builtin) for c in conditions) + ".")
        L.append("")
    hooked = [c for c in conditions if any(r.get("hook_grep_calls") is not None and config.VARIANTS[c].setup_claude
                                           for r in by_cond[c])]
    if hooked:
        L.append("**Hooks** (from the wrapper's logs; the agent saw only what Claude Code passed on):")
        L.append("")
        for c in hooked:
            rs = by_cond[c]
            calls = sum(r.get("hook_grep_calls") or 0 for r in rs)
            hints = sum(r.get("hook_grep_hints") or 0 for r in rs)
            reasons = Counter()
            for r in rs:
                reasons.update(r.get("hook_grep_silent_reasons") or {})
            fmt = sum(r.get("format_hook_calls") or 0 for r in rs)
            ready = sum(1 for r in rs if " ready " in (r.get("session_hook") or ""))
            # Since #152 the hook prints JSON: an `additionalContext` line for the
            # model ("... (index: <commit> = HEAD) ...") next to the systemMessage.
            ctx = [r.get("session_hook") or "" for r in rs if "additionalContext" in (r.get("session_hook") or "")]
            session = (f"The SessionStart hook put its context line into the model's context in "
                       f"{len(ctx)} of {len(rs)} sessions, naming the index "
                       f"current (`index: <commit> = HEAD`) in {sum(1 for x in ctx if '= HEAD' in x)} of them."
                       if ctx else
                       f"The SessionStart hook reported the index ready in {ready} of {len(rs)} sessions (a "
                       "`systemMessage`, shown to the user, not to the model).")
            L.append(f"- **{c}:** the grep hook ran on {calls} Grep calls and added a hint to {hints}; silent "
                     "because " + (", ".join(f"{k} ({v})" for k, v in reasons.most_common(4)) or "–")
                     + f". The PostToolUse formatter ran {fmt} times. " + session)
        L.append("")
    if any(config.VARIANTS[c].agent for c in conditions):
        for c in conditions:
            if not config.VARIANTS[c].agent:
                continue
            rs = by_cond[c]
            delegated = sum(1 for r in rs if r.get("agent_calls"))
            L.append(f"- **{c}:** the main agent delegated to `code-explorer` in {delegated} of {len(rs)} sessions "
                     f"({sum(r.get('agent_calls') or 0 for r in rs)} Agent calls); subagents made "
                     f"{sum(r.get('subagent_tool_calls') or 0 for r in rs)} tool calls, "
                     f"{sum(r.get('subagent_mcp_tool_calls') or 0 for r in rs)} of them to myceliums.")
        L.append("")

    L.append("## Sessions")
    L.append("")
    st = Counter((r["condition"], r["status"]) for r in rows)
    stats = sorted({s for _, s in st})
    L.append("| Condition | " + " | ".join(stats) + " |")
    L.append("|---|" + "---:|" * len(stats))
    for c in conditions:
        L.append(f"| {c} | " + " | ".join(str(st.get((c, s), 0)) for s in stats) + " |")
    L.append("")
    L.append(f"- Graded sessions: {len(rows)}. Agent sessions launched for this run (ledger), including any "
             f"retry: {meta.get('sessions_launched_ledger')}; attempts recorded: {meta.get('attempts_total')}.")
    if meta.get("retried_runs"):
        L.append(f"- Retried: {', '.join('`' + x + '`' for x in meta['retried_runs'])}.")
    if meta.get("notes"):
        L.append("")
        L.append("## Notes")
        L.append("")
        for n in meta["notes"]:
            L.append(f"- {n}")
    L.append("")
    summary = {"label": label, "threshold": thr, "baseline": baseline, "conditions": conditions,
               "analysis": a, "n_rows": len(rows),
               "statuses": {f"{c}/{s}": n for (c, s), n in sorted(st.items())}}
    return "\n".join(L) + "\n", summary
