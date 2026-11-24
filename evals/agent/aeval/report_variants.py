"""Report for runs with conditions other than the pilot's with/without (#125).

The variant matrix compares integration layers of myceliums (``A``: MCP server
only; ``C``: what ``myc setup-claude --scope project --no-agent`` installs;
``E``: the same plus the code-explorer subagent) on the same tasks. Every
variant is compared with ``A`` by a paired bootstrap over tasks. A run with a
single condition (the calibration run, ``none``) gets the per-task table and
the means only. A run exported with ``comparisons: pairwise`` (#182, the
interleaved A/C/D runs) has no baseline: every pair of conditions is compared
(McNemar on task success, paired bootstrap on F1, success and spend per
solved task, Holm over the success tests), and recall and success are given
per task category.

Like :mod:`aeval.report`, the output is a deterministic function of the
committed ``results.jsonl``, ``meta.json`` and ``tasks_snapshot.json``.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict

from . import config, report_commit
from .report import _fmt, _fmt_diff, _fmt_ratio
from .stats import exact_sign_test, mean, median, paired_bootstrap, per_success_bootstrap

PREFIX = "mcp__myceliums__"

DESCRIPTIONS = {
    "none": "no MCP server (calibration baseline); project settings and CLAUDE.md files load, none exist",
    "A": "the myceliums MCP server only (default core toolset, 9 tools loaded up front)",
    "C": "A + what `myc setup-claude --scope project --no-agent` writes: the routing block in CLAUDE.md, "
         "the SessionStart, PostToolUse and PreToolUse grep-redirect hooks in .claude/settings.json",
    "D": "myceliums only (#178): the MCP server and `Read`; `Grep` and `Glob` denied; Bash, if offered, limited "
         "by a PreToolUse hook to an allowlist (`myc`, plain file operations, git without search, the task's own "
         "build/test commands); edit tools on edit tasks",
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

def _myc_calls(r: dict) -> int:
    """MCP calls, plus ``myc`` runs from Bash where the session had myceliums
    (#163/#182: with ``--bash`` C and D reach the CLI; in ``none`` a ``myc``
    command finds no binary and is not a myceliums call)."""
    cli = r.get("myc_cli_calls") or 0
    return r["mcp_tool_calls"] + (cli if config.VARIANTS[r["condition"]].mcp else 0)


METRICS = [
    # key, label, format, per-run extractor
    ("myc_used", "Sessions with ≥ 1 myceliums call", "pct", lambda r: 1.0 if _myc_calls(r) else 0.0),
    ("myc_calls", "myceliums calls per session", "f1", _myc_calls),
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

# Edit tasks have no answer list: what the same columns hold there (#183).
EDIT_LABELS = {"success": "Success rate (the task's tests pass)", "f1": "File F1 (changed files vs. the commit's)",
               "precision": "File precision", "recall": "File recall (all of the commit's files)"}


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


def _holm(ps: list[float]) -> list[float]:
    """Holm-Bonferroni adjusted p-values, in the input order."""
    order = sorted(range(len(ps)), key=lambda i: ps[i])
    out, running = [0.0] * len(ps), 0.0
    for rank, i in enumerate(order):
        running = max(running, min(1.0, (len(ps) - rank) * ps[i]))
        out[i] = running
    return out


def pairwise(rows, tasks, conditions, n_resamples=10_000) -> dict:
    """#182: every pair of conditions, no baseline. A task succeeds in a
    condition when at least half of its repetitions do (McNemar's exact test
    over the tasks that succeed in one condition of the pair only); F1 and the
    success rate are compared per task by a paired bootstrap; tokens (input +
    output) and the CLI cost per solved task by :func:`per_success_bootstrap`
    (ratio first ÷ second)."""
    ids = [t["id"] for t in tasks]
    succ = _per_task(rows, lambda r: 1.0 if r["success"] else 0.0)
    f1 = _per_task(rows, lambda r: r["f1"])
    spend: dict = defaultdict(lambda: defaultdict(lambda: [0.0, 0.0, 0.0]))
    for r in rows:
        a = spend[r["task_id"]][r["condition"]]
        a[0] += (r.get("total_input_tokens") or 0) + (r.get("output_tokens") or 0)
        a[1] += r.get("cost_usd") or 0.0
        a[2] += 1.0 if r["success"] else 0.0
    out = []
    for i, x in enumerate(conditions):
        for y in conditions[i + 1:]:
            both = [t for t in ids if x in succ.get(t, {}) and y in succ.get(t, {})]
            wins = sum(1 for t in both if succ[t][x] >= 0.5 > succ[t][y])
            losses = sum(1 for t in both if succ[t][y] >= 0.5 > succ[t][x])
            per_tok = [(spend[t][x][0], spend[t][x][2], spend[t][y][0], spend[t][y][2]) for t in both]
            per_usd = [(spend[t][x][1], spend[t][x][2], spend[t][y][1], spend[t][y][2]) for t in both]
            out.append({"a": x, "b": y, "n_tasks": len(both), "wins": wins, "losses": losses,
                        "mcnemar_p": exact_sign_test(wins, losses),
                        "success": paired_bootstrap(_pairs(succ, ids, x, y), n_resamples).as_dict(),
                        "f1": paired_bootstrap(_pairs(f1, ids, x, y), n_resamples).as_dict(),
                        "tokens_per_solved": per_success_bootstrap(per_tok, n_resamples),
                        "cost_per_solved": per_success_bootstrap(per_usd, n_resamples)})
    for c, p in zip(out, _holm([c["mcnemar_p"] for c in out])):
        c["mcnemar_p_holm"] = p
    return {"pairs": out}


def by_category(rows, tasks, conditions) -> dict:
    """#182: recall, success and F1 per task category (``category``, else ``type``) and condition."""
    cat = {t["id"]: t.get("category") or t["type"] for t in tasks}
    out: dict = {}
    for key, fn in (("recall", lambda r: r["recall"]), ("success", lambda r: 1.0 if r["success"] else 0.0),
                    ("f1", lambda r: r["f1"])):
        pt = _per_task(rows, fn)
        for c in conditions:
            for k in sorted(set(cat.values())):
                vals = [pt[t][c] for t in cat if cat[t] == k and c in pt.get(t, {})]
                out.setdefault(k, {"n_tasks": sum(1 for t in cat if cat[t] == k)}).setdefault(c, {})[key] = \
                    mean(vals) if vals else float("nan")
    return out


def _pairwise_lines(pw: dict, cats: dict, conditions: list[str], thr: float, n_resamples: int,
                    notes: list[str] = ()) -> list[str]:
    L = ["## Pairwise comparisons", ""]
    L.append(f"Every pair of conditions, none of them a baseline. A task counts as solved in a condition when at "
             f"least half of its repetitions reach F1 ≥ {thr}; McNemar's exact test runs over the tasks solved in "
             f"one condition of the pair only (first : second). Differences are first − second, averaged over "
             f"tasks with the repetitions averaged first; spend per solved task is the sum over the sessions "
             f"divided by the solved sessions, input + output tokens and the CLI cost estimate, ratio "
             f"first ÷ second. Brackets: 95 % paired bootstrap over tasks ({n_resamples:,} resamples).")
    L.append("")
    L.append("| Pair | Tasks | Solved (first : second), McNemar p | Holm p | Δ success | Δ F1 | "
             "Tokens per solved, ratio | Cost per solved, ratio |")
    L.append("|---|---:|---|---:|---:|---:|---:|---:|")
    for c in pw["pairs"]:
        s, f, tk, us = c["success"], c["f1"], c["tokens_per_solved"], c["cost_per_solved"]
        L.append(f"| `{c['a']}` vs `{c['b']}` | {c['n_tasks']} | {c['wins']} : {c['losses']}, p = "
                 f"{c['mcnemar_p']:.2g} | {c['mcnemar_p_holm']:.2g} | "
                 f"{_fmt_diff(s['diff_mean'], 'pct')} [{_fmt_diff(s['diff_lo'], 'pct')}, {_fmt_diff(s['diff_hi'], 'pct')}] | "
                 f"{_fmt_diff(f['diff_mean'], 'f3')} [{_fmt_diff(f['diff_lo'], 'f3')}, {_fmt_diff(f['diff_hi'], 'f3')}] | "
                 f"{_fmt_ratio(tk['ratio'])} [{_fmt_ratio(tk['ratio_lo'])}, {_fmt_ratio(tk['ratio_hi'])}] | "
                 f"{_fmt_ratio(us['ratio'])} [{_fmt_ratio(us['ratio_lo'])}, {_fmt_ratio(us['ratio_hi'])}] |")
    L.append("")
    L.extend(notes)
    L.append(f"**Multiplicity.** {len(pw['pairs'])} pairs × 4 primary measures (success, F1, tokens and cost per "
             f"solved task) = {4 * len(pw['pairs'])} comparisons, plus the per-category table below. Only the "
             f"success tests are corrected (Holm over the {len(pw['pairs'])} pairs); every interval is a "
             f"single 95 % interval, not corrected for multiple comparisons. The per-category numbers rest on a "
             f"few tasks each and are exploratory.")
    L.append("")
    L.append("## By category")
    L.append("")
    L.append("Means over the category's tasks (repetitions averaged first). Categories differ in how far a single "
             "text search answers them and are read separately; few tasks each, so direction, not size.")
    L.append("")
    L.append("| Category | Tasks | " + " | ".join(f"Recall {c}" for c in conditions) + " | "
             + " | ".join(f"Success {c}" for c in conditions) + " | " + " | ".join(f"F1 {c}" for c in conditions)
             + " |")
    L.append("|---|---:|" + "---:|" * (3 * len(conditions)))
    for k, m in sorted(cats.items()):
        cells = ([_fmt(m[c]["recall"], "f3") for c in conditions] + [_fmt(m[c]["success"], "pct") for c in conditions]
                 + [_fmt(m[c]["f1"], "f3") for c in conditions])
        L.append(f"| {k} | {m['n_tasks']} | " + " | ".join(cells) + " |")
    L.append("")
    return L


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
    # #182: an interleaved A/C/D run compares every pair; no condition is the baseline.
    is_pairwise = meta.get("comparisons") == "pairwise" and len(conditions) > 1
    is_edit = bool(rows) and all(r.get("edit") for r in rows)
    label_of = lambda key, lab: EDIT_LABELS.get(key, lab) if is_edit else lab.format(thr=thr)  # noqa: E731
    registered = report_commit.applies(rows, tasks) and set(conditions) == set(report_commit.ARMS)  # #183
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
    mixed = meta.get("claude_versions") or {}
    cli = (" and ".join(f"{v} ({', '.join(f'{c} {n}' for c, n in sorted(m['sessions'].items()))}; repetitions "
                         f"{', '.join(map(str, m['reps']))})" for v, m in mixed.items())
           + " (**updated during the run**)" if mixed else meta.get("claude_version"))
    L.append(f"- **Agent:** Claude Code {cli}, `--model {meta.get('model_alias')}` "
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
             + ("`Grep`, `Glob` (not in D), `Bash`, `Edit`, `Write` (+ `ToolSearch`)" if registered else
                "`Grep`, `Glob`, `Bash` (+ `ToolSearch`)" if meta.get("bash") else "`Grep`, `Glob` (+ `ToolSearch`)")
             + "; `--setting-sources project` (the working copy's "
             "`.claude/settings.json` and `CLAUDE.md` load, the user's do not); fresh working copy and index "
             "copy per session; transcripts checked for the expected tools, MCP server and subagents.")
    if rows and all(r.get("edit") for r in rows):  # #183
        L.append("- **Grading:** edit tasks; success = the task's test command passes after the session; precision / recall / F1 "
                 "compare the files the session changed with the files of the replayed commit (per kind: TOKENS.md).")
    else:
        L.append(f"- **Grading:** set-based precision / recall / F1 against gold sets derived with compiler tooling; "
                 f"success = F1 ≥ {thr}.")
    L.append(f"- **Statistics:** repetitions averaged within each task; 95 % percentile bootstrap intervals over "
             f"tasks ({n_resamples:,} resamples), "
             + ("the registered comparisons only (see there)." if registered else "every pair of conditions compared, none of them a baseline (see *Pairwise comparisons*)."
                if is_pairwise else f"paired with condition `{baseline}` for the differences."))
    L.append("")
    L.append("| Condition | What the session has |")
    L.append("|---|---|")
    for c in conditions:
        desc = OUTCOME.get(c, DESCRIPTIONS.get(c, c)) if outcome else DESCRIPTIONS.get(c, c)
        if registered:  # since #200
            desc = desc.replace("the routing block in CLAUDE.md", "the routing block in .claude/rules/myceliums.md")
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
        L.append(f"| {label_of(key, lab)} | " + " | ".join(cells) + " |")
    L.append("")

    others = [] if is_pairwise else [c for c in conditions if c != baseline]
    pw = cats = commit = None
    if registered:  # #183: the registered comparisons and no others
        is_pairwise, others = False, []
        commit_lines, commit = report_commit.lines(rows, tasks, n_resamples)
        L.extend(commit_lines)
    if is_pairwise:
        pw = pairwise(rows, tasks, conditions, n_resamples)
        cats = by_category(rows, tasks, conditions)
        L.extend(_pairwise_lines(pw, cats, conditions, thr, n_resamples, config.pair_notes(res_dir)))
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
            L.append(f"| {label_of(key, lab)} | " + " | ".join(diffs) + " | " + " | ".join(ratios) + " |")
        L.append("")

    if not is_edit:  # one type only there
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
    L.append(("File F1" if is_edit else "F1") + " per repetition, in repetition order; myceliums calls summed over the repetitions.")
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
    if commit is not None:
        summary.update(baseline=None, commit_replay=commit)
    if is_pairwise:
        summary.update(baseline=None, pairwise=pw, by_category=cats)
    return "\n".join(L) + "\n", summary
