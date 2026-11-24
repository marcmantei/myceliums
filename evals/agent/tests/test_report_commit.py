"""#183: the registered analysis on synthetic sessions with known answers."""

import hashlib
import importlib.util
import json
import math
import random
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

EVAL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL))

from aeval import config, editgrade, isolation, report, report_commit as rc  # noqa: E402

N = 400  # resamples: enough for direction and determinism


def task(i: int, **kw) -> dict:
    """One primary file, two docs, a forced and an unforced test, two remote files (one of them comments only), one beside the primary."""
    d = f"m{i}"
    files = [f"{d}/src/main/java/a/P.java", f"{d}/src/main/java/a/Near.java", f"{d}/src/main/java/b/R.java", f"{d}/src/main/java/c/Comment.java",
             f"{d}/src/test/java/a/PTests.java", f"{d}/src/test/java/a/MoreTests.java", "framework-docs/x.adoc", "framework-docs/y.adoc"]
    gold = [{"path": f, "status": "M", **({"comment_only": True} if "Comment" in f else {})} for f in files]
    return {"id": f"t{i:02d}", "repo": "r", "type": "edit", "answer_kind": "diff", "question": "q", "n_gold": len(gold), "gold_files": gold,
            "primary_files": files[:1], "forced_tests": files[4:5], "strata": {"agents_md": i % 2 == 0, "size": "M", "docs": True}, **kw}


def row(t: dict, cond: str, rep: int = 1, touch: int = 4, ok: bool = True, tokens: int = 1000, **kw) -> dict:
    """A session that edits the primary file and the first ``touch`` counted dependents."""
    changed = t["primary_files"] + [u["path"] for u in rc.counted(t)][:touch]
    return {"run_id": f"{t['id']}__{cond}__r{rep}", "task_id": t["id"], "condition": cond, "rep": rep, "repo": "r", "type": "edit", "status": "ok",
            "final": True, "total_input_tokens": tokens - 100, "output_tokens": 100, "cost_usd": tokens / 1e4, "api_calls": 10, "wall_s": 60.0,
            "f1": 0.5, "precision": 1.0, "recall": 0.5, "success": ok, "claude_version": "2.1.331", "model": "m", "num_turns": 3, "tool_calls": 4, "mcp_tool_calls": 0, "attempt": 1,
            "tool_calls_by_name": {"Read": 4}, "tool_errors": {}, "first_tool": "Read", "answer": None,
            "edit": {"files_changed": changed, "success": ok, "tests_passed": ok, "test": {"seconds": 30.0}}, **kw}


def decided(path: Path, tasks_file: Path, **kw) -> None:
    path.write_text(json.dumps({"ceiling": False, "mean_recall": 0.5, "sessions": 20, "claude_version": "2.1.331", "model": "m",
                                "tasks_sha256": hashlib.sha256(tasks_file.read_bytes()).hexdigest(), **kw}))


def run(tasks, make, reps=3):
    return [make(t, c, r) for t in tasks for c in rc.ARMS for r in range(1, reps + 1)]


class Recall(unittest.TestCase):
    def test_counted_dependents(self):
        t = task(1)
        self.assertEqual([u["kind"] for u in rc.counted(t)], ["docs", "docs", "remote", "forced_tests"])
        self.assertEqual([u["kind"] for u in rc.counted(t, comments=True)], ["docs", "docs", "remote", "comments", "forced_tests"])
        self.assertEqual(sorted(u["kind"] for u in rc.counted(t, rc.ALL_TESTS)), ["docs", "docs", "forced_tests", "new_tests", "remote"])
        every = [u["path"] for u in rc.counted(t)]
        self.assertEqual((rc.recall(t, every), rc.recall(t, every, comments=True)), (1.0, 0.8))
        self.assertEqual(rc.recall(t, []), 0.0)
        self.assertEqual(rc.recall(t, every[:1], ("docs",)), 0.5)
        self.assertEqual(rc.recall(t, ["m1/src/main/java/a/Near.java", *t["primary_files"]]), 0.0)  # beside a primary file: not counted
        self.assertIsNone(rc.recall({**t, "primary_files": [g["path"] for g in t["gold_files"]]}, []))

    def test_only_code_can_be_comment_only(self):
        t = task(1)
        for g in t["gold_files"]:
            g["comment_only"] = True
        self.assertEqual([u["kind"] for u in rc.counted(t)], ["docs", "docs", "forced_tests"])
        near = next(u for u in editgrade.dependents(t) if u["path"].endswith("Near.java"))
        self.assertEqual((near["kind"], near["near"]), ("comments", True))  # apart, and not in the sensitivity analysis
        self.assertEqual(len(rc.counted(t, comments=True)), 5)

    def test_either_and_equivalents(self):
        t = task(1)
        r, other = "m1/src/main/java/b/R.java", "m1/src/main/java/b/Other.java"
        self.assertNotIn(r, [u["path"] for u in rc.counted({**t, "either": [r]})])
        eq = {**t, "gold_files": t["gold_files"] + [{"path": other, "status": "M"}], "equivalents": [[r, other]]}
        self.assertEqual(len(rc.counted(eq)), len(rc.counted(t)))  # one dependent, not two
        for changed in ([r], [other], [r, other]):
            self.assertEqual((rc.recall(eq, changed), rc.precision(eq, changed)), (0.25, 1.0))
        self.assertEqual((rc.precision(t, []), rc.precision(t, [r, "x"])), (0.0, 0.5))

    def test_the_hand_example(self):
        """deferred-bean-registrar: a reader/post-processor pair is one dependent. 3 counted (no test is forced), 4 with the comment-only file, 10 with the new tests."""
        t = next(t for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"] if t["id"].startswith("deferred-bean"))
        pair = t["equivalents"][0]
        rest = [u["path"] for u in rc.counted(t, comments=True) if u["path"] not in pair]
        for one in (pair[:1], pair[1:], pair):
            self.assertEqual((rc.recall(t, one, comments=True), rc.recall(t, one), rc.recall(t, one, rc.ALL_TESTS)), (1 / 4, 1 / 3, 1 / 10))
            dep = editgrade.grade_edit(t, one, {"passed": True})["dependents"]
            self.assertEqual((dep["n"], len(dep["forgotten"]), dep["by_kind"]["remote"]["n"], len(dep["by_kind"]["remote"]["forgotten"])), (3, 2, 3, 2))
            self.assertEqual(len(dep["by_kind"]["new_tests"]["forgotten"]), 7)
        self.assertEqual((rc.recall(t, rest, comments=True), rc.recall(t, rest)), (3 / 4, 2 / 3))
        dep = editgrade.grade_edit(t, rest, {"passed": True})["dependents"]
        self.assertEqual((len(dep["forgotten"]), dep["by_kind"]["remote"]["n"], dep["by_kind"]["comments"]), (1, 3, {"n": 1, "forgotten": []}))

    def test_grading_and_report_agree_on_every_task(self):
        differ = []
        for f in ("tasks-commit-replay.json", "tasks-commit-replay-reserve.json"):
            for t in json.loads((EVAL / "tasks" / f).read_text())["tasks"]:
                paths = [g["path"] for g in t["gold_files"]]
                for changed in ([], paths, t["primary_files"], paths[::2], paths[1::2], paths[:1]):
                    dep = editgrade.grade_edit(t, changed, {"passed": True})["dependents"]
                    by = dep["by_kind"]
                    for kinds in (rc.PRIMARY, rc.ALL_TESTS):
                        n = sum(by[k]["n"] for k in kinds if k in by)
                        missed = sum(len(by[k]["forgotten"]) for k in kinds if k in by)
                        self.assertEqual(n, len(rc.counted(t, kinds)), t["id"])
                        self.assertAlmostEqual(rc.recall(t, changed, kinds) if n else 0, 1 - missed / n if n else 0, msg=t["id"])
                        self.assertEqual(rc.recall(t, changed, kinds) is None, n == 0)
                    if "forced_tests" in t:  # new tests are apart: no dependents, never forgotten
                        self.assertEqual(dep["n"], len(rc.counted(t)) + by["local"]["n"], t["id"])
                        self.assertNotIn("tests", by)
                if f == "tasks-commit-replay.json" and len(rc.counted(t)) != len(rc.counted(t, comments=True)):
                    differ.append(t["id"].split("-")[0])
        self.assertEqual(differ, ["spel", "jsonl", "test", "deferred", "message", "retryable"])

    def test_the_task_set_has_a_counted_dependent_everywhere(self):
        for f in ("tasks-commit-replay.json", "tasks-commit-replay-reserve.json"):
            for t in json.loads((EVAL / "tasks" / f).read_text())["tasks"]:
                self.assertTrue(rc.counted(t, rc.ALL_TESTS), t["id"])
                self.assertIn("agents_md", t["strata"])
                if f == "tasks-commit-replay.json":
                    self.assertTrue(rc.counted(t), t["id"])

    def test_the_numbers_by_type_are_the_registered_ones(self):
        """groundtruth/dependents_183.json, fixed with the pre-registration. A change here changes a registered denominator."""
        tasks = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]
        registered = json.loads((EVAL / "groundtruth/dependents_183.json").read_text())
        self.assertEqual({t["id"]: rc.by_type(t) for t in tasks}, registered)
        self.assertEqual({k: sum(v[k] for v in registered.values()) for k in rc.KINDS},
                         {"docs": 13, "remote": 45, "forced_tests": 14, "new_tests": 49, "optional": 4, "local": 10, "comments": 8, "build": 1, "either": 3})

    def test_the_pilot_in_arm_none_scored_again(self):
        d = EVAL / "results/pilot-183"
        tasks = {t["id"]: t for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]}
        again = json.loads((d / "rescored_none.json").read_text())
        rows = [json.loads(x) for x in (d / "sessions.jsonl").read_text().splitlines()]
        self.assertEqual({r["condition"] for r in rows}, {"none"})
        for r in rows:
            e, t, changed = again["tasks"][r["task_id"]], tasks[r["task_id"]], r["edit"]["files_changed"]
            self.assertEqual(e["recall"], rc.recall(t, changed))
            self.assertAlmostEqual(e["recall_all_tests"], rc.recall(t, changed, rc.ALL_TESTS), 4)
        out = rc.pilot(rows, list(tasks.values()), [r["task_id"] for r in rows])
        self.assertEqual((out["mean_recall"], out["mean_recall_all_tests"]), (again["summary"]["mean_recall"], again["summary"]["mean_recall_all_tests"]))


class Analysis(unittest.TestCase):
    def setUp(self):
        self.tasks = [task(i) for i in range(1, 21)]

    def test_known_differences_and_their_sign(self):
        """C touches 3 of 4, none and D 1 of 4; C spends half, D the same as none."""
        touch, tokens = {"none": 1, "C": 3, "D": 1}, {"none": 2000, "C": 1000, "D": 2000}
        a = rc.analyse(run(self.tasks, lambda t, c, r: row(t, c, r, touch[c], tokens=tokens[c])), self.tasks, N)
        co = a["co_primary"]
        self.assertEqual(list(co), ["C vs none", "D vs none", "C vs D"])
        self.assertAlmostEqual(co["C vs none"]["recall"]["diff_mean"], 0.5)  # first minus second: positive favours C
        self.assertAlmostEqual(co["C vs none"]["tokens"]["diff_mean"], -1000)  # negative: C spends less
        self.assertAlmostEqual(co["C vs none"]["tokens"]["ratio_mean"], 0.5)
        self.assertAlmostEqual(co["C vs D"]["recall"]["diff_mean"], 0.5)
        self.assertAlmostEqual(co["C vs none"]["cost"]["diff_mean"], -0.1)
        self.assertAlmostEqual(co["D vs none"]["recall"]["diff_mean"], 0.0)
        for key in ("recall", "tokens"):
            p = co["C vs none"][key]
            self.assertLess(p["p"], 0.01)
            self.assertGreaterEqual(p["p_holm"], p["p"])
            self.assertEqual((co["D vs none"][key]["p"], co["D vs none"][key]["p_holm"]), (1.0, 1.0))  # ties
        self.assertNotIn("p_holm", co["C vs none"]["cost"])  # shown, not tested
        self.assertEqual(sum("p_holm" in s[k] for s in co.values() for k in s), 6)
        self.assertEqual((a["missing"], a["undefined_recall"]), ([], []))
        again = rc.analyse(run(self.tasks, lambda t, c, r: row(t, c, r, touch[c], tokens=tokens[c])), self.tasks, N)
        self.assertEqual(json.dumps(a), json.dumps(again))  # as text: undefined ratios are nan
        self.assertEqual(a["sole_primary"], None)
        self.assertIn("recall_all_tests", a["secondary"])
        self.assertNotIn("p_holm", str(a["secondary"]))

    def test_tokens_as_the_sole_primary_outcome(self):
        """At the ceiling: Holm over the three token tests, the recall as a harm check."""
        touch, tokens = {"none": 4, "C": 2, "D": 4}, {"none": 2000, "C": 1000, "D": 2000}
        rows = run(self.tasks, lambda t, c, r: row(t, c, r, touch[c], tokens=tokens[c]))
        a = rc.analyse(rows, self.tasks, N, sole=True)
        self.assertEqual(sum("p_holm" in s[k] for s in a["co_primary"].values() for k in s), 3)
        self.assertNotIn("p_holm", a["co_primary"]["C vs none"]["recall"])
        harm = {p: s["recall"]["harm"] for p, s in a["co_primary"].items()}
        self.assertEqual({p: h["detected"] for p, h in harm.items()}, {"C vs none": True, "D vs none": False, "C vs D": True})
        both = rc.analyse(rows, self.tasks, N)["co_primary"]
        p = a["co_primary"]["C vs none"]["tokens"]["p"]
        self.assertAlmostEqual(a["co_primary"]["C vs none"]["tokens"]["p_holm"], min(1.0, 3 * p))
        self.assertGreater(both["C vs none"]["tokens"]["p_holm"], a["co_primary"]["C vs none"]["tokens"]["p_holm"])
        with tempfile.TemporaryDirectory() as d, mock.patch.object(rc, "DECISION", Path(d) / "decision.json"), \
                mock.patch.object(config, "tasks_file", lambda: Path(d) / "tasks.json"):
            (Path(d) / "tasks.json").write_text("{}")
            with self.assertRaisesRegex(SystemExit, "no recorded decision"):  # never a silent default
                rc.lines(rows, self.tasks, N)
            for ceiling, value in ((True, 0.9499), (False, 0.8996)):
                decided(rc.DECISION, Path(d) / "tasks.json", ceiling=ceiling, mean_recall=value)
                text, b = rc.lines(rows, self.tasks, N)
                text = "\n".join(text)
                self.assertEqual(b["sole_primary"], "tokens" if ceiling else None)
                self.assertEqual("Primary outcome: tokens alone" in text, ceiling)
                self.assertEqual("Co-primary outcomes: recall and tokens" in text, not ceiling)
                self.assertIn(f"mean recall {value:.3f}, ceiling 0.900", text)
                self.assertEqual("* `C vs none`: harm detected; largest loss not excluded: 0.500 (lower bound of the 95 % interval) and up to 100.0% of tasks worse, each by "
                                 "at most everything (exact one-sided 95 % bound); tasks worse/better: 20/0 of 20." in text, ceiling)
                self.assertEqual("* `D vs none`: harm not detected; largest loss not excluded: 0.000 (lower bound of the 95 % interval) and up to 13.9% of tasks worse" in text, ceiling)
                self.assertNotIn("no harm", text)
                self.assertNotIn("Not the decision's setup", text)
            with self.subTest("another CLI"):
                text = "\n".join(rc.lines([{**r, "claude_version": "2.1.400"} for r in rows], self.tasks, N)[0])
                self.assertIn("Not the decision's setup:** claude_version 2.1.400 instead of 2.1.331", text)
            (Path(d) / "tasks.json").write_text("{} ")
            with self.assertRaisesRegex(SystemExit, "task file"):
                rc.lines(rows, self.tasks, N)

    def test_the_harm_check_says_what_it_does_not_exclude(self):
        def h(ps):
            return rc.harm_check(ps, rc.paired_bootstrap(ps, N).as_dict())
        tie = h([(1.0, 1.0)] * 20)
        self.assertEqual((tie["detected"], tie["worse"], tie["better"], tie["loss"]), (False, 0, 0, 0.0))
        self.assertAlmostEqual(tie["share"], 1 - 0.05 ** (1 / 20))
        one = h([(1.0 - 1 / 11, 1.0)] + [(1.0, 1.0)] * 19)
        self.assertAlmostEqual(one["share"], 0.2161, 4)  # 1 of 20
        for x, share in ((tie, "13.9%"), (one, "21.6%")):  # both bounds, always
            self.assertIn("(lower bound of the 95 % interval)", rc.harm_text(x))
            self.assertIn(f"up to {share} of tasks worse", rc.harm_text(x))
        self.assertGreater(one["loss"], 0)
        some = h([(0.0, 1.0)] * 2 + [(1.0, 0.0)] + [(1.0, 1.0)] * 17)
        self.assertEqual((some["detected"], some["worse"], some["better"]), (False, 2, 1))
        self.assertGreater(some["loss"], 0.1)
        self.assertIn("harm not detected; largest loss not excluded: ", rc.harm_text(some))
        self.assertIn("tasks worse/better: 2/1 of 20", rc.harm_text(some))
        self.assertTrue(h([(0.5, 1.0)] * 20)["detected"])
        self.assertEqual(rc.harm_text(h([])), "no task has both arms")

    def test_the_exact_share_bound(self):
        self.assertAlmostEqual(rc.share_bound(0, 20), 1 - 0.05 ** (1 / 20))
        self.assertAlmostEqual(rc.share_bound(1, 2), math.sqrt(0.95))  # P(at most 1 of 2) = 1 - p^2 = 0.05
        self.assertEqual(rc.share_bound(20, 20), 1.0)
        bounds = [rc.share_bound(k, 20) for k in range(21)]
        self.assertEqual(bounds, sorted(bounds))
        self.assertTrue(all(b > k / 20 for k, b in enumerate(bounds[:-1])))

    def test_no_stated_bound_shrinks_when_a_task_gets_worse(self):
        rng = random.Random(7)
        def h(ps):
            return rc.harm_check(ps, rc.paired_bootstrap(ps, N).as_dict())
        starts = [[(1.0, 1.0)] * 20] + [[(rng.choice((0, 0.5, 1.0)), rng.choice((0, 0.5, 1.0))) for _ in range(20)] for _ in range(5)]
        for ps in starts:
            before = h(ps)
            for i in rng.sample(range(20), 20):
                for step in (1 / 11, 1.0):
                    ps = [(max(0.0, a - step), b) if k == i else (a, b) for k, (a, b) in enumerate(ps)]
                    after = h(ps)
                    self.assertGreaterEqual(after["loss"], before["loss"] - 1e-12)
                    self.assertGreaterEqual(after["share"], before["share"] - 1e-12)
                    before = after

    def test_pooled_units(self):
        """A one-unit task that fails costs the mean 1/20, the pooled recall one unit."""
        tasks = self.tasks[1:] + [{**self.tasks[0], "gold_files": self.tasks[0]["gold_files"][:3], "forced_tests": []}]  # one counted dependent
        rows = run(tasks, lambda t, c, r: row(t, c, r, 0 if t["id"] == "t01" and c == "C" else 4))
        a = rc.analyse(rows, tasks, N)
        self.assertAlmostEqual(a["co_primary"]["C vs none"]["recall"]["diff_mean"], -1 / 20)
        self.assertEqual(a["pooled"]["units"], 19 * 4 + 1)
        self.assertAlmostEqual(a["pooled"]["pairs"]["C vs none"]["diff_mean"], -1 / 77)
        self.assertAlmostEqual(a["pooled"]["arms"]["C"], 76 / 77)
        self.assertEqual(a["pooled"]["pairs"]["D vs none"], {"n_tasks": 20, "diff_mean": 0.0, "diff_lo": 0.0, "diff_hi": 0.0})

    def test_a_task_without_a_measured_record_is_refused(self):
        tasks = [{k: v for k, v in t.items() if k != "forced_tests"} if t["id"] == "t07" else t for t in self.tasks]
        with self.assertRaisesRegex(SystemExit, "t07"):
            rc.analyse(run(tasks, lambda t, c, r: row(t, c, r)), tasks, N)

    def test_new_tests_move_the_secondary_outcome_only(self):
        def make(t, c, r):
            out = row(t, c, r, 2)
            if c == "C":
                out["edit"]["files_changed"].append(next(u["path"] for u in rc.counted(t, ("new_tests",))))
            return out
        a = rc.analyse(run(self.tasks, make), self.tasks, N)
        self.assertEqual(a["co_primary"]["C vs none"]["recall"]["diff_mean"], 0)
        self.assertAlmostEqual(a["secondary"]["recall_all_tests"]["pairs"]["C vs none"]["diff_mean"], 0.2)
        self.assertEqual((a["secondary"]["new_tests"]["means"]["C"], a["secondary"]["new_tests"]["means"]["none"]), (1.0, 0.0))

    def test_wall_time_without_stalls(self):
        r = row(self.tasks[0], "none", isolation_audit={"api_stall_s": 12.5})
        self.assertEqual((rc.wall_net(r), rc.wall_net(row(self.tasks[0], "none"))), (47.5, 60.0))

    def test_repetitions_are_averaged_within_a_task_first(self):
        """One task with many more sessions in one arm must not weigh more."""
        rows = run(self.tasks[:2], lambda t, c, r: row(t, c, r, 4 if t["id"] == "t01" and c == "C" else 0))
        rows += [row(self.tasks[0], "C", r, 4) for r in range(4, 40)]
        s = rc.analyse(rows, self.tasks[:2], N)["co_primary"]["C vs none"]["recall"]
        self.assertAlmostEqual(s["diff_mean"], 0.5)
        self.assertEqual(s["n_tasks"], 2)

    def test_all_ties(self):
        a = rc.analyse(run(self.tasks, lambda t, c, r: row(t, c, r)), self.tasks, N)
        for s in a["co_primary"].values():
            for key in ("recall", "tokens"):
                self.assertEqual((s[key]["diff_mean"], s[key]["diff_lo"], s[key]["diff_hi"], s[key]["p"], s[key]["p_holm"]), (0, 0, 0, 1.0, 1.0))

    def test_an_arm_that_solves_nothing(self):
        rows = run(self.tasks, lambda t, c, r: row(t, c, r, ok=c != "D", tokens=3000 if c == "D" else 1000))
        a = rc.analyse(rows, self.tasks, N)
        self.assertAlmostEqual(a["co_primary"]["D vs none"]["tokens"]["diff_mean"], 2000)  # unsolved sessions are in the co-primary outcome
        per = a["secondary"]["tokens_per_solved"]
        self.assertEqual(per["means"]["none"], 1000)
        self.assertTrue(math.isnan(per["means"]["D"]) and math.isnan(per["pairs"]["D vs none"]["ratio"]))
        self.assertEqual(a["secondary"]["success"]["means"]["D"], 0.0)
        with tempfile.TemporaryDirectory() as d, mock.patch.object(rc, "DECISION", Path(d) / "decision.json"):
            decided(rc.DECISION, config.tasks_file())
            text = "\n".join(rc.lines(rows, self.tasks, N)[0])
        self.assertIn("| Tokens per solved task | 1,000 | 1,000 | – |", text)
        self.assertNotIn("nan", text)

    def test_a_session_without_a_result_event_still_costs(self):
        r = row(self.tasks[0], "C", total_input_tokens=0, output_tokens=0, cost_usd=None, status="timeout",
                tokens_by_class={"cache_read": 700.0, "output": 50.0}, priced_cost_usd=0.2)
        r["total_input_tokens"] = r["output_tokens"] = 0
        self.assertEqual(rc.spend(r), (750.0, 0.2, False))
        self.assertEqual(rc.spend(row(self.tasks[0], "C")), (1000.0, 0.1, True))

    def test_sensitivity_subsets(self):
        tasks = [task(i, **({"weak_test": "x"} if i <= 5 else {})) for i in range(1, 21)]
        # C touches every counted dependent and not the comment-only file, the others nothing more than the primary.
        def make(t, c, r):
            out = row(t, c, r, 0)
            if c == "C":
                out["edit"]["files_changed"] += [u["path"] for u in rc.counted(t)]
            return out
        a = rc.analyse(run(tasks, make), tasks, N)
        s = a["sensitivity"]
        self.assertEqual({k: v["n_tasks"] for k, v in s.items()}, {"strong": 15, "comments": 20, "agents_md": 10})
        self.assertAlmostEqual(a["co_primary"]["C vs none"]["recall"]["diff_mean"], 1.0)
        self.assertAlmostEqual(s["comments"]["pairs"]["C vs none"]["recall"]["diff_mean"], 0.8)
        self.assertEqual(a["arms"]["C"]["comments"], 0.0)
        self.assertNotIn("p_holm", s["strong"]["pairs"]["C vs none"]["recall"])

    def test_missing_sessions_are_said(self):
        rows = [r for r in run(self.tasks, lambda t, c, r: row(t, c, r)) if not (r["task_id"] == "t03" and r["condition"] == "D")]
        a = rc.analyse(rows, self.tasks, N)
        self.assertEqual(a["missing"], ["t03/D: 3"])
        self.assertEqual((a["co_primary"]["D vs none"]["recall"]["n_tasks"], a["co_primary"]["C vs none"]["recall"]["n_tasks"]), (19, 20))

    def test_holm_and_p(self):
        self.assertEqual([round(x, 3) for x in rc.holm([0.01, 0.04, 0.03])], [0.03, 0.06, 0.06])
        out = rc.holm([0.01, float("nan"), 0.5])
        self.assertTrue(math.isnan(out[1]))
        self.assertAlmostEqual(out[0], 0.03)
        self.assertEqual(rc.bootstrap_p([(1, 0), (0, 1)] * 5, N), 1.0)
        self.assertLess(rc.bootstrap_p([(1.0, 0.0)] * 10, N), 0.01)
        self.assertGreater(rc.bootstrap_p([(1, 0), (0, 1), (1, 0), (0.2, 0), (0, 0.3)], N), 0.2)

    def test_detectable_difference(self):
        self.assertEqual([round(rc.detectable(0.3, alpha=a), 2) for a in (0.05, 0.05 / 3, 0.05 / 6)], [0.19, 0.22, 0.23])


class Report(unittest.TestCase):
    def test_export_keeps_the_grades_and_the_report_is_the_registered_one(self):
        tasks = [task(i) for i in range(1, 21)]
        rows = run(tasks, lambda t, c, r: row(t, c, r, {"none": 1, "C": 3, "D": 2}[c], ok=c == "C"))
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "tasks.json").write_text(json.dumps({"repos": [], "tasks": tasks}))
            (d / "runs/x").mkdir(parents=True)
            (d / "runs/x/results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
            with mock.patch.object(config, "RUNS_DIR", d / "runs"), mock.patch.object(config, "RESULTS_DIR", d / "results"), \
                    mock.patch.object(config, "load_tasks", lambda: tasks), mock.patch.object(config, "tasks_file", lambda: d / "tasks.json"):
                report.export("x", {"conditions": ["none", "C", "D"], "comparisons": "pairwise", "success_threshold": 0.8, "reps": 3, "repos": {}})
                kept = [json.loads(x) for x in (d / "results/x/results.jsonl").read_text().splitlines()]
                self.assertEqual({r["condition"]: r["success"] for r in kept}, {"none": False, "C": True, "D": False})
                with mock.patch.object(rc, "lines", lambda a, b, n: rc_lines(a, b, 100)), mock.patch.object(rc, "DECISION", d / "decision.json"):
                    with self.assertRaisesRegex(SystemExit, "no recorded decision"):
                        report.write_report("x")
                    self.assertFalse((d / "results/x/REPORT.md").exists())
                    decided(rc.DECISION, d / "tasks.json")
                    report.write_report("x")
                    self.assertTrue(report.write_report("x", check=True))
                text = (d / "results/x/REPORT.md").read_text()
                with self.assertRaises(SystemExit):
                    report.regrade("x", "note")
        self.assertIn("## Registered comparisons (#183)", text)
        self.assertIn("Recall with new tests (the outcome registered first)", text)
        self.assertIn("| units pooled (80 dependents, a task weighs as many as it has) | 20 |", text)
        self.assertNotIn("Not the decision's setup", text)
        for other in ("F1 ≥", "Pairwise comparisons", "McNemar", "## By task type", "gold sets"):
            self.assertNotIn(other, text)
        self.assertIn("Success rate (the task's tests pass)", text)
        self.assertEqual(json.loads(json.dumps(tasks[0]))["strata"], tasks[0]["strata"])


rc_lines = rc.lines


class Pilot(unittest.TestCase):
    def test_projection_ceiling_and_limits(self):
        tasks = [task(i) for i in range(1, 6)]
        ids = [t["id"] for t in tasks]
        out = rc.pilot([row(t, "none", touch=k) for t, k in zip(tasks, (0, 1, 2, 3, 4))], tasks, ids)
        self.assertAlmostEqual(out["projected_usd"], 0.1 * 180)
        self.assertAlmostEqual(out["projected_hours"], (60 + 30 + rc.OVERHEAD_S) * 180 / 3600)
        self.assertAlmostEqual(out["mean_recall"], 0.5)
        self.assertEqual((out["ceiling"], out["stop"]), (False, []))
        high = rc.pilot([row(t, "none", tokens=20000, wall_s=600.0) for t in tasks], tasks, ids)
        self.assertEqual((high["ceiling"], len(high["stop"])), (True, 2))

    def test_it_looks_at_no_other_arm_and_at_the_drawn_tasks_only(self):
        tasks = [task(i) for i in range(1, 6)]
        ids = [t["id"] for t in tasks]
        for rows in ([row(t, "none") for t in tasks] + [row(tasks[0], "C")], [row(t, "none") for t in tasks[:4]],
                     [row(t, "none", r) for t in tasks for r in (1, 2)]):
            with self.assertRaises(SystemExit):
                rc.pilot(rows, tasks, ids)

    def test_the_draw_is_the_recorded_one(self):
        spec = importlib.util.spec_from_file_location("commit_replay_pilot", EVAL / "groundtruth" / "commit_replay.py")
        cr = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cr)
        rec = json.loads((EVAL / "groundtruth" / "pilot_183.json").read_text())
        self.assertEqual(cr.draw_pilot(rec["candidates"], rec["seed"]), {k: rec[k] for k in ("none", "C", "D")})
        self.assertEqual(len(set(rec["none"])), 5)
        self.assertEqual((len(rec["C"]), len(rec["D"])), (1, 1))
        strata = {c["id"]: (c["size"], c["docs"]) for c in rec["candidates"]}
        self.assertEqual(len({strata[t] for t in rec["none"]}), 5)
        tasks = {t["id"] for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]}
        self.assertLessEqual(set(rec["none"]), tasks)


class Decision(unittest.TestCase):
    def setUp(self):
        self.tasks = [task(i) for i in range(1, 21)]

    def test_the_rule_at_the_threshold(self):
        """20 sessions of none, one per task; 0.90 is already a ceiling, the unrounded mean decides."""
        for full, want in ((16, 0.9), (15, 0.875), (20, 1.0)):
            d = rc.decide([row(t, "none", touch=4 if i < full else 2) for i, t in enumerate(self.tasks)], self.tasks, "h")
            self.assertAlmostEqual(d["mean_recall"], want)
            self.assertEqual((d["ceiling"], d["primary"]), (want >= 0.9, ["tokens"] if want >= 0.9 else ["recall", "tokens"]))
        self.assertEqual((d["sessions"], len(d["per_task"]), d["shown"]), (20, 20, "1.000"))
        self.assertEqual((d["claude_version"], d["model"], d["tasks_sha256"]), ("2.1.331", "m", "h"))
        for mean_recall, ceiling in ((0.9 - 1e-10, True), (0.9 - 1e-8, False), (0.8996, False)):  # 0.8996 would round to 0.90
            with mock.patch.object(rc, "recall", lambda *a, **k: mean_recall):
                self.assertEqual(rc.decide([row(t, "none") for t in self.tasks], self.tasks, "h")["ceiling"], ceiling, mean_recall)

    def test_timeouts_and_unsolved_sessions_count(self):
        rows = [row(t, "none", touch=0 if i == 0 else 4, ok=i > 0, status="timeout" if i == 0 else "ok") for i, t in enumerate(self.tasks)]
        d = rc.decide(rows, self.tasks, "h")
        self.assertEqual((d["mean_recall"], d["solved"], d["ceiling"], d["statuses"]), (0.95, 19, True, {"ok": 19, "timeout": 1}))

    def test_only_the_registered_sessions(self):
        rows = [row(t, "none") for t in self.tasks]
        bad = (rows[:-1], rows + rows[:1], rows[:-1] + [row(self.tasks[-1], "C")], rows[:-1] + [row(self.tasks[-1], "none", final=False)],
               rows[:-1] + [row(self.tasks[-1], "none", 2)], rows[:-1] + [row(self.tasks[-1], "none", claude_version="2.1.400")],
               rows[:-1] + [row(self.tasks[-1], "none", model="other")], [{k: v for k, v in r.items() if k != "model"} for r in rows])
        for b in bad:
            with self.assertRaises(SystemExit):
                rc.decide(b, self.tasks, "h")
        with self.assertRaises(SystemExit):
            rc.decide(rows[:5], self.tasks[:5], "h")  # the pilot is not this check
        empty = self.tasks[1:] + [{**self.tasks[0], "gold_files": self.tasks[0]["gold_files"][:1], "forced_tests": []}]
        with self.assertRaisesRegex(SystemExit, "no counted dependent: t01"):
            rc.decide([row(t, "none") for t in empty], empty, "h")


class Describe(unittest.TestCase):
    def test_types_missed_units_and_projection(self):
        tasks = [task(i) for i in range(1, 21)]
        rows = [row(t, "none", touch=4 if i else 3, tokens=1000 * (i + 1), isolation_audit={"api_stall_s": 12.0 if i == 1 else 0.0, "claude_refusals": 1})
                for i, t in enumerate(tasks)]
        d = rc.describe(rows, tasks, {"C vs none": 0.5})
        self.assertEqual(d["by_type"]["remote"], {"units": 20, "touched": 20, "tasks": 20, "mean_over_tasks": 1.0})
        self.assertEqual((d["by_type"]["docs"]["units"], d["by_type"]["docs"]["touched"]), (40, 40))
        self.assertEqual((d["by_type"]["forced_tests"]["touched"], d["tasks_at_one"]), (19, 19))
        self.assertEqual(d["tasks"]["t01"]["missed"], [{"kind": "forced_tests", "files": ["m1/src/test/java/a/PTests.java"]}])
        self.assertAlmostEqual(d["mean_recall"], 79 / 80)
        self.assertAlmostEqual(d["pooled_recall"], 79 / 80)
        self.assertAlmostEqual(d["main_run"]["usd"], 1.05 * 180)  # mean of 0.1 .. 2.0
        lo, hi = d["main_run"]["usd_95"]
        self.assertTrue(lo < 1.05 * 180 < hi)
        self.assertAlmostEqual(d["main_run"]["hours"] - d["main_run"]["hours_without_stalls"], 12 / 20 * 180 / 3600)
        self.assertAlmostEqual(d["recall_noise_only"]["paired_sd"], math.sqrt(2 / 3) * d["sd_recall_between_tasks"])
        self.assertEqual(d["closing_docs_misses"]["gain"], 0)  # no docs unit is missed here
        self.assertAlmostEqual(d["detectable_tokens_share_of_mean"]["C vs none"]["one test"], rc.detectable(0.5))
        self.assertEqual(d["tasks"]["t02"]["claude_refusals"], 1)
        self.assertEqual(json.loads(json.dumps(d)), d)

    def test_the_arm_that_closes_every_docs_miss(self):
        tasks = [task(i) for i in range(1, 21)]
        rows = [row(t, "none", touch=0 if i < 10 else 4) for i, t in enumerate(tasks)]  # ten tasks miss both docs units of four
        c = rc.describe(rows, tasks)["closing_docs_misses"]
        self.assertAlmostEqual(c["gain"], 0.25)
        self.assertAlmostEqual(c["sd_over_tasks"], rc.sd([0.5] * 10 + [0.0] * 10))
        self.assertAlmostEqual(c["power"]["one test"], rc.power(c["gain"], c["sd_over_tasks"]))
        # The power of the detectable difference is the one it was computed for.
        for alpha in (0.05, 0.05 / 6):
            self.assertAlmostEqual(rc.power(rc.detectable(0.3, alpha=alpha), 0.3, alpha=alpha), 0.8, 3)
        self.assertAlmostEqual(rc.power(0.0, 0.3), 0.05)
        self.assertEqual(rc.power(0.1, 0.0), 1.0)
        self.assertAlmostEqual(rc.power(0.0, 0.0), 0.05)

    def test_paired_token_sds(self):
        tasks = [task(i) for i in range(1, 5)]
        rows = run(tasks, lambda t, c, r: row(t, c, r, tokens=1000 + (500 if c == "C" and t["id"] in ("t01", "t02") else 0)))
        out = rc.paired_token_sds(rows)
        self.assertAlmostEqual(out["D vs none"], 0.0)
        self.assertAlmostEqual(out["C vs none"], rc.sd([500, 500, 0, 0]) / 1000)


class Watch(unittest.TestCase):
    def test_listeners_and_remote_connections_name_process_and_port(self):
        spec = importlib.util.spec_from_file_location("watch_run", EVAL / "groundtruth" / "watch_run.py")
        w = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(w)
        line = lambda proto, name, state="": f"java.real 4711 me  42u  IPv4 0x1234567890abcdef 0t0 {proto} {name} {state}".rstrip()  # noqa: E731
        for proto, name, state, want in (
                ("TCP", "127.0.0.1:5000->127.0.0.1:5001", "(ESTABLISHED)", ("LOOP", "TCP", "", "")), ("TCP", "127.0.0.1:5000", "(LISTEN)", ("LISTEN", "TCP", "", "")),
                ("TCP", "[::1]:5000", "(LISTEN)", ("LISTEN", "TCP", "", "")), ("TCP", "*:8080", "(LISTEN)", ("LISTEN-ALL", "TCP", "4711", "*:8080")),
                ("TCP", "192.168.1.2:8080", "(LISTEN)", ("LISTEN-ALL", "TCP", "4711", "192.168.1.2:8080")), ("UDP", "*:*", "", ("LISTEN", "UDP", "", "")),
                ("TCP", "*:*", "(CLOSED)", ("UNBOUND", "TCP", "4711", "*:*")), ("TCP", "*:*", "", ("UNBOUND", "TCP", "4711", "*:*")),
                ("UDP", "*:51234", "", ("LISTEN-ALL", "UDP", "4711", "*:51234")),
                ("TCP", "10.0.0.2:5000->93.184.216.34:443", "(ESTABLISHED)", ("REMOTE", "TCP", "4711", "93.184.216.34:443"))):
            self.assertEqual(w.classify(line(proto, name, state)), want)
        self.assertIsNone(w.classify("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME"))

    def test_it_ends_with_the_process_and_stops_nothing_by_itself(self):
        with tempfile.TemporaryDirectory() as d:
            child = subprocess.Popen([sys.executable, "-I", "-c", "import time; time.sleep(1.5)"])
            watch = subprocess.Popen([sys.executable, "-I", str(EVAL / "groundtruth" / "watch_run.py"), str(child.pid), f"{d}/log", d, "--jdk", f"{d}/no-jdk"])
            child.wait()
            self.assertEqual(watch.wait(timeout=20), 0)
            text = (Path(d) / "log").read_text()
            self.assertIn(" load ", text)
            self.assertRegex(text, r"ended after \d+ samples")


class RecordedDecision(unittest.TestCase):
    """The 20 sessions of arm none (24 November 2026), from the committed rows."""

    def setUp(self):
        self.rows = [json.loads(x) for x in (EVAL / "results/none20-183/sessions.jsonl").read_text().splitlines()]
        self.tasks_file = EVAL / "tasks/tasks-commit-replay.json"
        self.tasks = json.loads(self.tasks_file.read_text())["tasks"]
        self.decision = json.loads((EVAL / "groundtruth/decision_183.json").read_text())

    def test_it_follows_from_the_rows(self):
        sha = hashlib.sha256(self.tasks_file.read_bytes()).hexdigest()
        out = rc.decide([{**r, "final": True} for r in self.rows], self.tasks, sha)  # only final rows are exported
        self.assertEqual({"label": "2026-11-24-none20", **out}, self.decision)
        self.assertEqual((out["shown"], out["ceiling"], out["primary"]), ("0.661", False, ["recall", "tokens"]))

    def test_the_summary_is_of_the_same_rows(self):
        kept = json.loads((EVAL / "results/none20-183/summary.json").read_text())
        base = [json.loads(x) for x in (EVAL / "results/182-spring/results.jsonl").read_text().splitlines()]
        out = json.loads(json.dumps(rc.describe(self.rows, self.tasks, rc.paired_token_sds(base))))
        kept.pop("load_run")
        for e in kept["tasks"].values():
            for k in [k for k in e if k.startswith("load_")]:
                e.pop(k)
        self.assertEqual(json.dumps(_rounded(out)), json.dumps(_rounded(kept)))
        self.assertEqual(sum(len(e["missed"]) for e in kept["tasks"].values()), 15)

    def test_the_rows_are_of_arm_none_and_name_no_home(self):
        self.assertEqual({(r["condition"], r["rep"], r["status"]) for r in self.rows}, {("none", 1, "ok")})
        for f in (EVAL / "results/none20-183").iterdir():
            self.assertNotIn("/Users/", f.read_text(), f.name)


def _rounded(x):
    if isinstance(x, dict):
        return {k: _rounded(v) for k, v in x.items()}
    if isinstance(x, list):
        return [_rounded(v) for v in x]
    return round(x, 8) if isinstance(x, float) else x


class WarmUp(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("warm_gradle", EVAL / "groundtruth" / "warm_gradle.py")
        self.w = importlib.util.module_from_spec(spec)
        with mock.patch.dict("os.environ"):
            spec.loader.exec_module(self.w)

    def test_the_online_command_runs_no_test(self):
        for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]:
            cmd = self.w.online_command(t["test_command"])
            for no in ("--offline", "--tests", "--continue", "--scan ", "--build-cache "):
                self.assertNotIn(f" {no}", cmd + " ")
            for yes in ("--no-scan", "--no-build-cache", "connectionTimeout=45000", "socketTimeout=45000", "max.tentatives=1", "auto-download=false"):
                self.assertIn(yes, cmd)
            self.assertRegex(cmd, r"^\./gradlew -I /private/tmp/aeval-warm/resolve-only\.gradle ")
        with self.assertRaises(SystemExit):
            self.w.online_command("./gradlew :a:test")
        self.assertIn("test.actions = []", self.w.INIT)

    def test_the_offline_record_is_of_this_init_script(self):
        rec = json.loads((EVAL / "groundtruth/warm_init_check.json").read_text())
        self.assertEqual(rec["init_sha256"], hashlib.sha256(self.w.INIT.encode()).hexdigest())
        self.assertTrue(all(r["returncode"] == 0 and r["test_reports"] == 0 for r in rec["builds"]))

    def test_a_timeout_kills_what_the_command_started(self):
        """Also a process that left the group, as Gradle's daemon does."""
        with tempfile.TemporaryDirectory() as d:
            pids = Path(d) / "pids"
            child = f"import os,time;os.setsid();open({str(pids)!r},'a').write(str(os.getpid())+chr(10));time.sleep(60)"
            cmd = f"{sys.executable} -I -c \"{child}\" & echo $! >> {pids}; echo started; sleep 60"
            out = editgrade.run_test(cmd, Path(d), timeout_s=2)
            self.assertEqual((out["timed_out"], out["passed"], out["returncode"]), (True, False, None))
            self.assertIn("started", out["output"])
            self.assertLess(out["seconds"], 20)
            time.sleep(0.3)
            for p in set(pids.read_text().split()):
                alive = subprocess.run(["ps", "-o", "stat=", "-p", p], capture_output=True, text=True).stdout.strip()
                self.assertIn(alive[:1], ("", "Z"), p)

    def test_a_test_report_is_noticed(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "spring-core/build/test-results/test"
            p.mkdir(parents=True)
            self.assertEqual(self.w.ran_tests(Path(d)), [])
            (p / "TEST-a.xml").write_text("")
            self.assertEqual(self.w.ran_tests(Path(d)), ["spring-core/build/test-results/test/TEST-a.xml"])

    def test_spring_framework_artifacts_are_not_frozen(self):
        with tempfile.TemporaryDirectory() as d:
            for ok, name in ((True, "org.springframework/spring-framework-bom/5.3.39/a/b.pom"), (True, "io.spring.javaformat/x/1/a/x.jar"),
                             (False, "org.springframework/spring-core/7.0.0/a/spring-core-7.0.0.jar"),
                             (False, "org.springframework/spring-core/7.0.0/a/spring-core-7.0.0-sources.jar")):
                src, dst = Path(d) / f"s-{Path(name).name}", Path(d) / f"d-{Path(name).name}"
                f = src / "modules-2/files-2.1" / name
                f.parent.mkdir(parents=True)
                f.write_text("x")
                if ok:
                    isolation.freeze_dependencies(src, dst)
                else:
                    with self.assertRaises(SystemExit):
                        isolation.freeze_dependencies(src, dst)
                    self.assertFalse(dst.exists())


if __name__ == "__main__":
    unittest.main()
