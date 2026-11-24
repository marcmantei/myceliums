"""The pre-registered analysis of the commit-replay run (#183).

Exactly what the README's *Pre-registration* lists: two co-primary outcomes
(recall over the dependents; tokens per session, unsolved ones included) for
three pairs of arms, Holm over the six tests, then the secondary outcomes and
sensitivity analyses, which are not tested. Tasks are the unit: repetitions
are averaged within a task first, and tasks are resampled in pairs. Every
difference is **first minus second**: positive recall favours the first arm,
negative tokens favour it. The pilot (:func:`pilot`) is never pooled with it.
"""

from __future__ import annotations

import math
import random
from statistics import NormalDist

from .editgrade import file_kind, gold_paths, representative
from .stats import mean, paired_bootstrap, per_success_bootstrap

ARMS = ("none", "C", "D")
PAIRS = (("C", "none"), ("D", "none"), ("C", "D"))
N_TASKS, N_REPS = 20, 3
SESSIONS = N_TASKS * N_REPS * len(ARMS)
# The pilot stops the main run when it projects more than this, and calls a recall at or above CEILING a ceiling.
MAX_USD, MAX_HOURS, CEILING = 288.0, 24.0, 0.90
OVERHEAD_S = 10  # copying the working copy and the index, beside the session and the grading (mock-API probes: about 5 s)


# ---- one session -------------------------------------------------------------
def dependents(task: dict, comments: bool = True) -> list[tuple[str, str]]:
    """``(gold file, kind)`` of the dependents the co-primary recall counts:
    gold files other than the primary ones that are ``docs``, ``tests`` or
    ``remote`` (code outside the primary files' directories). Not counted:
    code beside a primary file, build scripts, and files whose part can go
    into a primary file (``either``). ``comments=False`` also leaves out code
    files in which only comments change."""
    primary = set(task.get("primary_files") or [])
    dirs = {p.rpartition("/")[0] for p in primary}
    either = set(task.get("either") or [])
    only = {g["path"] for g in task.get("gold_files") or [] if isinstance(g, dict) and g.get("comment_only")}
    out = []
    for f in sorted(set(gold_paths(task)) - primary - either):
        kind = file_kind(f)
        if kind == "code":
            if f.rpartition("/")[0] in dirs or f.endswith((".gradle", ".gradle.kts")):
                continue
            if not comments and f in only:
                continue
            kind = "remote"
        out.append((f, kind))
    return out


def recall(task: dict, changed: list[str], kinds: tuple[str, ...] = ("docs", "tests", "remote"), comments: bool = True) -> float | None:
    """Share of those dependents the session touched (equivalent places count
    as one, as in the grading); None where the task has none."""
    deps = [f for f, k in dependents(task, comments) if k in kinds]
    touched = {representative(task, f) for f in changed}
    return sum(representative(task, f) in touched for f in deps) / len(deps) if deps else None


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


def sd(xs: list[float]) -> float:
    m = mean(xs)
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (len(xs) - 1)) if len(xs) > 1 else float("nan")


# ---- the registered analysis ------------------------------------------------------
SECONDARY = (  # key, label, format, value of a session
    ("docs", "Docs recall", "f3", lambda t, r: recall(t, r["edit"]["files_changed"], ("docs",))),
    ("success", "Success (tests pass)", "pct", lambda t, r: float(solved(r))),
    ("precision", "Precision (changed files the commit changed)", "f3", lambda t, r: precision(t, r["edit"]["files_changed"])),
    ("api_calls", "API calls per session", "f1", lambda t, r: r.get("api_calls")),
    ("wall_s", "Wall time per session (s)", "f1", lambda t, r: r.get("wall_s")),
)
SENSITIVITY = (  # key, label, tasks kept, comment-only dependents counted
    ("strong", "without weakly tested tasks", lambda t: not t.get("weak_test"), True),
    ("no_comments", "without comment-only dependents", lambda t: True, False),
    ("agents_md", "only tasks whose parent has AGENTS.md", lambda t: bool((t.get("strata") or {}).get("agents_md")), True),
)


def applies(rows: list[dict], tasks: list[dict]) -> bool:
    return bool(rows) and all(r.get("edit") for r in rows) and all(t.get("primary_files") and "strata" in t for t in tasks)


def _co_primary(rows, by_id, ids, comments=True, n=10_000) -> dict:
    values = {"recall": per_task(rows, lambda r: recall(by_id[r["task_id"]], r["edit"]["files_changed"], comments=comments)),
              "tokens": per_task(rows, lambda r: spend(r)[0]), "cost": per_task(rows, lambda r: spend(r)[1])}
    out = {}
    for a, b in PAIRS:
        for key, v in values.items():
            ps = pairs(v, ids, a, b)
            out.setdefault(f"{a} vs {b}", {})[key] = {**paired_bootstrap(ps, n).as_dict(), "p": bootstrap_p(ps, n),
                                                     "sd": sd([x - y for x, y in ps])}
    return out


def analyse(rows: list[dict], tasks: list[dict], n_resamples: int = 10_000) -> dict:
    by_id = {t["id"]: t for t in tasks}
    ids = [t["id"] for t in tasks]
    co = _co_primary(rows, by_id, ids, n=n_resamples)
    tests = [(p, k) for p in co for k in ("recall", "tokens")]  # pairs x co-primary outcomes; cost is not a test
    for (p, k), adj in zip(tests, holm([co[p][k]["p"] for p, k in tests])):
        co[p][k]["p_holm"] = adj
    secondary: dict = {}
    for key, _label, _fmt, fn in SECONDARY:
        v = per_task(rows, lambda r: fn(by_id[r["task_id"]], r))
        secondary[key] = {"means": {c: mean([v[t][c] for t in ids if c in v.get(t, {})]) for c in ARMS},
                          "pairs": {f"{a} vs {b}": paired_bootstrap(pairs(v, ids, a, b), n_resamples).as_dict() for a, b in PAIRS}}
    for key, i in (("tokens_per_solved", 0), ("cost_per_solved", 1)):
        total = {c: [sum(spend(r)[i] for r in rows if r["condition"] == c), sum(solved(r) for r in rows if r["condition"] == c)] for c in ARMS}
        ps = {}
        for a, b in PAIRS:
            per = [tuple(x for c in (a, b) for x in (
                sum(spend(r)[i] for r in rows if r["task_id"] == t and r["condition"] == c),
                sum(solved(r) for r in rows if r["task_id"] == t and r["condition"] == c))) for t in ids]
            ps[f"{a} vs {b}"] = per_success_bootstrap(per, n_resamples)
        secondary[key] = {"means": {c: s / k if k else float("nan") for c, (s, k) in total.items()}, "pairs": ps}
    sens = {}
    for key, _label, keep, comments in SENSITIVITY:
        sub = [t for t in ids if keep(by_id[t])]
        sens[key] = {"n_tasks": len(sub), "pairs": _co_primary([r for r in rows if r["task_id"] in sub], by_id, sub, comments, n_resamples)}
    count: dict = {}
    for r in rows:
        count.setdefault((r["task_id"], r["condition"]), set()).add(r["rep"])
    missing = [f"{t}/{c}: {N_REPS - len(count.get((t, c), ()))}" for t in ids for c in ARMS if len(count.get((t, c), ())) < N_REPS]
    missing += [f"{N_TASKS - len(ids)} tasks"] if len(ids) < N_TASKS else []
    means = {c: {"recall": mean([x for r in rows if r["condition"] == c
                                 and (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"])) is not None]),
                 "sessions": sum(r["condition"] == c for r in rows),
                 "solved": sum(solved(r) for r in rows if r["condition"] == c),
                 "incomplete_usage": sum(not spend(r)[2] for r in rows if r["condition"] == c)} for c in ARMS}
    return {"co_primary": co, "secondary": secondary, "sensitivity": sens, "missing": missing, "arms": means,
            "undefined_recall": [t for t in ids if not dependents(by_id[t])]}


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
    a = analyse(rows, tasks, n_resamples)
    L = ["## Registered comparisons (#183)", "",
         f"Pre-registered in the README before the pilot. Unit: the task ({len(tasks)}), its repetitions averaged first; paired percentile "
         f"bootstrap over tasks ({n_resamples:,} resamples), 95 % intervals not corrected. Every difference is **first minus second**: "
         "a positive recall difference favours the first arm, a negative token difference favours it (it spends less). *Recall*: share "
         "of the dependents (docs, tests, code outside the primary files' directories) the session touched. *Tokens*: input including "
         "cache plus output per session, unsolved sessions included. p: two-sided, the bootstrap's; Holm over the six tests "
         "(three pairs × two outcomes). Cost is shown beside the tokens and not tested."]
    if a["missing"]:
        L += ["", f"**Incomplete run:** missing sessions (task/arm: number) {', '.join(a['missing'])}. A pair uses the tasks both arms have."]
    if a["undefined_recall"]:
        L += ["", f"No counted dependent, so no recall: {', '.join(a['undefined_recall'])}."]
    L += ["", "| Arm | Sessions | Solved (tests pass) | Recall, mean over sessions | Sessions without a result event |", "|---|---:|---:|---:|---:|"]
    for c in ARMS:
        m = a["arms"][c]
        L.append(f"| `{c}` | {m['sessions']} | {m['solved']} | {_num(m['recall'], 'f3')} | {m['incomplete_usage']} |")
    L += ["", "A session without a result event (timeout, crash) counts with the calls its transcript shows; its output tokens are a lower bound.",
          "", "| Pair | Tasks | Δ recall | p | Holm p | Tokens, ratio | Δ tokens | p | Holm p | Δ cost (USD) |",
          "|---|---:|---|---:|---:|---|---|---:|---:|---|"]
    for p, s in a["co_primary"].items():
        x, y = p.split(" vs ")
        L.append(f"| `{x}` vs `{y}` | {s['recall']['n_tasks']} | {_ci(s['recall'], 'f3')} | {_p(s['recall']['p'])} | {_p(s['recall']['p_holm'])} | "
                 f"{_ratio(s['tokens'])} | {_ci(s['tokens'], 'int')} | {_p(s['tokens']['p'])} | {_p(s['tokens']['p_holm'])} | {_ci(s['cost'], 'usd')} |")
    L += ["", "### Secondary outcomes", "", "Not tested and not corrected. Per solved task: the sum over all sessions ÷ the solved sessions, ratio first ÷ second; "
          "– where an arm solved nothing.", "",
          "| Outcome | " + " | ".join(f"`{c}`" for c in ARMS) + " | " + " | ".join(a["co_primary"]) + " |",
          "|---|" + "---:|" * len(ARMS) + "---|" * len(PAIRS)]
    for key, label, kind, _fn in SECONDARY:
        s = a["secondary"][key]
        L.append(f"| {label} | " + " | ".join(_num(s["means"][c], kind) for c in ARMS) + " | " + " | ".join(_ci(s["pairs"][p], kind) for p in a["co_primary"]) + " |")
    for key, label, kind in (("tokens_per_solved", "Tokens per solved task", "int"), ("cost_per_solved", "Cost per solved task (USD)", "usd")):
        s = a["secondary"][key]
        L.append(f"| {label} | " + " | ".join(_num(s["means"][c], kind) for c in ARMS) + " | "
                 + " | ".join(_ratio(s["pairs"][p], "ratio") for p in a["co_primary"]) + " |")
    L += ["", "### Sensitivity", "", "The co-primary differences again, not tested.", "",
          "| Analysis | Tasks | " + " | ".join(f"Δ recall, {p}" for p in a["co_primary"]) + " | " + " | ".join(f"Tokens, {p}" for p in a["co_primary"]) + " |",
          "|---|---:|" + "---|" * (2 * len(PAIRS))]
    for key, label, _keep, _c in SENSITIVITY:
        s = a["sensitivity"][key]
        L.append(f"| {label} | {s['n_tasks']} | " + " | ".join(_ci(s["pairs"][p]["recall"], "f3") for p in a["co_primary"]) + " | "
                 + " | ".join(_ratio(s["pairs"][p]["tokens"]) for p in a["co_primary"]) + " |")
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
    recalls = [x for r in rows if (x := recall(by_id[r["task_id"]], r["edit"]["files_changed"])) is not None]
    usd = mean([spend(r)[1] for r in rows]) * SESSIONS
    hours = mean([r["wall_s"] + ((r["edit"].get("test") or {}).get("seconds") or 0) + OVERHEAD_S for r in rows]) * SESSIONS / 3600
    s = sd(recalls)
    stop = ([f"projected cost ${usd:.0f} > ${MAX_USD:.0f}"] if usd > MAX_USD else []) + (
        [f"projected time {hours:.1f} h > {MAX_HOURS:.0f} h"] if hours > MAX_HOURS else [])
    return {"sessions": len(rows), "solved": sum(solved(r) for r in rows), "mean_recall": mean(recalls), "sd_recall_between_tasks": s,
            "tokens_per_session": mean([spend(r)[0] for r in rows]), "projected_usd": usd, "projected_hours": hours,
            "detectable_recall": {"one test": detectable(s), "Bonferroni, three pairs": detectable(s, alpha=0.05 / 3),
                                  "first Holm step, six tests": detectable(s, alpha=0.05 / 6)},
            "ceiling": mean(recalls) >= CEILING, "stop": stop}
