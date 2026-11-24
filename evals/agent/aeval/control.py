"""The control arm `none-docs` against arm `none` (#183).

Three sets of sessions: the 20 **recorded** sessions of arm ``none`` (24 November
2026), the 20 of ``none-docs``, and nine **concurrent** sessions of ``none`` on
the nine tasks that have docs units, interleaved with them (:func:`plan`), so
that drift and noise can be seen. **Descriptive**: nothing is tested. What was
registered before the first session (README, *Control arm*): the comparison and
the rule that reads it (:func:`outcome`). The rule counts a docs unit twice: when
the session **touched** the file, and when the file it left meets the criteria
fixed beforehand (``groundtruth/coverage_183.json``): **covered**. Coverage is
judged blind to the arm, from a shuffled sheet (:func:`sheet`) that also holds,
for every unit, the commit's own edit and a faulty decoy; a "not covered" goes to
a second blind judge, and where the two disagree the unit counts both ways.
"""

from __future__ import annotations

import difflib
import hashlib
import random
import re

from . import config
from .editgrade import file_kind, gold_paths, touched
from .report_commit import ALL_TESTS, N_TASKS, PRIMARY, counted, measured, recall, solved, spend, wall_net
from .stats import mean, paired_bootstrap, percentile

BASE, ARM = "none", "none-docs"
RECORDED, CONCURRENT, NEW = "recorded", "concurrent", "none-docs"
GOLD, DECOY = "gold", "decoy"  # on the judges' sheet only
JUDGE_ERRORS = 5  # a judge with more wrong answers than this on the commit's edits and the decoys is replaced,
JUDGE_ERRORS_DOCS = 2  # and with more than this on those of docs units
WAYS = ("touched", "covered_strict", "covered_lenient")  # an undecided unit is not covered, or covered
MOST = 0.5  # more than this share of the docs gap closed is "most of it"
PLAN_SEED = 183
KINDS = ("docs", "tests", "code")


# ---- the sessions ------------------------------------------------------------------
def docs_tasks(tasks: list[dict]) -> list[str]:
    return sorted(t["id"] for t in tasks if counted(t, ("docs",)))


def plan(tasks: list[dict], seed: int = PLAN_SEED) -> list[dict]:
    """The 29 sessions in order: one block per task, shuffled by the seed. A task
    with docs units is a pair (``none`` and ``none-docs``, adjacent), and the arm
    that comes first alternates from pair to pair; the others are ``none-docs``
    alone."""
    rng, paired = random.Random(seed), set(docs_tasks(tasks))
    blocks = sorted(t["id"] for t in tasks)
    rng.shuffle(blocks)
    out, first = [], [BASE, ARM]
    for i in blocks:
        out += [{"task": i, "condition": c, "rep": 1} for c in (first if i in paired else [ARM])]
        if i in paired:
            first = first[::-1]
    return out


def split(rows: list[dict], tasks: list[dict]) -> tuple[list[dict], list[dict]]:
    """``(concurrent none, none-docs)`` of the control run's rows."""
    if bad := sorted({r["condition"] for r in rows} - {BASE, ARM}):
        raise SystemExit(f"the control run has the arms {BASE} and {ARM} only, found {bad}")
    return [r for r in rows if r["condition"] == BASE], [r for r in rows if r["condition"] == ARM]


def _one_per_task(rows: list[dict], ids: list[str], what: str, arm: str) -> dict[str, dict]:
    if bad := sorted({r["condition"] for r in rows} - {arm}):
        raise SystemExit(f"{what}: the sessions of arm {arm} only, found {bad}")
    if sorted(r["task_id"] for r in rows) != sorted(ids):
        raise SystemExit(f"{what}: exactly one session of each of the {len(ids)} tasks")
    if bad := sorted(r["task_id"] for r in rows if not r.get("final", True) or r.get("rep") != 1):
        raise SystemExit(f"{what}: not a final session of repetition 1: {', '.join(bad)}")
    return {r["task_id"]: r for r in rows}


def load(recorded: list[dict], run: list[dict], tasks: list[dict]) -> dict[str, dict[str, dict]]:
    """The three sets, checked: the registered sessions, one Claude Code, one model."""
    if len(tasks) != N_TASKS:
        raise SystemExit(f"{len(tasks)} tasks, not {N_TASKS}")
    ids, (concurrent, new) = sorted(t["id"] for t in tasks), split(run, tasks)
    sets = {RECORDED: _one_per_task(recorded, ids, "the recorded sessions", BASE),
            CONCURRENT: _one_per_task(concurrent, docs_tasks(tasks), "the concurrent sessions", BASE),
            NEW: _one_per_task(new, ids, "the control arm", ARM)}
    for k in ("claude_version", "model"):
        seen = sorted({str(r.get(k)) for s in sets.values() for r in s.values()})
        if len(seen) != 1 or seen == ["None"]:
            raise SystemExit(f"the sets are not of one {k}: {', '.join(seen)}")
    return sets


# ---- coverage --------------------------------------------------------------------------
_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def file_diff(diff: str, path: str) -> str:
    """The part of a ``git diff`` that concerns one file."""
    parts = re.split(r"(?m)^(?=diff --git )", diff)
    return "".join(p for p in parts if p.startswith(f"diff --git a/{path} b/{path}\n"))


def after(parent: str | None, diff: str) -> str | None:
    """One file after a ``git diff`` of it; None when the diff deletes it."""
    if re.search(r"(?m)^\+\+\+ /dev/null$", diff):
        return None
    old, out, at, lines = (parent or "").removesuffix("\n").split("\n") if parent else [], [], 0, diff.split("\n")
    i = 0
    while i < len(lines):
        m = _HUNK.match(lines[i])
        i += 1
        if not m:
            continue
        start, left_old = int(m.group(1)), int(m.group(2) or 1)
        left_new = int(m.group(4) or 1)
        begin = start - 1 if left_old else start
        out += old[at:begin]
        at = begin
        while left_old > 0 or left_new > 0:
            line = lines[i]
            i += 1
            if line.startswith("\\"):
                continue
            if line[:1] in (" ", "-", ""):
                if old[at] != line[1:]:
                    raise ValueError(f"the diff does not fit the parent at line {at + 1}")
                at, left_old = at + 1, left_old - 1
            if line[:1] in (" ", "+", ""):
                out.append(line[1:])
                left_new -= 1
    return "\n".join(out + old[at:]) + "\n"


def units(task: dict) -> list[dict]:
    """The units coverage is asked for: docs and forced tests."""
    return counted(task, ("docs", "forced_tests"))


def mechanical(criterion: dict, text: str) -> bool:
    return not ("absent" in criterion and re.search(criterion["absent"], text)) and not (
        "present" in criterion and not re.search(criterion["present"], text))


def unified(before: dict[str, str | None], left: dict[str, str | None]) -> str:
    """One format for every diff on the sheet, whoever made the edit."""
    out = []
    for f in sorted(set(before) | set(left)):
        a, b = before.get(f), left.get(f)
        if a != b:
            out += difflib.unified_diff((a or "").splitlines(True), (b or "").splitlines(True), f"a/{f}" if a is not None else "/dev/null",
                                        f"b/{f}" if b is not None else "/dev/null")
    return "".join(x if x.endswith("\n") else x + "\n" for x in out)


def excerpt(before: str, left: str, gold: str, patterns: list[str], around: int = 12) -> str:
    """The parts of the file as it is left that a judge needs, with line numbers:
    around every change, around the places the commit changes, and around every
    match of a criterion's pattern; ``...`` for what is left out."""
    lines, keep = left.splitlines(), set()
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, before.splitlines(), lines, autojunk=False).get_opcodes():
        if tag != "equal":
            keep |= set(range(j1 - around, j2 + around))
    places = {i for tag, i1, i2, _, _ in difflib.SequenceMatcher(None, before.splitlines(), gold.splitlines(), autojunk=False).get_opcodes()
              if tag != "equal" for i in (i1, max(i1, i2 - 1))}
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, before.splitlines(), lines, autojunk=False).get_opcodes():
        for i in places & set(range(i1, i2 + 1)):
            at = j1 + min(i - i1, max(0, j2 - j1 - 1))
            keep |= set(range(at - around, at + around))
    for k, line in enumerate(lines):
        if any(re.search(x, line) for x in patterns):
            keep |= set(range(k - 3, k + 4))
    out, last = [], -1
    for k in sorted(x for x in keep if 0 <= x < len(lines)):
        if k != last + 1:
            out.append("...")
        out.append(f"{k + 1:5d}  {lines[k]}")
        last = k
    return "\n".join(out + (["..."] if last + 1 < len(lines) else []))


def decoy_of(spec: dict, parent: str, gold: str) -> str:
    """The faulty edit of a unit: replacements in the parent's file, or the
    commit's file with one of the parent's tests (``test``: this one) disabled or emptied."""
    if "replace" in spec:
        text = parent
        for a, b in spec["replace"]:
            if a not in text:
                raise ValueError(f"the decoy does not fit: {a[:60]}")
            text = text.replace(a, b, 1)
        return text
    tests = list(re.finditer(r"(?m)^(\t+)@Test[^\n]*\n(?:\1@[^\n]*\n)*\1[^\n]*?\bvoid (\w+)\([^\n]*\{\n", gold))
    kept = [m for m in tests if re.search(rf"\b{m.group(2)}\(", parent)] or tests  # in a renamed file every test may have another name
    if spec.get("test"):
        kept = [m for m in kept if m.group(2) == spec["test"]]
    for m in kept:
        indent = m.group(1)
        if spec["gold_with_a_test"] == "disabled":
            return gold[:m.start()] + f'{indent}@org.junit.jupiter.api.Disabled("to be adapted")\n' + gold[m.start():]
        if end := re.compile(rf"(?m)^{indent}\}}$").search(gold, m.end()):
            return gold[:m.end()] + gold[end.start():]
    raise ValueError("no test to disable or empty")


def sheet(sets: dict[str, dict[str, dict]], tasks: list[dict], criteria: dict, diff_of, parent_of, gold_of, seed: int = PLAN_SEED) -> tuple[list[dict], dict]:
    """What a judge gets and the key that stays apart. One item per docs or
    forced-test unit that a session touched, and for every unit the commit's own
    edit and a faulty decoy; all in one form, in an order by seed, numbered by it,
    without arm, set or session. ``diff_of(set, task)``, ``parent_of(task, path)``
    and ``gold_of(task, path)`` return text (None: no such file)."""
    by_id = {t["id"]: t for t in tasks}
    found = [(s, i, u) for s in (RECORDED, CONCURRENT, NEW) for i in sorted(sets.get(s, {})) for u in units(by_id[i])
             if touched(by_id[i], u, sets[s][i]["edit"]["files_changed"])]
    found += [(s, i, u) for s in (GOLD, DECOY) for i in sorted(by_id) for u in units(by_id[i])]
    random.Random(seed).shuffle(found)
    items, key = [], {}
    for n, (s, i, u) in enumerate(found, 1):
        task, spec = by_id[i], criteria[i][u["path"]]
        group = sorted({f for g in task.get("equivalents") or [] if u["path"] in g for f in g} | set(u["files"]))
        before = {f: parent_of(i, f) for f in group}
        commit = {f: gold_of(i, f) for f in group}
        if s == GOLD:
            left = commit
        elif s == DECOY:
            old, new = next(f for f in group if before[f] is not None), next(f for f in group if commit[f] is not None)
            made = decoy_of(spec["decoy"], before[old], commit[new])
            left = {**before, old: made} if "replace" in spec["decoy"] else {**commit, new: made}
        else:
            left = {f: after(before[f], d) if (d := file_diff(diff_of(s, i), f)) else before[f] for f in group}
        join = lambda fs: "\n".join(fs[f] for f in sorted(fs) if fs[f] is not None)  # noqa: E731
        text, crit = join(left), spec["criteria"]
        patterns = [x[k] for x in crit for k in ("absent", "present") if k in x]
        name = f"{n:03d}"
        items.append({"item": name, "task": i, "unit": u["path"], "change": task["question"], "wrong_before": spec["wrong"],
                      "criteria": [{"id": x["id"], "text": x["text"]} for x in crit], "diff": unified(before, left),
                      "file_after_excerpt": excerpt(join(before), text, join(commit), patterns)})
        key[name] = {"set": s, "task": i, "unit": u["path"], "kind": u["kind"], "mechanical": {x["id"]: mechanical(x, text) if {"absent", "present"} & set(x) else None for x in crit},
                     "file_sha256": hashlib.sha256(text.encode()).hexdigest()}
    return items, key


def judged_units(key: dict, judged: dict, which: str) -> dict[tuple[str, str, str], bool]:
    """``(set, task, unit) -> covered`` of one judge. A criterion's mechanical pass
    decides it; a mechanical failure, or none, leaves it to the judge (``{item:
    {criterion: true or false}}``, every item and criterion). A judge who takes more
    than :data:`JUDGE_ERRORS` of the commit's edits for not covered or of the decoys
    for covered, or more than :data:`JUDGE_ERRORS_DOCS` of those of docs units, is refused."""
    if bad := sorted(set(key) ^ set(judged)):
        raise SystemExit(f"{which} judge: not judged, or unknown: items {', '.join(bad[:10])}")
    out = {}
    for name, k in key.items():
        if set(judged[name]) != set(k["mechanical"]) or not all(isinstance(x, bool) for x in judged[name].values()):
            raise SystemExit(f"{which} judge, item {name}: a true or false for each of {', '.join(sorted(k['mechanical']))}")
        out[k["set"], k["task"], k["unit"]] = all(k["mechanical"][c] is True or ok for c, ok in judged[name].items())
    wrong, kind = judge_errors(out), {(k["set"], k["task"], k["unit"]): k["kind"] for k in key.values()}
    docs = [w for w in wrong if kind[w] == "docs"]
    if len(wrong) > JUDGE_ERRORS or len(docs) > JUDGE_ERRORS_DOCS:
        raise SystemExit(f"{which} judge: {len(wrong)} wrong answers on the commit's edits and the decoys, {len(docs)} of them on docs units "
                         f"(more than {JUDGE_ERRORS}, or {JUDGE_ERRORS_DOCS}); another judge is needed")
    return out


def judge_errors(units_: dict[tuple[str, str, str], bool]) -> list[tuple[str, str, str]]:
    return sorted(k for k, covered in units_.items() if k[0] == GOLD and not covered or k[0] == DECOY and covered)


def verdicts(first: dict[tuple[str, str, str], bool], second: dict[tuple[str, str, str], bool]) -> dict[tuple[str, str, str], bool | None]:
    """The first judge's *covered* stands; a *not covered* stands when the second,
    independent judge agrees; otherwise the unit is undecided (None)."""
    if set(first) != set(second):
        raise SystemExit("the two judges have not judged the same units")
    return {k: True if ok else None if second[k] else False for k, ok in first.items()}


# ---- the comparison --------------------------------------------------------------------
def extra_files(task: dict, changed: list[str]) -> dict[str, list[str]]:
    """Over-editing: files changed outside the commit's files, by type
    (:func:`editgrade.file_kind`). A place equivalent to a gold file is in the gold."""
    gold = set(gold_paths(task)) | {f for g in task.get("equivalents") or [] for f in g}
    out: dict[str, list[str]] = {k: [] for k in KINDS}
    for f in sorted(set(changed) - gold):
        out[file_kind(f)].append(f)
    return out


def share_closed(ps: list[tuple[float, float]]) -> float | None:
    """The share of a gap closed, from ``(base, new)`` per task; None without a gap."""
    lack = sum(b for b, _ in ps)
    return 1 - sum(n for _, n in ps) / lack if lack else None


def _interval(ps: list[tuple[float, float]], n_resamples: int, seed: int = 0) -> list[float] | None:
    """Bootstrap of it over tasks (resamples without a gap are left out)."""
    rng, draws = random.Random(seed), []
    for _ in range(n_resamples):
        if (x := share_closed([ps[rng.randrange(len(ps))] for _ in ps])) is not None:
            draws.append(x)
    draws.sort()
    return [percentile(draws, 0.025), percentile(draws, 0.975)] if draws else None


def outcome(shares: dict) -> str:
    """The registered rule. ``shares``: the share of the docs gap closed for each
    of :data:`WAYS`, against the recorded sessions (with ``interval``) and the
    concurrent ones. ``i``: all six above :data:`MOST`, ``ii``: none is;
    ``undecided`` when they disagree, a base has no docs gap, or an interval
    against the recorded sessions contains :data:`MOST`; ``unjudged`` before coverage is."""
    if not all(shares[k]["judged"] for k in WAYS):
        return "unjudged"
    points = [shares[k][b] for k in WAYS for b in (RECORDED, CONCURRENT)]
    if any(p is None for p in points):
        return "undecided"
    if any(iv is None or iv[0] <= MOST <= iv[1] for iv in (shares[k]["interval"] for k in WAYS)):
        return "undecided"
    above = {p > MOST + 1e-9 for p in points}
    return "undecided" if len(above) > 1 else "i" if above.pop() else "ii"


MEANING = {
    "i": "**Outcome (i): the sentence closes most of the docs gap.** For (a), the recall, a reminder suffices for this failure. That says nothing about tokens, the other co-primary outcome, "
         "which stays open; whether a main run is made for it is Marc's decision.",
    "ii": "**Outcome (ii): the sentence does not close most of the docs gap.** The registered next step is a main run with the arms `none`, `none-docs` and C (no D), "
          "20 tasks x 3 repetitions, Holm over four tests (C vs `none`, C vs `none-docs`; recall and tokens), `none-docs` vs `none` described. Only a gain over `none-docs` counts as more than a reminder.",
    "undecided": "**Undecided:** the touched and the covered units (undecided ones counted both ways), or the two bases, disagree, a base has no docs gap, or an interval contains 50 %. Marc decides.",
    "unjudged": "**No outcome yet:** the coverage has not been judged (`coverage-sheet`, then `control --judged`).",
}


def compare(recorded: list[dict], run: list[dict], tasks: list[dict], covered: dict | None = None, n_resamples: int = 10_000) -> dict:
    """``covered``: :func:`verdicts`; None before the judging."""
    measured(tasks)
    sets = load(recorded, run, tasks)
    by_id = {t["id"]: t for t in tasks}
    ids, nine = sorted(by_id), docs_tasks(tasks)
    files = {s: {i: r["edit"]["files_changed"] for i, r in rows.items()} for s, rows in sets.items()}
    extra = {s: {i: extra_files(by_id[i], f) for i, f in fs.items()} for s, fs in files.items()}

    def hit(s: str, i: str, u: dict, how: str = "touched") -> bool:
        if not touched(by_id[i], u, files[s][i]):
            return False
        if how == "touched" or u["kind"] == "remote":
            return True
        if (s, i, u["path"]) not in covered:
            raise SystemExit(f"no verdict on {u['path']} of {i} ({s})")
        ok = covered[s, i, u["path"]]
        return how == "covered_lenient" if ok is None else ok

    def gap(s: str, i: str, kind: str, how: str = "touched") -> float:
        us = counted(by_id[i])
        return sum(u["kind"] == kind and not hit(s, i, u, how) for u in us) / len(us)

    def tally(s: str, kind: str, how: str = "touched", only=None) -> dict:
        have = [(i, [u for u in counted(by_id[i]) if u["kind"] == kind]) for i in files[s] if only is None or i in only]
        have = [(len(us), sum(hit(s, i, u, how) for u in us)) for i, us in have if us]
        return {"units": sum(n for n, _ in have), "touched": sum(h for _, h in have), "tasks": len(have),
                "tasks_all_touched": sum(h == n for n, h in have), "tasks_any_touched": sum(h > 0 for _, h in have)}

    ways = WAYS if covered is not None else ("touched",)
    per = {}
    for i in ids:
        us, here = counted(by_id[i]), [s for s in sets if i in sets[s]]
        per[i] = {"recall": {s: recall(by_id[i], files[s][i]) for s in here},
                  **{k: {s: f"{sum(hit(s, i, u) for u in us if u['kind'] == k)}/{sum(u['kind'] == k for u in us)}" for s in here} for k in PRIMARY},
                  **({"covered": {s: [u["path"] for u in us if u["kind"] != "remote" and hit(s, i, u, "covered_strict")] for s in here},
                      "undecided_units": {s: [u["path"] for u in us if u["kind"] != "remote" and hit(s, i, u, "covered_lenient") and not hit(s, i, u, "covered_strict")] for s in here}}
                     if covered is not None else {}),
                  "found_now": [[u["kind"], u["path"]] for u in us if hit(NEW, i, u) and not hit(RECORDED, i, u)],
                  "missed_by_both": [[u["kind"], u["path"]] for u in us if not hit(NEW, i, u) and not hit(RECORDED, i, u)],
                  "missed_now_only": [[u["kind"], u["path"]] for u in us if not hit(NEW, i, u) and hit(RECORDED, i, u)],
                  "flipped_in_none": [u["path"] for u in us if i in sets[CONCURRENT] and hit(CONCURRENT, i, u) != hit(RECORDED, i, u)],
                  "tokens": {s: spend(sets[s][i])[0] for s in here}, "usd": {s: spend(sets[s][i])[1] for s in here},
                  "solved": {s: solved(sets[s][i]) for s in here}, "status": {s: sets[s][i].get("status") for s in here},
                  "extra": {s: {k: len(v) for k, v in extra[s][i].items()} for s in here}, "extra_files": extra[NEW][i]}
    summary = {}
    for s, rows in sets.items():
        audit = [r.get("isolation_audit") or {} for r in rows.values()]
        summary[s] = {"sessions": len(rows), "mean_recall": mean([per[i]["recall"][s] for i in rows]),
                      "gap": {k: mean([gap(s, i, k) for i in rows]) for k in PRIMARY}, "by_type": {k: tally(s, k) for k in PRIMARY},
                      **({"covered_by_type": {w: {k: tally(s, k, w) for k in ("docs", "forced_tests")} for w in WAYS[1:]}} if covered is not None else {}),
                      "docs_on_the_nine": {w: tally(s, "docs", w, nine)["touched"] for w in ways},
                      "recall_all_tests": mean([recall(by_id[i], files[s][i], ALL_TESTS) for i in rows]),
                      "new_tests_touched": sum(touched(by_id[i], u, files[s][i]) for i in rows for u in counted(by_id[i], ("new_tests",))),
                      "optional_touched": sum(touched(by_id[i], u, files[s][i]) for i in rows for u in counted(by_id[i], ("optional",))),
                      "solved": sum(solved(r) for r in rows.values()), "tokens_total": sum(spend(r)[0] for r in rows.values()),
                      "tokens_on_the_nine": sum(spend(rows[i])[0] for i in nine), "usd_total": sum(spend(r)[1] for r in rows.values()),
                      "wall_s_mean": mean([r["wall_s"] for r in rows.values()]), "wall_net_s_mean": mean([wall_net(r) for r in rows.values()]),
                      "api_calls_mean": mean([r.get("api_calls") or 0 for r in rows.values()]),
                      "extra": {k: {"files": sum(len(extra[s][i][k]) for i in rows), "sessions": sum(bool(extra[s][i][k]) for i in rows)} for k in KINDS},
                      "files_changed": sum(len(files[s][i]) for i in rows),
                      "refused_by_hook": sum(x.get("refused_calls", 0) for x in audit), "refused_by_claude": sum(x.get("claude_refusals", 0) for x in audit),
                      "refused_by_claude_cd": sum(x.get("claude_refusals_cd", 0) for x in audit), "disagreements": sum(x.get("disagreements", 0) for x in audit),
                      "span": [min(r["finished_at"] for r in rows.values()), max(r["finished_at"] for r in rows.values())]}
    shares: dict = {}
    for w in WAYS:
        if w not in ways:
            shares[w] = {RECORDED: None, CONCURRENT: None, "interval": None, "judged": False}
            continue
        of = {b: [(gap(b, i, "docs", w), gap(NEW, i, "docs", w)) for i in sets[b]] for b in (RECORDED, CONCURRENT)}
        shares[w] = {RECORDED: share_closed(of[RECORDED]), CONCURRENT: share_closed(of[CONCURRENT]), "interval": _interval(of[RECORDED], n_resamples), "judged": True}
    diff = lambda fn, base, over: paired_bootstrap([(float(fn(NEW, i)), float(fn(base, i))) for i in over], n_resamples).as_dict()  # noqa: E731
    row = sets[NEW][ids[0]]
    out = {"claude_version": row["claude_version"], "model": row["model"], "sentence": config.DOCS_SENTENCE, "docs_tasks": nine, "summary": summary,
           "docs_gap_closed": shares, "forced_tests_gap_closed": {w: share_closed([(gap(RECORDED, i, "forced_tests", w), gap(NEW, i, "forced_tests", w)) for i in ids]) for w in ways},
           "difference": {"recall": diff(lambda s, i: per[i]["recall"][s], RECORDED, ids), "recall_on_the_nine_concurrent": diff(lambda s, i: per[i]["recall"][s], CONCURRENT, nine),
                          "tokens": diff(lambda s, i: per[i]["tokens"][s], RECORDED, ids), "tokens_on_the_nine_concurrent": diff(lambda s, i: per[i]["tokens"][s], CONCURRENT, nine),
                          "usd": diff(lambda s, i: per[i]["usd"][s], RECORDED, ids), "extra_files": diff(lambda s, i: sum(per[i]["extra"][s].values()), RECORDED, ids)},
           "tasks": per}
    out["outcome"] = outcome(shares)
    return out


def lines(c: dict) -> list[str]:
    r, k, n = (c["summary"][s] for s in (RECORDED, CONCURRENT, NEW))
    f3, num = (lambda x: f"{x:.3f}"), (lambda x: "–" if x is None else f"{x:.2f}")
    sign = lambda s: f"{s['diff_mean']:+.3f} [{s['diff_lo']:+.3f}, {s['diff_hi']:+.3f}]"  # noqa: E731
    ratio = lambda s: f"{s['ratio_mean']:.2f}x [{s['ratio_lo']:.2f}x, {s['ratio_hi']:.2f}x]"  # noqa: E731
    span = lambda s: f"{s['span'][0][:16]} to {s['span'][1][:16]}"  # noqa: E731
    L = ["## Control arm `none-docs` against arm `none` (#183)", "",
         f"Descriptive: **nothing is tested**; intervals are 95 % bootstrap over the tasks, one session per task and set. Claude Code {c['claude_version']}, {c['model']} in all. "
         f"`none-docs` has one more sentence in the prompt: \"{c['sentence']}\" *Recorded*: the 20 sessions of `none` (finished {span(r)} UTC); *concurrent*: {k['sessions']} more on the tasks with docs units, "
         f"interleaved with `none-docs` ({span(k)}; `none-docs` {span(n)}). Recorded against concurrent shows drift and session noise; what is left between them and the sentence cannot be told apart.", "",
         MEANING[c["outcome"]], "",
         "| Share of the docs gap closed | Recorded `none` as base | Interval | Concurrent `none` as base |", "|---|---:|---|---:|"]
    say = {"touched": "touched", "covered_strict": "covered, undecided ones not", "covered_lenient": "covered, undecided ones too"}
    for w in WAYS:
        x = c["docs_gap_closed"][w]
        L.append(f"| {say[w]} | {num(x[RECORDED])} | {'–' if x['interval'] is None else '[' + ', '.join(f'{v:.2f}' for v in x['interval']) + ']'} | {num(x[CONCURRENT])} |")
    L += ["", f"More than {MOST:.2f} is \"most\". Forced tests' gap closed (decides nothing): " + ", ".join(f"{w} {num(v)}" for w, v in c["forced_tests_gap_closed"].items()) + ".", "",
          "| | `none` recorded (20) | `none` concurrent (9) | `none-docs` (20) | `none-docs` minus `none` |", "|---|---:|---:|---:|---|",
          f"| Mean recall (a) | {f3(r['mean_recall'])} | {f3(k['mean_recall'])} | {f3(n['mean_recall'])} | recorded {sign(c['difference']['recall'])}; concurrent, 9 tasks {sign(c['difference']['recall_on_the_nine_concurrent'])} |"]
    for t in PRIMARY:
        L.append(f"| {t.replace('_', ' ')}: units touched | " + " | ".join(f"{x['by_type'][t]['touched']}/{x['by_type'][t]['units']}" for x in (r, k, n)) + " | |")
    if "covered_by_type" in n:
        both = lambda x, t: "-".join(dict.fromkeys(str(x["covered_by_type"][w][t]["touched"]) for w in WAYS[1:]))  # noqa: E731
        for t in ("docs", "forced_tests"):
            L.append(f"| {t.replace('_', ' ')}: units covered (undecided: not - too) | " + " | ".join(f"{both(x, t)}/{x['covered_by_type']['covered_strict'][t]['units']}" for x in (r, k, n)) + " | |")
    L += ["| tasks with docs: all / any touched | " + " | ".join(f"{x['by_type']['docs']['tasks_all_touched']} / {x['by_type']['docs']['tasks_any_touched']} of {x['by_type']['docs']['tasks']}" for x in (r, k, n)) + " | |",
          "| Recall with new tests | " + " | ".join(f3(x["recall_all_tests"]) for x in (r, k, n)) + " | |",
          "| Solved | " + " | ".join(str(x["solved"]) for x in (r, k, n)) + " | |",
          "| Tokens, total | " + " | ".join(f"{x['tokens_total']:,.0f}" for x in (r, k, n)) + f" | recorded {ratio(c['difference']['tokens'])}; concurrent, 9 tasks {ratio(c['difference']['tokens_on_the_nine_concurrent'])} |",
          "| Tokens on the tasks with docs | " + " | ".join(f"{x['tokens_on_the_nine']:,.0f}" for x in (r, k, n)) + " | |",
          "| Cost, total (USD) | " + " | ".join(f"{x['usd_total']:.2f}" for x in (r, k, n)) + " | |",
          "| Wall time, mean s (without stalls) | " + " | ".join(f"{x['wall_s_mean']:.0f} ({x['wall_net_s_mean']:.0f})" for x in (r, k, n)) + " | |",
          "| Files changed | " + " | ".join(str(x["files_changed"]) for x in (r, k, n)) + " | |"]
    for t in KINDS:
        L.append(f"| Over-editing: {t} files outside the commit (sessions with any) | " + " | ".join(f"{x['extra'][t]['files']} ({x['extra'][t]['sessions']})" for x in (r, k, n)) + " | |")
    L += ["| In the commit but not needed: new tests / optional touched | " + " | ".join(f"{x['new_tests_touched']} / {x['optional_touched']}" for x in (r, k, n)) + " | |",
          "| Calls refused by the hook / by Claude Code (with `cd`) | " + " | ".join(f"{x['refused_by_hook']} / {x['refused_by_claude']} ({x['refused_by_claude_cd']})" for x in (r, k, n)) + " | |", "",
          "| Task | (a) recorded / concurrent / `none-docs` | Docs | Forced tests | Remote | Found now | Missed by both | Missed now only | Differs between the two `none` | Tokens | Extra files (docs/tests/code) |",
          "|---|---|---|---|---|---|---|---|---|---|---|"]
    name = lambda xs: ", ".join(f"`{(p if isinstance(p, str) else p[1]).rsplit('/', 1)[-1]}`" for p in xs) or "–"  # noqa: E731
    three = lambda d, f: " / ".join(f(d[s]) if s in d else "–" for s in (RECORDED, CONCURRENT, NEW))  # noqa: E731
    for i, t in c["tasks"].items():
        L.append(f"| `{i}` | {three(t['recall'], lambda x: f'{x:.2f}')} | {three(t['docs'], str)} | {three(t['forced_tests'], str)} | {three(t['remote'], str)} | "
                 f"{name(t['found_now'])} | {name(t['missed_by_both'])} | {name(t['missed_now_only'])} | {name(t['flipped_in_none'])} | {three(t['tokens'], lambda x: f'{x:,.0f}')} | "
                 f"{three(t['extra'], lambda x: '/'.join(str(x[k]) for k in KINDS))} |")
    return L + [""]
