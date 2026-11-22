"""Report for a re-run of one condition of the outcome eval at scale (#171).

#171 re-runs condition ``C`` of #163 at some of its budgets with a newer myc
(after #169) and fresh indexes, and compares it, paired by task, with the
committed #163 runs of ``none`` and of the old ``C`` (``meta["rerun_of"]``
names that results directory). Only the new ``C`` sessions live in this
directory's ``results.jsonl``; the #163 rows are read from the committed
results of ``rerun_of``, so the report stays a deterministic function of
committed files.

Two task views. ``meta["primary_exclude"]`` (task id -> reason) leaves tasks
out of the primary analysis, as in #163, and the report gives the view over
all tasks alongside; without it the primary analysis covers every task and
``meta["continuity_exclude"]`` names the tasks #163 left out, for a view
comparable with #163's primary analysis. Statistics as in #163: means over tasks with 95 %
paired bootstrap intervals, an exact McNemar test per budget, nothing
corrected for multiple comparisons.
"""

from __future__ import annotations

import json
from collections import Counter

from . import config
from .report import _fmt
from .report_scale import (ARM, BASE, HEADLINE, METRICS, PREFIX, _diff, _sign, compare, myc_calls)
from .stats import mean

NEW = "C (#171)"
OLD_C = "C (#163)"
OLD_NONE = "none (#163)"


def _pair(base_rows: list[dict], arm_rows: list[dict]) -> list[dict]:
    """Rows for :func:`report_scale.compare`: ``base_rows`` as the baseline, ``arm_rows`` as the arm."""
    return [dict(r, condition=BASE) for r in base_rows] + [dict(r, condition=ARM) for r in arm_rows]


def load(label: str) -> tuple[dict, list[dict], list[dict], list[dict], dict]:
    """meta, new C rows, #163 none rows, #163 C rows (budgets of the re-run only), #163 meta."""
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    rows = [json.loads(x) for x in (res_dir / "results.jsonl").read_text().splitlines() if x.strip()]
    base_dir = config.RESULTS_DIR / meta["rerun_of"]
    base_meta = config.load_json(base_dir / "meta.json")
    base = [json.loads(x) for x in (base_dir / "results.jsonl").read_text().splitlines() if x.strip()]
    budgets = set(meta["budgets"])
    base = [r for r in base if r["budget"] in budgets]
    return (meta, rows, [r for r in base if r["condition"] == "none"], [r for r in base if r["condition"] == "C"],
            base_meta)


def analyse(new, none, old, ids, budgets, n_resamples=10_000) -> dict:
    """Per budget: the new C against #163's none and against #163's C."""
    out = {}
    for b in budgets:
        nb = [r for r in new if r["budget"] == b]
        out[str(b)] = {
            "vs_none": compare(_pair([r for r in none if r["budget"] == b], nb), ids, n_resamples),
            "vs_old_c": compare(_pair([r for r in old if r["budget"] == b], nb), ids, n_resamples),
        }
    out["all"] = {"vs_none": compare(_pair(none, new), ids, n_resamples),
                  "vs_old_c": compare(_pair(old, new), ids, n_resamples)}
    return out


def _bname(b, meta) -> str:
    if b == "all":
        return "All re-run budgets"
    run = meta.get("runs", {}).get(str(b), {})
    return f"{b} turns" + ("" if run.get("prompt_budget", True) else " (cap; no budget stated)")


def _hint_rows(rows: list[dict]) -> dict:
    sessions = [r for r in rows if r.get("incomplete_results")]
    kinds = Counter()
    for r in rows:
        kinds.update(r.get("next_kinds") or {})
    return {
        "sessions": len(rows),
        "sessions_with_hint": len(sessions),
        "hinted_results": sum(r.get("incomplete_results") or 0 for r in rows),
        "followed_results": sum(r.get("next_followed") or 0 for r in rows),
        "sessions_followed": sum(1 for r in sessions if r.get("next_followed")),
        "sessions_source_after": sum(1 for r in sessions if r.get("source_after_hint")),
        "success_with_hint": sum(1 for r in sessions if r["success"]),
        "kinds": dict(sorted(kinds.items())),
    }


def render(label: str, n_resamples: int = 10_000) -> tuple[str, dict]:
    meta, new, none, old, base_meta = load(label)
    tasks = config.load_json(config.RESULTS_DIR / label / "tasks_snapshot.json")
    ran = {r["task_id"] for r in new}
    tasks = [t for t in tasks if t["id"] in ran]
    budgets = meta["budgets"]
    thr = meta.get("success_threshold", 0.8)
    prim_ex = {k: v for k, v in (meta.get("primary_exclude") or {}).items() if k in ran}
    cont_ex = prim_ex or {k: v for k, v in (meta.get("continuity_exclude") or {}).items() if k in ran}
    ids_all = [t["id"] for t in tasks]
    ids_sub = [t for t in ids_all if t not in cont_ex]
    # The primary view and the one given alongside.
    ids_p, ids_s = (ids_sub, ids_all) if prim_ex else (ids_all, ids_sub)
    a = analyse(new, none, old, ids_p, budgets, n_resamples)
    a_cont = analyse(new, none, old, ids_s, budgets, n_resamples)
    na, nc = len(ids_p), len(ids_s)
    p_name = f"{na} (primary)" if prim_ex else f"all {na} (primary)"
    s_name = f"all {nc}" if prim_ex else f"{nc} (continuity)"
    same_tasks = meta.get("tasks_sha256") == base_meta.get("tasks_sha256")
    L: list[str] = []
    L.append(f"# Agent benchmark: {label}")
    L.append("")
    L.append(f"A re-run of condition C of [`{meta['rerun_of']}`](../{meta['rerun_of']}/REPORT.md) (#163) at "
             + " and ".join(str(b) for b in budgets) + " turns with a newer myc and fresh indexes, compared, paired "
             f"by task, with the committed #163 sessions of none and of the old C. Generated by `python3 "
             f"evals/agent/agent_eval.py report {label}` from `results.jsonl`, `meta.json` and `tasks_snapshot.json` "
             f"in this directory and the committed `results.jsonl` of `{meta['rerun_of']}`; method in "
             "[`../../README.md`](../../README.md).")
    L.append("")
    L.append("## Setup")
    L.append("")
    L.append(f"- **Date:** {meta.get('date')} (the #163 sessions: {base_meta.get('date')})")
    L.append(f"- **Agent:** Claude Code {meta.get('claude_version')} (#163: {base_meta.get('claude_version')}), "
             f"`--model {meta.get('model_alias')}` (resolved: `{meta.get('model')}`; #163: `{base_meta.get('model')}`), "
             f"`--effort {meta.get('effort')}` (#163: `{base_meta.get('effort')}`)")
    parts = []
    for b in budgets:
        r = meta.get("runs", {}).get(str(b), {})
        old_r = base_meta.get("runs", {}).get(str(b), {})
        parts.append(f"`--max-turns {b}` ({'stated in the prompt' if r.get('prompt_budget') else 'a safety cap, not stated in the prompt'}"
                     f", timeout {r.get('timeout_s')} s; #163: {'stated' if old_r.get('prompt_budget') else 'not stated'}, "
                     f"{old_r.get('timeout_s')} s)")
    L.append("- **Budgets:** " + "; ".join(parts) + ".")
    L.append(f"- **myceliums (C #171):** `{meta.get('myc_version')}` built from commit `{meta.get('myc_commit')}` "
             f"(binary sha256 `{str(meta.get('myc_sha256'))[:16]}…`); {meta.get('mcp_tools_offered')} MCP tools offered. "
             f"C #163: `{base_meta.get('myc_version')}` from `{base_meta.get('myc_commit')}` (sha256 "
             f"`{str(base_meta.get('myc_sha256'))[:16]}…`).")
    L.append(f"- **Tasks:** {len(ids_all)} from `{meta.get('tasks_file')}` (sha256 `{str(meta.get('tasks_sha256'))[:16]}…`, "
             + ("the file the #163 results are graded against" if same_tasks else
                f"**differs** from the #163 results' `{str(base_meta.get('tasks_sha256'))[:16]}…`")
             + f"), {meta.get('reps')} repetition per task and budget; {len(new)} graded C sessions.")
    for name in sorted({t["repo"] for t in tasks}):
        r = meta.get("repos", {}).get(name, {})
        idx = meta.get("index", {}).get(name, {})
        old_idx = base_meta.get("index", {}).get(name, {})
        n = sum(1 for t in tasks if t["repo"] == name)
        excl = idx.get("index_exclude_extra") or []
        L.append(f"  - `{name}` {r.get('ref')} (`{r.get('commit', '')[:12]}`, {r.get('language_name')}): {n} tasks; "
                 f"fresh index {idx.get('symbols', 0):,} symbols in {idx.get('files', 0):,} files, "
                 f"{idx.get('relationships') or 0:,} relationships, {idx.get('embedded') or 0:,} embedded"
                 + (f", not indexed: {', '.join('`' + x + '`' for x in excl)}" if excl else
                    ", myceliums' default configuration (nothing excluded beyond its defaults)")
                 + f" (#163: {old_idx.get('symbols', 0):,} symbols in {old_idx.get('files', 0):,} files, "
                 f"{old_idx.get('relationships') or 0:,} relationships)")
    if prim_ex:
        L.append(f"- **Primary analysis:** the {na} tasks without " + ", ".join(f"`{k}`" for k in sorted(prim_ex))
                 + ". " + " ".join(sorted(set(prim_ex.values()))) + f" The {nc}-task numbers are given alongside.")
    else:
        L.append(f"- **Primary analysis:** all {na} tasks. The #163 primary analysis left out "
                 + ", ".join(f"`{k}`" for k in sorted(cont_ex)) + " because its Django index had no `tests/`; the "
                 f"fresh index has, so the confound is gone. The {nc}-task view is reported alongside for continuity.")
    L.append("- **Comparisons:** C (#171) against none (#163) and against C (#163), paired by task at the same "
             "budget. Same prompt, model alias, effort, turn limits, timeouts, built-in tools (`Read`, `Grep`, "
             "`Glob`, `Bash`), isolation and `myc setup-claude --scope project --no-agent` as #163; a fresh "
             "working copy and index copy per session.")
    L.append("- **Statistics:** tasks are the unit; 95 % percentile bootstrap intervals "
             f"({n_resamples:,} resamples over tasks, paired); per budget an exact two-sided McNemar test on success; "
             "*all re-run budgets* averages each task over its budgets and uses an exact sign test. **No interval or "
             "p-value is corrected for multiple comparisons** (two budgets, two baselines, two task views, several "
             "metrics, repositories and task types); the repository and task-type tables are exploratory.")
    L.append(f"- **Time drift:** the none and old C arms ran on {base_meta.get('date')} before this re-run, not "
             "interleaved with it; a change of the model behind the alias or of the service in between would show up "
             "as a difference, too.")
    L.append("- **Labels:** the gold sets of this task file were drafted by an agent from the AST and the type checker "
             "and are not reviewed (the #164 review left this set out).")
    L.append("")

    # ---- per budget
    L.append("## Per budget")
    L.append("")
    L.append(f"Means over the {na} tasks of the primary analysis; brackets: 95 % CI (uncorrected). Differences are percentage points or "
             f"absolute, ratios new ÷ old. Below each table the {nc}-task view.")
    L.append("")
    for b in [*budgets, "all"]:
        s = a[str(b)]
        sc = a_cont[str(b)]
        vn, vo = s["vs_none"], s["vs_old_c"]
        L.append(f"### {_bname(b, meta)}")
        L.append("")
        L.append(f"| Metric | {OLD_NONE} | {OLD_C} | {NEW} | {NEW} − none | {NEW} − {OLD_C} |")
        L.append("|---|---:|---:|---:|---:|---:|")
        for key, lab, kind, _fn, ratio in METRICS:
            if key not in HEADLINE:
                continue
            m_n, m_o = vn[key], vo[key]
            L.append(f"| {lab.format(thr=thr)} | {_fmt(m_n['b_mean'], kind)} | {_fmt(m_o['b_mean'], kind)} | "
                     f"{_fmt(m_n['a_mean'], kind)} [{_fmt(m_n['a_lo'], kind)}, {_fmt(m_n['a_hi'], kind)}] | "
                     f"{_diff(m_n, kind, ratio)} | {_diff(m_o, kind, ratio)} |")
        test = "sign test" if b == "all" else "McNemar"
        L.append(f"| Success, {test} (new better : old better) | | | | {_sign(vn['sign_test'])} | "
                 f"{_sign(vo['sign_test'])} |")
        L.append("")
        cn, co = sc["vs_none"], sc["vs_old_c"]
        L.append(f"{'All ' if prim_ex else ''}{nc} tasks: success none {_fmt(cn['success']['b_mean'], 'pct')}, C (#163) "
                 f"{_fmt(co['success']['b_mean'], 'pct')}, C (#171) {_fmt(cn['success']['a_mean'], 'pct')} "
                 f"(vs none {_diff(cn['success'], 'pct', False)}, {test} {_sign(cn['sign_test'])}; vs C (#163) "
                 f"{_diff(co['success'], 'pct', False)}, {_sign(co['sign_test'])}); mean F1 "
                 f"{cn['f1']['b_mean']:.3f} / {co['f1']['b_mean']:.3f} / {cn['f1']['a_mean']:.3f} (vs none "
                 f"{_diff(cn['f1'], 'f3', False)}, vs C (#163) {_diff(co['f1'], 'f3', False)}).")
        L.append("")

    # ---- headline
    L.append("## Summary")
    L.append("")
    L.append(f"| Tasks | Budget | Success none / C (#163) / C (#171) | Δ vs none [95 % CI], McNemar | "
             "Δ vs C (#163) [95 % CI], McNemar | Mean F1 none / C (#163) / C (#171) | Δ F1 vs none | Δ F1 vs C (#163) |")
    L.append("|---|---|---:|---:|---:|---:|---:|---:|")
    for tag, an in ((p_name, a), (s_name, a_cont)):
        for b in budgets:
            vn, vo = an[str(b)]["vs_none"], an[str(b)]["vs_old_c"]
            L.append(f"| {tag} | {_bname(b, meta)} | {_fmt(vn['success']['b_mean'], 'pct')} / "
                     f"{_fmt(vo['success']['b_mean'], 'pct')} / {_fmt(vn['success']['a_mean'], 'pct')} | "
                     f"{_diff(vn['success'], 'pct', False)}, {_sign(vn['sign_test'])} | "
                     f"{_diff(vo['success'], 'pct', False)}, {_sign(vo['sign_test'])} | "
                     f"{vn['f1']['b_mean']:.3f} / {vo['f1']['b_mean']:.3f} / {vn['f1']['a_mean']:.3f} | "
                     f"{_diff(vn['f1'], 'f3', False)} | {_diff(vo['f1'], 'f3', False)} |")
    L.append("")

    # ---- per repo and per type
    groups_out = {}
    for key, title in (("repo", "By repository"), ("type", "By task type")):
        L.append(f"## {title} (exploratory)")
        L.append("")
        L.append(f"Primary analysis ({na} tasks). Success none (#163) / C (#163) / C (#171) per budget; then C (#171) − none over the "
                 "re-run budgets with its 95 % CI and sign test, uncorrected. Few tasks per group: direction, not size.")
        L.append("")
        L.append(f"| {'Repository' if key == 'repo' else 'Type'} | Tasks | "
                 + " | ".join(f"{b} turns" for b in budgets) + " | Δ success vs none | Sign test vs none | "
                 "Δ success vs C (#163) | Sign test vs C (#163) |")
        L.append("|---|---:|" + "---:|" * len(budgets) + "---:|---|---:|---|")
        for gname in sorted({t[key] for t in tasks if t["id"] in ids_p}):
            gids = [t["id"] for t in tasks if t[key] == gname and t["id"] in ids_p]
            g = analyse(new, none, old, gids, budgets, n_resamples)
            groups_out[f"{key}:{gname}"] = g
            cells = [f"{_fmt(g[str(b)]['vs_none']['success']['b_mean'], 'pct')} / "
                     f"{_fmt(g[str(b)]['vs_old_c']['success']['b_mean'], 'pct')} / "
                     f"{_fmt(g[str(b)]['vs_none']['success']['a_mean'], 'pct')}" for b in budgets]
            gn, go = g["all"]["vs_none"], g["all"]["vs_old_c"]
            L.append(f"| {gname} | {len(gids)} | " + " | ".join(cells) + f" | {_diff(gn['success'], 'pct', False)} | "
                     f"{_sign(gn['sign_test'])} | {_diff(go['success'], 'pct', False)} | {_sign(go['sign_test'])} |")
        L.append("")

    # ---- per task
    L.append("## Per task")
    L.append("")
    L.append(f"F1 none (#163) / C (#163) / C (#171) at each budget; myceliums calls (MCP + CLI) and results with an "
             f"Incomplete block in C (#171), summed over the budgets. Tasks marked † are "
             + ("not in the primary analysis." if prim_ex else f"outside the {nc}-task view."))
    L.append("")
    L.append("| Task | Type | Gold | " + " | ".join(f"{b} turns" for b in budgets)
             + " | myc calls C (#171) | Incomplete results |")
    L.append("|---|---|---:|" + "---|" * len(budgets) + "---:|---:|")
    ix = {}
    for tag, rs in (("n", none), ("o", old), ("c", new)):
        for r in rs:
            ix[(tag, r["task_id"], r["budget"])] = r
    for t in tasks:
        cells = []
        for b in budgets:
            vals = [ix.get((tag, t["id"], b)) for tag in ("n", "o", "c")]
            cells.append(" / ".join(f"{v['f1']:.2f}" if v else "–" for v in vals))
        mine = [r for r in new if r["task_id"] == t["id"]]
        L.append(f"| `{t['id']}`{' †' if t['id'] in cont_ex else ''} | {t['type']} | {t['n_gold']} | "
                 + " | ".join(cells) + f" | {sum(myc_calls(r) for r in mine)} | "
                 f"{sum(r.get('incomplete_results') or 0 for r in mine)} |")
    L.append("")

    # ---- hints
    L.append("## Incomplete and Next hints (#169)")
    L.append("")
    L.append("Since #169 a graph walk that stops early carries an `Incomplete (N branch(es) not followed)` block (or "
             "`incomplete` in `find_callers` / `find_callees`) ending in `Next:` with the step that continues it. "
             "Counted from the transcripts (`aeval/hints.py`): a result is *followed* when a later tool call takes one "
             "of the steps its `Next` names, judged by the kind of call (the same myceliums tool; for “read the "
             "source” a `Read`, `Grep` or non-`myc` Bash call; for “search the source for `X(`” a `Grep` or Bash call "
             "containing `X`), not by its exact arguments. The #163 binary had no such hints.")
    L.append("")
    L.append("| Budget | C (#171) sessions | Sessions with ≥ 1 hint | Results with a hint | … Next step taken | "
             "Sessions taking ≥ 1 step | Sessions reading the source after a hint | Successful sessions with a hint |")
    L.append("|---|---:|---:|---:|---:|---:|---:|---:|")
    hints_out = {}
    for b in [*budgets, "all"]:
        rs = new if b == "all" else [r for r in new if r["budget"] == b]
        h = _hint_rows(rs)
        hints_out[str(b)] = h
        L.append(f"| {_bname(b, meta)} | {h['sessions']} | {h['sessions_with_hint']} | {h['hinted_results']} | "
                 f"{h['followed_results']} | {h['sessions_followed']} | {h['sessions_source_after']} | "
                 f"{h['success_with_hint']} of {h['sessions_with_hint']} |")
    L.append("")
    kinds = hints_out["all"]["kinds"]
    if kinds:
        names = {"call": "a myceliums call (deeper walk, larger window)", "read_source": "read the source",
                 "grep_source": "search the source", "full_analysis": "a full analysis", "other": "other"}
        L.append("Next steps offered over all budgets (a Next can name several): " + ", ".join(
            f"{names.get(k, k)} {v}" for k, v in sorted(kinds.items())) + ".")
        L.append("")

    # ---- myceliums use
    L.append("## How the agent used myceliums")
    L.append("")
    L.append("| Budget | Arm | C sessions | ≥ 1 myc call | via MCP | via CLI (Bash) | MCP calls | CLI runs | "
             "First tool (most common) |")
    L.append("|---|---|---:|---:|---:|---:|---:|---:|---|")
    for b in budgets:
        for tag, rs0 in ((OLD_C, old), (NEW, new)):
            cs = [r for r in rs0 if r["budget"] == b]
            first = Counter(r["first_tool"] for r in cs).most_common(2)
            L.append(f"| {_bname(b, meta)} | {tag} | {len(cs)} | {sum(1 for r in cs if myc_calls(r))} | "
                     f"{sum(1 for r in cs if r.get('mcp_tool_calls'))} | {sum(1 for r in cs if r.get('myc_cli_calls'))} | "
                     f"{sum(r.get('mcp_tool_calls') or 0 for r in cs)} | {sum(r.get('myc_cli_calls') or 0 for r in cs)} | "
                     + ", ".join(f"`{k}` {v}" for k, v in first) + " |")
    L.append("")
    for tag, rs0 in ((OLD_C, old), (NEW, new)):
        tools, errors, cli = Counter(), Counter(), Counter()
        for r in rs0:
            for k, v in (r.get("tool_calls_by_name") or {}).items():
                if k.startswith(PREFIX):
                    tools[k[len(PREFIX):]] += v
            for k, v in (r.get("tool_errors") or {}).items():
                if k.startswith(PREFIX):
                    errors[k[len(PREFIX):]] += v
            cli.update(r.get("myc_cli_subcommands") or [])
        if tools:
            L.append(f"- MCP tool calls in {tag} over the re-run budgets (errors in brackets): " + ", ".join(
                f"`{k}` {v}" + (f" ({errors[k]})" if errors.get(k) else "") for k, v in sorted(tools.items())) + ".")
        if cli:
            L.append(f"- `myc` CLI subcommands run through Bash in {tag} (sessions using each): " + ", ".join(
                f"`{k}` {v}" for k, v in sorted(cli.items())) + ".")
    builtin = sorted({k for r in [*old, *new, *none] for k in (r.get("tool_calls_by_name") or {})
                      if not k.startswith(PREFIX)})
    for b in budgets:
        per = []
        for tag, rs0 in ((OLD_NONE, none), (OLD_C, old), (NEW, new)):
            rs = [r for r in rs0 if r["budget"] == b]
            per.append(f"**{tag}** " + ", ".join(
                f"`{k}` {mean([(r.get('tool_calls_by_name') or {}).get(k, 0) for r in rs]):.1f}" for k in builtin))
        L.append(f"- Built-in tool calls per session at {_bname(b, meta)} (mean): " + "; ".join(per) + ".")
    calls = sum(r.get("hook_grep_calls") or 0 for r in new)
    hints = sum(r.get("hook_grep_hints") or 0 for r in new)
    ctx = [r.get("session_hook") or "" for r in new if "additionalContext" in (r.get("session_hook") or "")]
    L.append(f"- Hooks in C (#171): the grep hook ran on {calls} Grep or Bash `grep`/`rg` calls and added a hint to "
             f"{hints}; the PostToolUse formatter ran {sum(r.get('format_hook_calls') or 0 for r in new)} times; the "
             f"SessionStart hook put its context line into {len(ctx)} of {len(new)} sessions, naming the index current "
             f"in {sum(1 for x in ctx if '= HEAD' in x)}.")
    L.append("")

    # ---- sessions
    L.append("## Sessions")
    L.append("")
    st = Counter((r["budget"], r["status"]) for r in new)
    stats = sorted({r["status"] for r in [*new, *none, *old]})
    L.append("| Budget | Arm | " + " | ".join(stats) + " |")
    L.append("|---|---|" + "---:|" * len(stats))
    old_st = Counter((r["budget"], r["condition"], r["status"]) for r in [*none, *old])
    for b in budgets:
        for tag, cond in ((OLD_NONE, "none"), (OLD_C, "C")):
            L.append(f"| {b} | {tag} | " + " | ".join(str(old_st.get((b, cond, s), 0)) for s in stats) + " |")
        L.append(f"| {b} | {NEW} | " + " | ".join(str(st.get((b, s), 0)) for s in stats) + " |")
    L.append("")
    for b in budgets:
        r = meta.get("runs", {}).get(str(b), {})
        L.append(f"- `{r.get('label')}` ({_bname(b, meta)}): {r.get('runs_final')} graded, "
                 f"{r.get('sessions_launched_ledger')} agent sessions launched (ledger, retries included)"
                 + (f"; retried: {', '.join('`' + x + '`' for x in r['retried_runs'])}" if r.get("retried_runs") else
                    "; no retries")
                 + ".")
    if meta.get("notes"):
        L.append("")
        L.append("## Notes")
        L.append("")
        for n in meta["notes"]:
            L.append(f"- {n}")
    L.append("")
    summary = {"label": label, "rerun_of": meta["rerun_of"], "threshold": thr, "budgets": budgets,
               "analysis": a, "analysis_continuity": a_cont, "primary_exclude": prim_ex, "continuity_exclude": cont_ex, "groups": groups_out,
               "hints": hints_out, "n_rows": len(new),
               "statuses": {f"{b}/{s}": n for (b, s), n in sorted(st.items())}}
    return "\n".join(L) + "\n", summary
