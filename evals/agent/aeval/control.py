"""The control arm `none-docs` against the 20 recorded sessions of arm `none` (#183).

One session per task of each arm, paired by task. **Descriptive**: nothing is
tested, and the two sets ran at different times, so a difference holds the
sentence's effect, session noise and whatever changed in between (*drift*; the
report gives both time spans). What was registered before the first session
(README, *Control arm*): the comparison below and the rule that reads it
(:func:`outcome`).
"""

from __future__ import annotations

import random

from . import config
from .editgrade import file_kind, gold_paths, touched
from .report_commit import ALL_TESTS, N_TASKS, PRIMARY, counted, measured, recall, solved, spend, wall_net
from .stats import mean, paired_bootstrap, percentile

BASE, ARM = "none", "none-docs"
MOST = 0.5  # more than this share of the docs gap closed is "most of it"
KINDS = ("docs", "tests", "code")


def _one_per_task(rows: list[dict], tasks: list[dict], arm: str) -> dict[str, dict]:
    if bad := sorted({r["condition"] for r in rows} - {arm}):
        raise SystemExit(f"these are the sessions of arm {arm} only, found {bad}")
    ids = sorted(t["id"] for t in tasks)
    if len(ids) != N_TASKS or sorted(r["task_id"] for r in rows) != ids:
        raise SystemExit(f"arm {arm}: exactly one session of each of the {N_TASKS} tasks")
    if bad := sorted(r["task_id"] for r in rows if not r.get("final", True) or r.get("rep") != 1):
        raise SystemExit(f"arm {arm}: not a final session of repetition 1: {', '.join(bad)}")
    return {r["task_id"]: r for r in rows}


def same_setup(base: dict[str, dict], new: dict[str, dict]) -> dict[str, str]:
    """One Claude Code and one model, the same in both sets."""
    out = {}
    for k in ("claude_version", "model"):
        seen = sorted({str(r.get(k)) for r in (*base.values(), *new.values())})
        if len(seen) != 1 or seen == ["None"]:
            raise SystemExit(f"the two sets are not of one {k}: {', '.join(seen)}")
        out[k] = seen[0]
    return out


def extra_files(task: dict, changed: list[str]) -> dict[str, list[str]]:
    """Over-editing: files changed outside the commit's files, by type
    (:func:`editgrade.file_kind`). A place equivalent to a gold file is in the gold."""
    gold = set(gold_paths(task)) | {f for g in task.get("equivalents") or [] for f in g}
    out: dict[str, list[str]] = {k: [] for k in KINDS}
    for f in sorted(set(changed) - gold):
        out[file_kind(f)].append(f)
    return out


def _gap(task: dict, changed: list[str]) -> dict[str, float]:
    """What a session lacks to (a) = 1, by type."""
    units = counted(task)
    return {k: sum(u["kind"] == k and not touched(task, u, changed) for u in units) / len(units) for k in PRIMARY}


def compare(base_rows: list[dict], new_rows: list[dict], tasks: list[dict], n_resamples: int = 10_000) -> dict:
    measured(tasks)
    base, new = _one_per_task(base_rows, tasks, BASE), _one_per_task(new_rows, tasks, ARM)
    setup = same_setup(base, new)
    by_id = {t["id"]: t for t in tasks}
    ids = sorted(by_id)
    arms = {BASE: base, ARM: new}
    files = {a: {i: rows[i]["edit"]["files_changed"] for i in ids} for a, rows in arms.items()}
    gaps = {a: {i: _gap(by_id[i], files[a][i]) for i in ids} for a in arms}
    extra = {a: {i: extra_files(by_id[i], files[a][i]) for i in ids} for a in arms}

    def pair(fn) -> dict:
        ps = [(float(fn(ARM, i)), float(fn(BASE, i))) for i in ids]
        return paired_bootstrap(ps, n_resamples).as_dict()

    per = {}
    for i in ids:
        t = by_id[i]
        hit = {a: {u["path"] for u in counted(t) if touched(t, u, files[a][i])} for a in arms}
        missed = {a: [u["path"] for u in counted(t) if u["path"] not in hit[a]] for a in arms}
        kind = {u["path"]: u["kind"] for u in counted(t)}
        per[i] = {"recall": {a: recall(t, files[a][i]) for a in arms},
                  **{k: {a: f"{sum(kind[p] == k for p in hit[a])}/{sum(v == k for v in kind.values())}" for a in arms} for k in PRIMARY},
                  "found_now": [[kind[p], p] for p in missed[BASE] if p in hit[ARM]],
                  "missed_by_both": [[kind[p], p] for p in missed[BASE] if p not in hit[ARM]],
                  "missed_now_only": [[kind[p], p] for p in missed[ARM] if p in hit[BASE]],
                  "tokens": {a: spend(arms[a][i])[0] for a in arms}, "usd": {a: spend(arms[a][i])[1] for a in arms},
                  "solved": {a: solved(arms[a][i]) for a in arms}, "status": {a: arms[a][i].get("status") for a in arms},
                  "extra": {a: {k: len(v) for k, v in extra[a][i].items()} for a in arms}, "extra_files": extra[ARM][i]}

    def units(a: str, k: str) -> dict:
        have = [(counted(by_id[i], (k,)), files[a][i], by_id[i]) for i in ids]
        have = [(u, f, t) for u, f, t in have if u]
        hits = [sum(touched(t, x, f) for x in u) for u, f, t in have]
        return {"units": sum(len(u) for u, _, _ in have), "touched": sum(hits), "tasks": len(have), "tasks_all_touched": sum(h == len(u) for h, (u, _, _) in zip(hits, have)),
                "tasks_any_touched": sum(h > 0 for h in hits)}

    summary: dict = {}
    for a, rows in arms.items():
        g = {k: mean([gaps[a][i][k] for i in ids]) for k in PRIMARY}
        audit = [r.get("isolation_audit") or {} for r in rows.values()]
        summary[a] = {"mean_recall": mean([per[i]["recall"][a] for i in ids]), "gap": g, "by_type": {k: units(a, k) for k in PRIMARY},
                      "recall_all_tests": mean([recall(by_id[i], files[a][i], ALL_TESTS) for i in ids]),
                      "new_tests_touched": sum(touched(by_id[i], u, files[a][i]) for i in ids for u in counted(by_id[i], ("new_tests",))),
                      "optional_touched": sum(touched(by_id[i], u, files[a][i]) for i in ids for u in counted(by_id[i], ("optional",))),
                      "solved": sum(solved(r) for r in rows.values()), "tokens_total": sum(spend(r)[0] for r in rows.values()),
                      "usd_total": sum(spend(r)[1] for r in rows.values()), "wall_s_mean": mean([r["wall_s"] for r in rows.values()]),
                      "wall_net_s_mean": mean([wall_net(r) for r in rows.values()]), "api_calls_mean": mean([r.get("api_calls") or 0 for r in rows.values()]),
                      "extra": {k: {"files": sum(len(extra[a][i][k]) for i in ids), "sessions": sum(bool(extra[a][i][k]) for i in ids)} for k in KINDS},
                      "files_changed": sum(len(files[a][i]) for i in ids),
                      "refused_by_hook": sum(x.get("refused_calls", 0) for x in audit), "refused_by_claude": sum(x.get("claude_refusals", 0) for x in audit),
                      "refused_by_claude_cd": sum(x.get("claude_refusals_cd", 0) for x in audit), "disagreements": sum(x.get("disagreements", 0) for x in audit),
                      "span": [min(r["finished_at"] for r in rows.values()), max(r["finished_at"] for r in rows.values())]}
    closed = {k: 1 - summary[ARM]["gap"][k] / summary[BASE]["gap"][k] if summary[BASE]["gap"][k] else None for k in PRIMARY}
    out = {"arms": [BASE, ARM], **setup, "sentence": config.DOCS_SENTENCE, "summary": summary, "gap_closed": closed,
           "difference": {"recall": pair(lambda a, i: per[i]["recall"][a]), "docs_gap": pair(lambda a, i: gaps[a][i]["docs"]),
                          "forced_tests_gap": pair(lambda a, i: gaps[a][i]["forced_tests"]), "tokens": pair(lambda a, i: per[i]["tokens"][a]),
                          "usd": pair(lambda a, i: per[i]["usd"][a]), "extra_files": pair(lambda a, i: sum(per[i]["extra"][a].values()))},
           "docs_gap_closed_95": _share_interval([(gaps[BASE][i]["docs"], gaps[ARM][i]["docs"]) for i in ids], n_resamples),
           "tasks": per}
    out["outcome"] = outcome(closed["docs"])
    return out


def _share_interval(ps: list[tuple[float, float]], n_resamples: int, seed: int = 0) -> list[float]:
    """Bootstrap over tasks of the share of the docs gap closed (resamples without a docs gap are left out)."""
    rng, draws = random.Random(seed), []
    for _ in range(n_resamples):
        pick = [ps[rng.randrange(len(ps))] for _ in ps]
        if lack := sum(b for b, _ in pick):
            draws.append(1 - sum(n for _, n in pick) / lack)
    draws.sort()
    return [percentile(draws, 0.025), percentile(draws, 0.975)]


def outcome(docs_gap_closed: float | None) -> str:
    """``i``: the sentence closes most of the docs gap (more than :data:`MOST`
    of it), ``ii``: it does not (README, *Control arm*)."""
    return "i" if docs_gap_closed is not None and docs_gap_closed > MOST + 1e-9 else "ii"


MEANING = {
    "i": "**Outcome (i): the sentence closes most of the docs gap.** A reminder suffices for this failure; what is left of it is too little to look for with myceliums on these tasks.",
    "ii": "**Outcome (ii): the sentence does not close most of the docs gap.** The registered next step is a main run with the arms `none`, `none-docs` and C (no D), "
          "20 tasks x 3 repetitions, Holm over four tests (C vs `none`, C vs `none-docs`; recall and tokens), `none-docs` vs `none` described. Only a gain over `none-docs` counts as more than a reminder.",
}


def lines(c: dict) -> list[str]:
    b, n = c["summary"][BASE], c["summary"][ARM]
    f3, ci = (lambda x: f"{x:.3f}"), (lambda s, f: f"{f(s['diff_mean'])} [{f(s['diff_lo'])}, {f(s['diff_hi'])}]")
    sign = lambda x: f"{x:+.3f}"  # noqa: E731
    L = ["## Control arm `none-docs` against the recorded sessions of arm `none` (#183)", "",
         f"One session per task and arm, paired by task, descriptive: **nothing is tested**, intervals are 95 % bootstrap over the {N_TASKS} tasks. Claude Code {c['claude_version']}, {c['model']} in both. "
         f"`none-docs` has one more sentence in the prompt: \"{c['sentence']}\" "
         f"The sets ran at different times (`none` finished {b['span'][0][:16]} to {b['span'][1][:16]}, `none-docs` {n['span'][0][:16]} to {n['span'][1][:16]} UTC): "
         "a difference also holds session noise and drift (the model's service, the machine's load), which cannot be told from the sentence.", "",
         MEANING[c["outcome"]], "",
         f"Share of the docs gap closed: {'–' if c['gap_closed']['docs'] is None else format(c['gap_closed']['docs'], '.2f')} "
         f"[{c['docs_gap_closed_95'][0]:.2f}, {c['docs_gap_closed_95'][1]:.2f}] (more than {MOST:.2f} is \"most\")."
         + (f" The interval contains {MOST:.2f}: the rule decides by the point, the data do not." if c["docs_gap_closed_95"][0] <= MOST <= c["docs_gap_closed_95"][1] else ""), "",
         "| | `none` | `none-docs` | Difference |", "|---|---:|---:|---|",
         f"| Mean recall (a) | {f3(b['mean_recall'])} | {f3(n['mean_recall'])} | {ci(c['difference']['recall'], sign)} |"]
    for k in PRIMARY:
        x, y = b["by_type"][k], n["by_type"][k]
        L.append(f"| {k.replace('_', ' ')}: units touched | {x['touched']}/{x['units']} | {y['touched']}/{y['units']} | gap of the mean {f3(b['gap'][k])} → {f3(n['gap'][k])} |")
    x, y = b["by_type"]["docs"], n["by_type"]["docs"]
    L += [f"| tasks with docs: all / any touched | {x['tasks_all_touched']} / {x['tasks_any_touched']} of {x['tasks']} | {y['tasks_all_touched']} / {y['tasks_any_touched']} of {y['tasks']} | |",
          f"| Recall with new tests | {f3(b['recall_all_tests'])} | {f3(n['recall_all_tests'])} | |",
          f"| Solved | {b['solved']} | {n['solved']} | |",
          f"| Tokens, total | {b['tokens_total']:,.0f} | {n['tokens_total']:,.0f} | {c['difference']['tokens']['ratio_mean']:.2f}x [{c['difference']['tokens']['ratio_lo']:.2f}x, {c['difference']['tokens']['ratio_hi']:.2f}x] |",
          f"| Cost, total (USD) | {b['usd_total']:.2f} | {n['usd_total']:.2f} | |",
          f"| Wall time, mean s (without stalls) | {b['wall_s_mean']:.0f} ({b['wall_net_s_mean']:.0f}) | {n['wall_s_mean']:.0f} ({n['wall_net_s_mean']:.0f}) | |",
          f"| Files changed | {b['files_changed']} | {n['files_changed']} | |"]
    for k in KINDS:
        L.append(f"| Over-editing: {k} files outside the commit (sessions with any) | {b['extra'][k]['files']} ({b['extra'][k]['sessions']}) | {n['extra'][k]['files']} ({n['extra'][k]['sessions']}) | |")
    L += [f"| In the commit but not needed: new tests / optional touched | {b['new_tests_touched']} / {b['optional_touched']} | {n['new_tests_touched']} / {n['optional_touched']} | |",
          f"| Calls refused by the hook / by Claude Code (with `cd`) | {b['refused_by_hook']} / {b['refused_by_claude']} ({b['refused_by_claude_cd']}) | {n['refused_by_hook']} / {n['refused_by_claude']} ({n['refused_by_claude_cd']}) | |",
          "", "| Task | (a) `none` | (a) `none-docs` | Docs | Forced tests | Remote | Found now | Missed by both | Missed now only | Tokens | Extra files (docs/tests/code) |",
          "|---|---:|---:|---|---|---|---|---|---|---|---|"]
    name = lambda xs: ", ".join(f"`{p.rsplit('/', 1)[-1]}`" for _, p in xs) or "–"  # noqa: E731
    for i, t in c["tasks"].items():
        L.append(f"| `{i}` | {t['recall'][BASE]:.2f} | {t['recall'][ARM]:.2f} | {t['docs'][BASE]} → {t['docs'][ARM]} | {t['forced_tests'][BASE]} → {t['forced_tests'][ARM]} | {t['remote'][BASE]} → {t['remote'][ARM]} | "
                 f"{name(t['found_now'])} | {name(t['missed_by_both'])} | {name(t['missed_now_only'])} | {t['tokens'][BASE]:,.0f} → {t['tokens'][ARM]:,.0f} | "
                 f"{'/'.join(str(t['extra'][BASE][k]) for k in KINDS)} → {'/'.join(str(t['extra'][ARM][k]) for k in KINDS)} |")
    return L + [""]
