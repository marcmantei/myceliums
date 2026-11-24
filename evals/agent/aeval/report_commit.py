"""The pre-registered analysis of the commit-replay run (#183).

Exactly what the README's *Pre-registration* lists: two co-primary outcomes
(recall over remote code, docs and forced tests; tokens per session, unsolved
ones included) for three pairs of arms, Holm over the six tests, then the
secondary outcomes and sensitivity analyses, which are not tested. When arm
``none`` is at the ceiling (:func:`decide`, one session per task, before the
main run), tokens are the sole primary outcome, Holm goes over three tests and
the recall is a harm check. Tasks are the unit: repetitions
are averaged within a task first, and tasks are resampled in pairs. Every
difference is **first minus second**: positive recall favours the first arm,
negative tokens favour it. The pilot (:func:`pilot`) is never pooled with it.
"""

from __future__ import annotations

import json
import math
import random
from dataclasses import dataclass
from pathlib import Path
from statistics import NormalDist

from . import config
from .editgrade import dependents, gold_paths, representative, touched
from .stats import mean, paired_bootstrap, per_success_bootstrap, percentile

ARMS = ("none", "C", "D")
PAIRS = (("C", "none"), ("D", "none"), ("C", "D"))
N_TASKS, N_REPS = 20, 3


@dataclass(frozen=True)
class Design:
    """The arms of a main run and its pairs (first minus second): those that are
    tested (Holm over them, times the primary outcomes) and those only described."""

    arms: tuple[str, ...]
    tested: tuple[tuple[str, str], ...]
    described: tuple[tuple[str, str], ...] = ()

    @property
    def pairs(self) -> tuple[tuple[str, str], ...]:
        return self.tested + self.described


REGISTERED = Design(ARMS, PAIRS)
# Registered with the arm `none-docs` (README): only a gain over the reminder counts as more than a reminder.
CONTROLLED = Design(("none", "none-docs", "C"), (("C", "none"), ("C", "none-docs")), (("none-docs", "none"),))
DESIGNS = {frozenset(d.arms): d for d in (REGISTERED, CONTROLLED)}


def design_of(rows: list[dict]) -> Design:
    """The registered design of these arms. A missing or foreign arm is refused:
    another design would answer another question."""
    arms = frozenset(r["condition"] for r in rows)
    if arms not in DESIGNS:
        raise SystemExit(f"the arms {', '.join(sorted(arms)) or '-'} are no registered design ({'; '.join(', '.join(d.arms) for d in DESIGNS.values())})")
    return DESIGNS[arms]


SESSIONS = N_TASKS * N_REPS * len(ARMS)
# Outcome (a): what the change needs outside the primary files' directories. ALL_TESTS adds the commit's other test
# changes: the outcome registered first (README), now a secondary one.
PRIMARY = ("docs", "remote", "forced_tests")
ALL_TESTS = (*PRIMARY, "new_tests", "tests", "optional")
KINDS = ("docs", "remote", "forced_tests", "new_tests", "optional", "local", "comments", "build", "either")
DECISION = Path(__file__).resolve().parent.parent / "groundtruth" / "decision_183.json"  # written by `agent_eval.py ceiling`
# The pilot stops the main run when it projects more than this; a mean recall of CEILING or more in `none` is a ceiling.
MAX_USD, MAX_HOURS, CEILING = 288.0, 24.0, 0.90
OVERHEAD_S = 10  # copying the working copy and the index, beside the session and the grading (mock-API probes: about 5 s)


# ---- one session -------------------------------------------------------------
def counted(task: dict, kinds: tuple[str, ...] = PRIMARY, comments: bool = False) -> list[dict]:
    """The dependents (:func:`editgrade.dependents`, the grading's own) the
    recall (a) counts: ``docs``, ``remote`` and ``forced_tests``. ``comments``
    (a sensitivity analysis) adds the code files in which only comments change
    and that would be ``remote``, not those beside a primary file."""
    return [u for u in dependents(task) if u["kind"] in kinds or comments and u["kind"] == "comments" and not u["near"]]


def by_type(task: dict) -> dict[str, int]:
    """The task's dependents by type, as registered (``groundtruth/dependents_183.json``)."""
    units = dependents(task)
    return {k: sum(u["kind"] == k for u in units) for k in KINDS}


def recall(task: dict, changed: list[str], kinds: tuple[str, ...] = PRIMARY, comments: bool = False) -> float | None:
    """Share of those dependents the session touched; None where the task has none."""
    units = counted(task, kinds, comments)
    return sum(touched(task, u, changed) for u in units) / len(units) if units else None


def precision(task: dict, changed: list[str]) -> float:
    """Share of the changed files that the commit changed; 0 when nothing changed."""
    gold = {representative(task, f) for f in gold_paths(task)}
    got = {representative(task, f) for f in changed}
    return len(gold & got) / len(got) if got else 0.0


def spend(row: dict) -> tuple[float, float, bool]:
    """``(tokens, USD, complete)`` of a session: input (cache included) plus
    output and the CLI's cost estimate. A session ended without a result event
    (timeout, crash) reports neither; the calls in its transcript do, with
    their output as a lower bound (``complete`` is False)."""
    tokens = (row.get("total_input_tokens") or 0) + (row.get("output_tokens") or 0)
    if tokens and row.get("cost_usd") is not None:
        return float(tokens), float(row["cost_usd"]), True
    return (float(tokens or sum((row.get("tokens_by_class") or {}).values())),
            float(row.get("cost_usd") or row.get("priced_cost_usd") or 0.0), False)


def wall_net(row: dict) -> float | None:
    """Wall time without the seconds the session waited for an answer that had
    stopped arriving (the audit's ``api_stall_s``)."""
    return None if row.get("wall_s") is None else row["wall_s"] - ((row.get("isolation_audit") or {}).get("api_stall_s") or 0.0)


def solved(row: dict) -> bool:
    """The test command passes after the session; nothing else."""
    return bool(row["edit"].get("success"))


# ---- tasks as the unit ---------------------------------------------------------
def per_task(rows: list[dict], fn) -> dict[str, dict[str, float]]:
    acc: dict = {}
    for r in rows:
        v = fn(r)
        if v is not None:
            acc.setdefault(r["task_id"], {}).setdefault(r["condition"], []).append(float(v))
    return {t: {c: mean(vs) for c, vs in by.items()} for t, by in acc.items()}


def pairs(values: dict, ids: list[str], a: str, b: str) -> list[tuple[float, float]]:
    return [(values[t][a], values[t][b]) for t in ids if a in values.get(t, {}) and b in values.get(t, {})]


def bootstrap_p(ps: list[tuple[float, float]], n_resamples: int = 10_000, seed: int = 0) -> float:
    """Two-sided p of the paired percentile bootstrap (the interval's
    inversion): twice the smaller share of resampled mean differences on either
    side of zero, ties on both sides, plus one resample. 1 when every
    difference is zero."""
    d = [a - b for a, b in ps]
    if not d:
        return float("nan")
    rng, k, le, ge = random.Random(seed), len(d), 0, 0
    for _ in range(n_resamples):
        s = math.fsum(d[rng.randrange(k)] for _ in range(k))
        le += s <= 1e-12
        ge += s >= -1e-12
    return min(1.0, 2 * (1 + min(le, ge)) / (n_resamples + 1))


def holm(ps: list[float]) -> list[float]:
    """Holm's adjusted p-values in the input order; a missing test (nan) stays
    one and still counts as a test."""
    order = sorted(range(len(ps)), key=lambda i: (ps[i] != ps[i], ps[i] if ps[i] == ps[i] else 0.0))
    out, running = [float("nan")] * len(ps), 0.0
    for rank, i in enumerate(order):
        if ps[i] == ps[i]:
            running = max(running, min(1.0, (len(ps) - rank) * ps[i]))
            out[i] = running
    return out


def detectable(sd: float, n: int = N_TASKS, alpha: float = 0.05, power: float = 0.8) -> float:
    """Mean paired difference a two-sided test finds with ``power`` (normal
    approximation) when the tasks' differences have standard deviation ``sd``."""
    z = NormalDist().inv_cdf
    return (z(1 - alpha / 2) + z(power)) * sd / math.sqrt(n)


def power(diff: float, sd: float, n: int = N_TASKS, alpha: float = 0.05) -> float:
    """Power of that two-sided test for a true mean paired difference ``diff``."""
    nd = NormalDist()
    z, shift = nd.inv_cdf(1 - alpha / 2), abs(diff) * math.sqrt(n) / sd if sd else float("inf") if diff else 0.0
    return nd.cdf(shift - z) + nd.cdf(-shift - z) if math.isfinite(shift) else 1.0


def sd(xs: list[float]) -> float:
    m = mean(xs)
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (len(xs) - 1)) if len(xs) > 1 else float("nan")


# ---- the registered analysis ------------------------------------------------------
SECONDARY = (  # key, label, format, value of a session
    ("recall_all_tests", "Recall with new tests (the outcome registered first)", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ALL_TESTS)),
    ("new_tests", "New tests (the commit's unforced test files) touched", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ("new_tests",))),
    ("docs", "Docs recall", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ("docs",))),
    ("remote", "Remote code recall", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ("remote",))),
    ("forced_tests", "Forced tests recall", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ("forced_tests",))),
    ("success", "Success (tests pass)", "pct", lambda t, r: float(solved(r))),
    ("precision", "Precision (changed files the commit changed)", "f3", lambda t, r: precision(t, r["edit"]["files_changed"])),
    ("api_calls", "API calls per session", "f1", lambda t, r: r.get("api_calls")),
    ("wall_s", "Wall time per session (s)", "f1", lambda t, r: r.get("wall_s")),
    ("wall_net_s", "Wall time without API stalls (s)", "f1", lambda t, r: wall_net(r)),
)
SENSITIVITY = (  # key, label, tasks kept, comment-only dependents counted
    ("strong", "without weakly tested tasks", lambda t: not t.get("weak_test"), False),
    ("comments", "with comment-only dependents", lambda t: True, True),
    ("agents_md", "only tasks whose parent has AGENTS.md", lambda t: bool((t.get("strata") or {}).get("agents_md")), False),
)


def applies(rows: list[dict], tasks: list[dict]) -> bool:
    return bool(rows) and all(r.get("edit") for r in rows) and all(t.get("primary_files") and "strata" in t for t in tasks)


def _co_primary(rows, by_id, ids, comments=False, n=10_000, design: Design = REGISTERED) -> dict:
    values = {"recall": per_task(rows, lambda r: recall(by_id[r["task_id"]], r["edit"]["files_changed"], comments=comments)),
              "tokens": per_task(rows, lambda r: spend(r)[0]), "cost": per_task(rows, lambda r: spend(r)[1])}
    out = {}
    for a, b in design.pairs:
        for key, v in values.items():
            ps = pairs(v, ids, a, b)
            out.setdefault(f"{a} vs {b}", {})[key] = {**paired_bootstrap(ps, n).as_dict(), "p": bootstrap_p(ps, n),
                                                     "sd": sd([x - y for x, y in ps])}
    return out


def harm_check(ps: list[tuple[float, float]], s: dict) -> dict:
    """The recall as a harm check of the first arm: *detected* when the 95 %
    interval of the difference lies entirely below zero; and, whatever it is,
    always two bounds on what is not excluded: the interval's lower bound on
    the mean loss (``loss``), which says little where few tasks are worse (none:
    the bootstrap has no loss to resample), and the exact one-sided 95 % bound
    on the share of tasks that are worse (``share``, :func:`share_bound`), each of
    which loses at most everything. Neither shrinks when a task gets worse; one of
    them alone, chosen by the data, would."""
    d = [a - b for a, b in ps]
    worse, better = sum(x < -1e-9 for x in d), sum(x > 1e-9 for x in d)
    if not d:
        return {"detected": False, "worse": 0, "better": 0, "n": 0, "loss": float("nan"), "share": float("nan")}
    return {"detected": s["diff_hi"] < 0, "worse": worse, "better": better, "n": len(d), "loss": max(0.0, -s["diff_lo"]),
            "share": share_bound(worse, len(d))}


def share_bound(k: int, n: int, alpha: float = 0.05) -> float:
    """Exact (Clopper-Pearson) one-sided upper bound of a share, ``k`` of ``n``
    seen: the p at which k or fewer have probability alpha. For k = 0 that is
    1 - alpha^(1/n), about 3/n."""
    if k >= n:
        return 1.0
    lo, hi = k / n, 1.0
    for _ in range(100):
        p = (lo + hi) / 2
        at_most = math.fsum(math.comb(n, i) * p ** i * (1 - p) ** (n - i) for i in range(k + 1))
        lo, hi = (p, hi) if at_most > alpha else (lo, p)
    return (lo + hi) / 2


def harm_text(h: dict) -> str:
    if not h["n"]:
        return "no task has both arms"
    return (f"harm {'detected' if h['detected'] else 'not detected'}; largest loss not excluded: {h['loss']:.3f} (lower bound of the 95 % interval) "
            f"and up to {h['share']:.1%} of tasks worse, each by at most everything (exact one-sided 95 % bound); tasks worse/better: {h['worse']}/{h['better']} of {h['n']}")


def pooled(values: dict, by_id: dict, ids: list[str], n_resamples: int = 10_000, seed: int = 0, design: Design = REGISTERED) -> dict:
    """Sensitivity: the recall over all units pooled, so that a task counts with
    its number of counted dependents (a task with one counts as much as one
    with many in the registered mean). Bootstrap over tasks as everywhere."""
    weight = {t: len(counted(by_id[t])) for t in ids}
    out: dict = {"units": sum(weight.values()), "arms": {}, "pairs": {}}
    for c in design.arms:
        have = [t for t in ids if c in values.get(t, {})]
        out["arms"][c] = sum(weight[t] * values[t][c] for t in have) / sum(weight[t] for t in have) if have else float("nan")
    for a, b in design.pairs:
        have = [(weight[t], values[t][a] - values[t][b]) for t in ids if a in values.get(t, {}) and b in values.get(t, {})]
        if not have:
            continue
        rng, draws = random.Random(seed), []
        for _ in range(n_resamples):
            pick = [have[rng.randrange(len(have))] for _ in have]
            draws.append(sum(w * d for w, d in pick) / sum(w for w, _ in pick))
        draws.sort()
        out["pairs"][f"{a} vs {b}"] = {"n_tasks": len(have), "diff_mean": sum(w * d for w, d in have) / sum(w for w, _ in have),
                                       "diff_lo": percentile(draws, 0.025), "diff_hi": percentile(draws, 0.975)}
    return out


def measured(tasks: list[dict]) -> None:
    """Every task says which of its tests the change forces; without that the recall (a) is not defined."""
    if bad := [t["id"] for t in tasks if "forced_tests" not in t]:
        raise SystemExit(f"no forced-tests record (agent_eval.py forced-tests): {', '.join(bad)}")


def decision() -> dict | None:
    return json.loads(DECISION.read_text()) if DECISION.exists() else None


def analyse(rows: list[dict], tasks: list[dict], n_resamples: int = 10_000, sole: bool = False, design: Design | None = None) -> dict:
    """``sole``: tokens are the sole primary outcome (arm ``none`` was at the
    ceiling, :func:`decide`); the recall's differences are a harm check."""
    measured(tasks)
    design = design or design_of(rows)
    by_id = {t["id"]: t for t in tasks}
    ids = [t["id"] for t in tasks]
    co = _co_primary(rows, by_id, ids, n=n_resamples, design=design)
    named = {f"{a} vs {b}" for a, b in design.tested}
    tests = [(p, k) for p in co if p in named for k in (("tokens",) if sole else ("recall", "tokens"))]  # cost is not a test
    for (p, k), adj in zip(tests, holm([co[p][k]["p"] for p, k in tests])):
        co[p][k]["p_holm"] = adj
    for p in set(co) - named:  # described, not tested
        for k in co[p]:
            co[p][k].pop("p", None)
    mine = per_task(rows, lambda r: recall(by_id[r["task_id"]], r["edit"]["files_changed"]))
    for p, s in co.items():
        s["recall"]["harm"] = harm_check(pairs(mine, ids, *p.split(" vs ")), s["recall"])
    secondary: dict = {}
    for key, _label, _fmt, fn in SECONDARY:
        v = per_task(rows, lambda r: fn(by_id[r["task_id"]], r))
        secondary[key] = {"means": {c: mean([v[t][c] for t in ids if c in v.get(t, {})]) for c in design.arms},
                          "pairs": {f"{a} vs {b}": paired_bootstrap(pairs(v, ids, a, b), n_resamples).as_dict() for a, b in design.pairs}}
    for key, i in (("tokens_per_solved", 0), ("cost_per_solved", 1)):
        total = {c: [sum(spend(r)[i] for r in rows if r["condition"] == c), sum(solved(r) for r in rows if r["condition"] == c)] for c in design.arms}
        ps = {}
        for a, b in design.pairs:
            per = [tuple(x for c in (a, b) for x in (
                sum(spend(r)[i] for r in rows if r["task_id"] == t and r["condition"] == c),
                sum(solved(r) for r in rows if r["task_id"] == t and r["condition"] == c))) for t in ids]
            ps[f"{a} vs {b}"] = per_success_bootstrap(per, n_resamples)
        secondary[key] = {"means": {c: s / k if k else float("nan") for c, (s, k) in total.items()}, "pairs": ps}
    sens = {}
    for key, _label, keep, comments in SENSITIVITY:
        sub = [t for t in ids if keep(by_id[t])]
        sens[key] = {"n_tasks": len(sub), "pairs": _co_primary([r for r in rows if r["task_id"] in sub], by_id, sub, comments, n_resamples, design)}
    count: dict = {}
    for r in rows:
        count.setdefault((r["task_id"], r["condition"]), set()).add(r["rep"])
    missing = [f"{t}/{c}: {N_REPS - len(count.get((t, c), ()))}" for t in ids for c in design.arms if len(count.get((t, c), ())) < N_REPS]
    missing += [f"{N_TASKS - len(ids)} tasks"] if len(ids) < N_TASKS else []
    means = {c: {"recall": mean([x for r in rows if r["condition"] == c
                                 and (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"])) is not None]),
                 "comments": mean([x for r in rows if r["condition"] == c
                                   and (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"], ("comments",))) is not None]),
                 "sessions": sum(r["condition"] == c for r in rows),
                 "solved": sum(solved(r) for r in rows if r["condition"] == c),
                 "incomplete_usage": sum(not spend(r)[2] for r in rows if r["condition"] == c)} for c in design.arms}
    return {"sole_primary": "tokens" if sole else None, "co_primary": co, "secondary": secondary, "pooled": pooled(mine, by_id, ids, n_resamples, design=design), "sensitivity": sens, "missing": missing, "arms": means,
            "undefined_recall": [t for t in ids if not counted(by_id[t])]}


def _num(x: float, kind: str, signed: bool = False) -> str:
    from .report import _fmt, _fmt_diff
    return _fmt_diff(x, kind) if signed else _fmt(x, kind)


def _ci(s: dict, kind: str) -> str:
    return f"{_num(s['diff_mean'], kind, True)} [{_num(s['diff_lo'], kind, True)}, {_num(s['diff_hi'], kind, True)}]"


def _ratio(s: dict, key: str = "ratio_mean", lo: str = "ratio_lo", hi: str = "ratio_hi") -> str:
    from .report import _fmt_ratio
    return f"{_fmt_ratio(s[key])} [{_fmt_ratio(s[lo])}, {_fmt_ratio(s[hi])}]"


def _p(x: float) -> str:
    return "–" if x != x else f"{x:.2g}"


def lines(rows: list[dict], tasks: list[dict], n_resamples: int = 10_000) -> tuple[list[str], dict]:
    d = decision()
    if d is None:
        raise SystemExit(f"no recorded decision ({DECISION.name}): one session of arm none per task, then `agent_eval.py ceiling LABEL --write`, "
                         "before the main run (README, decision rule)")
    if d["tasks_sha256"] != config.sha256_file(config.tasks_file()):
        raise SystemExit(f"the task file is not the one of the recorded decision (sha256 {d['tasks_sha256'][:12]})")
    sole = bool(d["ceiling"])
    other = [f"{k} {', '.join(seen)} instead of {d[k]}" for k, seen in (
        (k, sorted({str(r.get(k)) for r in rows} - {str(d[k])})) for k in ("claude_version", "model")) if seen]
    design = design_of(rows)
    a = analyse(rows, tasks, n_resamples, sole, design)
    numbers = {2: "two", 3: "three", 4: "four", 6: "six"}
    k = len(design.tested)
    which = "three pairs" if not design.described else "two pairs: " + ", ".join(f"`{x}` vs `{y}`" for x, y in design.tested)
    L = ["## Registered comparisons (#183)", "",
         f"Pre-registered in the README; the recall was redefined after the pilot in arm `none` and before any result of C or D. Unit: the task ({len(tasks)}), its repetitions averaged first; paired percentile "
         f"bootstrap over tasks ({n_resamples:,} resamples), 95 % intervals not corrected. Every difference is **first minus second**: "
         "a positive recall difference favours the first arm, a negative token difference favours it (it spends less). *Recall*: share "
         "of the dependents (docs, code outside the primary files' directories, tests the change forces; not new tests and not files in which only comments or documentation text change) "
         "the session touched. *Tokens*: input including cache plus output per session, unsolved sessions included. p: two-sided, the bootstrap's. Cost is shown beside the tokens and not tested.", "",
         f"The decision recorded before the main run: {d['sessions']} sessions of arm `none` (Claude Code {d['claude_version']}, {d['model']}), mean recall {d['mean_recall']:.3f}, "
         f"ceiling {CEILING:.3f}; task file sha256 {d['tasks_sha256'][:12]}. "
         + (f"**Primary outcome: tokens alone.** Holm over the {numbers[k]} token tests; the recall is a harm check: *detected* when its 95 % interval lies entirely below zero; not detected is no evidence "
            "that there is none, so the largest loss not excluded stands beside it." if sole else
            f"**Co-primary outcomes: recall and tokens.** Holm over the {numbers[2 * k]} tests ({which} × two outcomes).")
         + (" " + ", ".join(f"`{x}` vs `{y}`" for x, y in design.described) + " is described, not tested. **Only a gain of C over `none-docs` counts as more than a reminder**; "
            "a gain over `none` alone does not." if design.described else "")]
    if other:
        L += ["", f"**Not the decision's setup:** {'; '.join(other)}."]
    if a["missing"]:
        L += ["", f"**Incomplete run:** missing sessions (task/arm: number) {', '.join(a['missing'])}. A pair uses the tasks both arms have."]
    if a["undefined_recall"]:
        L += ["", f"No counted dependent, so no recall: {', '.join(a['undefined_recall'])}."]
    L += ["", "| Arm | Sessions | Solved (tests pass) | Recall, mean over sessions | Sessions without a result event |", "|---|---:|---:|---:|---:|"]
    for c in design.arms:
        m = a["arms"][c]
        L.append(f"| `{c}` | {m['sessions']} | {m['solved']} | {_num(m['recall'], 'f3')} | {m['incomplete_usage']} |")
    L += ["", "Files in which only comments or documentation text change, apart (share of them touched, mean over sessions of tasks that have any): "
          + "; ".join(f"`{c}` {_num(a['arms'][c]['comments'], 'f3')}" for c in design.arms) + ".",
          "", "A session without a result event (timeout, crash) counts with the calls its transcript shows; its output tokens are a lower bound.",
          "", "| Pair | Tasks (recall) | Δ recall | p | Holm p | Tokens, ratio | Δ tokens | p | Holm p | Δ cost (USD) |",
          "|---|---:|---|---:|---:|---|---|---:|---:|---|"]
    for p, s in a["co_primary"].items():
        x, y = p.split(" vs ")
        told = lambda o: f"{_p(o['p'])} | {_p(o['p_holm'])}" if "p_holm" in o else "– | –"  # noqa: E731
        L.append(f"| `{x}` vs `{y}` | {s['recall']['n_tasks']} | {_ci(s['recall'], 'f3')} | {told(s['recall'])} | "
                 f"{_ratio(s['tokens'])} | {_ci(s['tokens'], 'int')} | {told(s['tokens'])} | {_ci(s['cost'], 'usd')} |")
    if sole:
        L += ["", "Harm check of the first arm (recall, not a test):", ""] + [
            f"* `{p}`: {harm_text(s['recall']['harm'])}." for p, s in a["co_primary"].items()]
    L += ["", "### Secondary outcomes", "", "Not tested and not corrected. Per solved task: the sum over all sessions ÷ the solved sessions, ratio first ÷ second; "
          "– where an arm solved nothing.", "",
          "| Outcome | " + " | ".join(f"`{c}`" for c in design.arms) + " | " + " | ".join(a["co_primary"]) + " |",
          "|---|" + "---:|" * len(design.arms) + "---|" * len(design.pairs)]
    for key, label, kind, _fn in SECONDARY:
        s = a["secondary"][key]
        L.append(f"| {label} | " + " | ".join(_num(s["means"][c], kind) for c in design.arms) + " | " + " | ".join(_ci(s["pairs"][p], kind) for p in a["co_primary"]) + " |")
    for key, label, kind in (("tokens_per_solved", "Tokens per solved task", "int"), ("cost_per_solved", "Cost per solved task (USD)", "usd")):
        s = a["secondary"][key]
        L.append(f"| {label} | " + " | ".join(_num(s["means"][c], kind) for c in design.arms) + " | "
                 + " | ".join(_ratio(s["pairs"][p], "ratio") for p in a["co_primary"]) + " |")
    L += ["", "### Sensitivity", "", "The co-primary differences again, not tested.", "",
          "| Analysis | Tasks | " + " | ".join(f"Δ recall, {p}" for p in a["co_primary"]) + " | " + " | ".join(f"Tokens, {p}" for p in a["co_primary"]) + " |",
          "|---|---:|" + "---|" * (2 * len(design.pairs))]
    for key, label, _keep, _c in SENSITIVITY:
        s = a["sensitivity"][key]
        L.append(f"| {label} | {s['n_tasks']} | " + " | ".join(_ci(s["pairs"][p]["recall"], "f3") for p in a["co_primary"]) + " | "
                 + " | ".join(_ratio(s["pairs"][p]["tokens"]) for p in a["co_primary"]) + " |")
    pool = a["pooled"]
    L.append(f"| units pooled ({pool['units']} dependents, a task weighs as many as it has) | {len(tasks)} | "
             + " | ".join(_ci(pool["pairs"][p], "f3") if p in pool["pairs"] else "–" for p in a["co_primary"]) + " | " + " | ".join("–" for _ in design.pairs) + " |")
    L += ["", "Pooled recall: " + "; ".join(f"`{c}` {_num(pool['arms'][c], 'f3')}" for c in design.arms) + "."]
    return L + [""], a


# ---- the pilot --------------------------------------------------------------------
def pilot(rows: list[dict], tasks: list[dict], drawn: list[str]) -> dict:
    """What the pilot may say: cost and time of the main run projected from arm
    ``none``, and whether ``none`` is already at the ceiling. Sessions of other
    arms are refused: their outcomes are not looked at."""
    if bad := sorted({r["condition"] for r in rows} - {"none"}):
        raise SystemExit(f"the pilot's report covers arm none only, the run has {bad}")
    if sorted(r["task_id"] for r in rows) != sorted(drawn):
        raise SystemExit(f"the pilot is one session of each drawn task ({', '.join(drawn)})")
    by_id = {t["id"]: t for t in tasks}
    measured([by_id[i] for i in drawn])
    each = lambda kinds: [x for r in rows if (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"], kinds)) is not None]  # noqa: E731
    recalls, every = each(PRIMARY), each(ALL_TESTS)
    usd = mean([spend(r)[1] for r in rows]) * SESSIONS
    hours = mean([r["wall_s"] + ((r["edit"].get("test") or {}).get("seconds") or 0) + OVERHEAD_S for r in rows]) * SESSIONS / 3600
    s = sd(recalls)
    stop = ([f"projected cost ${usd:.0f} > ${MAX_USD:.0f}"] if usd > MAX_USD else []) + (
        [f"projected time {hours:.1f} h > {MAX_HOURS:.0f} h"] if hours > MAX_HOURS else [])
    return {"sessions": len(rows), "solved": sum(solved(r) for r in rows), "mean_recall": mean(recalls), "sd_recall_between_tasks": s, "mean_recall_all_tests": mean(every),
            "wall_s_per_session": mean([r["wall_s"] for r in rows]), "wall_net_s_per_session": mean([wall_net(r) for r in rows]),
            "tokens_per_session": mean([spend(r)[0] for r in rows]), "projected_usd": usd, "projected_hours": hours,
            "projected_hours_without_stalls": hours - mean([r["wall_s"] - wall_net(r) for r in rows]) * SESSIONS / 3600,
            "detectable_recall": {"one test": detectable(s), "Bonferroni, three pairs": detectable(s, alpha=0.05 / 3),
                                  "first Holm step, six tests": detectable(s, alpha=0.05 / 6)},
            "ceiling": mean(recalls) >= CEILING, "stop": stop}


def decide(rows: list[dict], tasks: list[dict], tasks_sha256: str) -> dict:
    """The registered decision before the main run: one final session of arm
    ``none`` per task (a timeout is final and counts), not pooled with anything.
    An unrounded mean recall (a) of CEILING - 1e-9 or more makes tokens the sole
    primary outcome; otherwise both stay co-primary. All sessions are of one
    Claude Code and one model, which the record names, with the task file's hash."""
    if bad := sorted({r["condition"] for r in rows} - {"none"}):
        raise SystemExit(f"the ceiling check covers arm none only, the run has {bad}")
    ids = sorted(t["id"] for t in tasks)
    if len(ids) != N_TASKS or sorted(r["task_id"] for r in rows) != ids:
        raise SystemExit(f"the ceiling check is exactly one session of each of the {N_TASKS} tasks")
    if bad := sorted(r["task_id"] for r in rows if not r.get("final") or r.get("rep") != 1):
        raise SystemExit(f"not a final session of repetition 1: {', '.join(bad)}")
    one = {}
    for k in ("claude_version", "model"):
        seen = sorted({str(r.get(k)) for r in rows})
        if len(seen) != 1 or seen == ["None"]:
            raise SystemExit(f"the sessions are not of one known {k}: {', '.join(seen)}")
        one[k] = seen[0]
    measured(tasks)
    by_id = {t["id"]: t for t in tasks}
    if bad := [t["id"] for t in tasks if not counted(t)]:
        raise SystemExit(f"no counted dependent: {', '.join(bad)}")
    per = {r["task_id"]: recall(by_id[r["task_id"]], r["edit"]["files_changed"]) for r in sorted(rows, key=lambda r: r["task_id"])}
    values = list(per.values())
    weight = {t["id"]: len(counted(t)) for t in tasks}
    every = [x for r in rows if (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"], ALL_TESTS)) is not None]
    ceiling = mean(values) >= CEILING - 1e-9
    return {"sessions": len(rows), "arm": "none", "solved": sum(solved(r) for r in rows), "mean_recall": mean(values), "shown": f"{mean(values):.3f}",
            "statuses": {s: sum(r.get("status") == s for r in rows) for s in sorted({str(r.get("status")) for r in rows})}, **one, "tasks_sha256": tasks_sha256,
            "per_task": per, "pooled_recall": sum(weight[t] * per[t] for t in per) / sum(weight.values()), "mean_recall_all_tests": mean(every), "threshold": CEILING, "ceiling": ceiling,
            "primary": ["tokens"] if ceiling else ["recall", "tokens"]}


def describe(rows: list[dict], tasks: list[dict], paired_tokens_rel_sd: dict[str, float] | None = None) -> dict:
    """What the sessions of one arm show beside the decision: recall by type
    with every missed unit, tokens, cost, wall time with and without API stalls,
    and the main run projected from them. There is no paired data on the recall.
    ``recall_noise_only`` is a scenario, not a bound and not power: the paired SD
    of two arms' 3-repetition means if the whole between-task SD were session
    noise (sqrt(2/3) of it) and no task favoured an arm. The between-task SD
    itself is not the paired SD: the part of it that is the task cancels in a
    pair, while an arm that helps in some tasks only adds to it.
    ``closing_docs_misses`` is such an arm, without any noise: it touches every
    docs unit missed here and nothing else changes; its gain, the SD of its gains
    over the tasks and the power that leaves.
    ``paired_tokens_rel_sd``: paired SDs of another run, as a share of its mean."""
    by_id = {t["id"]: t for t in tasks}
    rows = sorted(rows, key=lambda r: r["task_id"])
    n, per = len(rows), {}
    for r in rows:
        t, changed = by_id[r["task_id"]], r["edit"]["files_changed"]
        missed = [{"kind": u["kind"], "files": u["files"]} for u in counted(t) if not touched(t, u, changed)]
        a = r.get("isolation_audit") or {}
        per[r["task_id"]] = {
            "status": r["status"], "solved": solved(r), "recall": recall(t, changed), "recall_all_tests": recall(t, changed, ALL_TESTS),
            **{k: f"{sum(touched(t, u, changed) for u in counted(t, (k,)))}/{len(counted(t, (k,)))}" for k in (*PRIMARY, "new_tests", "optional")},
            "missed": missed, "tokens": spend(r)[0], "usd": spend(r)[1], "complete_usage": spend(r)[2], "api_calls": r.get("api_calls"),
            "wall_s": r["wall_s"], "wall_net_s": round(wall_net(r), 2), "grading_s": (r["edit"].get("test") or {}).get("seconds"),
            **{k: a.get(k, 0) for k in ("refused_calls", "claude_refusals", "claude_refusals_cd", "api_stalls", "api_stall_s", "disagreements", "kernel_refusals")}}
    values = [p["recall"] for p in per.values()]
    kinds = {}
    for k in PRIMARY:
        have = [(len(u), sum(touched(by_id[r["task_id"]], x, r["edit"]["files_changed"]) for x in u)) for r in rows if (u := counted(by_id[r["task_id"]], (k,)))]
        kinds[k] = {"units": sum(x for x, _ in have), "touched": sum(y for _, y in have), "tasks": len(have), "mean_over_tasks": mean([y / x for x, y in have])}
    spread, noise = sd(values), math.sqrt(2 / 3) * sd(values)
    gains = [sum(m["kind"] == "docs" for m in p["missed"]) / len(counted(by_id[t])) for t, p in per.items()]
    levels = {"one test": 0.05, "first Holm step, three tests": 0.05 / 3, "first Holm step, six tests": 0.05 / 6}
    usd = [p["usd"] for p in per.values()]
    secs = {k: [p[k] + (p["grading_s"] or 0) + OVERHEAD_S for p in per.values()] for k in ("wall_s", "wall_net_s")}
    out = {"sessions": n, "solved": sum(p["solved"] for p in per.values()), "mean_recall": mean(values), "sd_recall_between_tasks": spread,
           "tasks_at_one": sum(v == 1 for v in values), "pooled_recall": sum(k["touched"] for k in kinds.values()) / sum(k["units"] for k in kinds.values()),
           "by_type": kinds, "recall_all_tests": mean([p["recall_all_tests"] for p in per.values()]),
           "tokens_total": sum(p["tokens"] for p in per.values()), "tokens_mean": mean([p["tokens"] for p in per.values()]),
           "tokens_sd_between_tasks": sd([p["tokens"] for p in per.values()]), "usd_total": sum(usd), "usd_mean": mean(usd),
           "wall_s_mean": mean([p["wall_s"] for p in per.values()]), "wall_net_s_mean": mean([p["wall_net_s"] for p in per.values()]),
           "main_run": {"sessions": SESSIONS, "usd": mean(usd) * SESSIONS, "usd_95": _boot(usd, SESSIONS),
                        "hours": mean(secs["wall_s"]) * SESSIONS / 3600, "hours_95": _boot(secs["wall_s"], SESSIONS / 3600),
                        "hours_without_stalls": mean(secs["wall_net_s"]) * SESSIONS / 3600},
           "recall_noise_only": {"paired_sd": noise, **{k: detectable(noise, alpha=a) for k, a in levels.items()}},
           "closing_docs_misses": {"gain": mean(gains), "sd_over_tasks": sd(gains), "power": {k: power(mean(gains), sd(gains), alpha=a) for k, a in levels.items()}},
           "tasks": per}
    if paired_tokens_rel_sd:
        out["detectable_tokens_share_of_mean"] = {pair: {"paired_rel_sd": s, **{k: detectable(s, alpha=a) for k, a in levels.items()}}
                                                  for pair, s in paired_tokens_rel_sd.items()}
    return out


def _boot(xs: list[float], scale: float, n_resamples: int = 10_000, seed: int = 0) -> list[float]:
    rng = random.Random(seed)
    draws = sorted(math.fsum(xs[rng.randrange(len(xs))] for _ in xs) / len(xs) * scale for _ in range(n_resamples))
    return [percentile(draws, 0.025), percentile(draws, 0.975)]


def paired_token_sds(rows: list[dict]) -> dict[str, float]:
    """The SD over tasks of the paired differences of mean tokens, as a share of
    the second arm's mean, in a finished run with these arms (#182)."""
    values, ids = per_task(rows, lambda r: spend(r)[0]), sorted({r["task_id"] for r in rows})
    out = {}
    for a, b in PAIRS:
        if ps := pairs(values, ids, a, b):
            out[f"{a} vs {b}"] = sd([x - y for x, y in ps]) / mean([y for _, y in ps])
    return out
