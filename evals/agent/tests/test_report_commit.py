"""#183: the registered analysis on synthetic sessions with known answers."""

import hashlib
import importlib.util
import json
import math
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
    """One primary file, two docs, one test, two remote files (one of them comments only), one beside the primary."""
    d = f"m{i}"
    files = [f"{d}/src/main/java/a/P.java", f"{d}/src/main/java/a/Near.java", f"{d}/src/main/java/b/R.java", f"{d}/src/main/java/c/Comment.java",
             f"{d}/src/test/java/a/PTests.java", "framework-docs/x.adoc", "framework-docs/y.adoc"]
    gold = [{"path": f, "status": "M", **({"comment_only": True} if "Comment" in f else {})} for f in files]
    return {"id": f"t{i:02d}", "repo": "r", "type": "edit", "answer_kind": "diff", "question": "q", "n_gold": len(gold), "gold_files": gold,
            "primary_files": files[:1], "strata": {"agents_md": i % 2 == 0, "size": "M", "docs": True}, **kw}


def row(t: dict, cond: str, rep: int = 1, touch: int = 4, ok: bool = True, tokens: int = 1000, **kw) -> dict:
    """A session that edits the primary file and the first ``touch`` counted dependents."""
    changed = t["primary_files"] + [u["path"] for u in rc.counted(t)][:touch]
    return {"run_id": f"{t['id']}__{cond}__r{rep}", "task_id": t["id"], "condition": cond, "rep": rep, "repo": "r", "type": "edit", "status": "ok",
            "final": True, "total_input_tokens": tokens - 100, "output_tokens": 100, "cost_usd": tokens / 1e4, "api_calls": 10, "wall_s": 60.0,
            "f1": 0.5, "precision": 1.0, "recall": 0.5, "success": ok, "num_turns": 3, "tool_calls": 4, "mcp_tool_calls": 0, "attempt": 1,
            "tool_calls_by_name": {"Read": 4}, "tool_errors": {}, "first_tool": "Read", "answer": None,
            "edit": {"files_changed": changed, "success": ok, "tests_passed": ok, "test": {"seconds": 30.0}}, **kw}


def run(tasks, make, reps=3):
    return [make(t, c, r) for t in tasks for c in rc.ARMS for r in range(1, reps + 1)]


class Recall(unittest.TestCase):
    def test_counted_dependents(self):
        t = task(1)
        self.assertEqual([u["kind"] for u in rc.counted(t)], ["docs", "docs", "remote", "tests"])
        self.assertEqual([u["kind"] for u in rc.counted(t, comments=True)], ["docs", "docs", "remote", "comments", "tests"])
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
        self.assertEqual([u["kind"] for u in rc.counted(t)], ["docs", "docs", "tests"])
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
        """deferred-bean-registrar: a reader/post-processor pair is one dependent. 10 counted, 11 with the comment-only file."""
        t = next(t for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"] if t["id"].startswith("deferred-bean"))
        pair = t["equivalents"][0]
        rest = [u["path"] for u in rc.counted(t, comments=True) if u["path"] not in pair]
        for one in (pair[:1], pair[1:], pair):
            self.assertEqual((rc.recall(t, one, comments=True), rc.recall(t, one)), (1 / 11, 1 / 10))
            dep = editgrade.grade_edit(t, one, {"passed": True})["dependents"]
            self.assertEqual((dep["n"], len(dep["forgotten"]), dep["by_kind"]["remote"]["n"], len(dep["by_kind"]["remote"]["forgotten"])), (10, 9, 3, 2))
        self.assertEqual((rc.recall(t, rest, comments=True), rc.recall(t, rest)), (10 / 11, 9 / 10))
        dep = editgrade.grade_edit(t, rest, {"passed": True})["dependents"]
        self.assertEqual((len(dep["forgotten"]), dep["by_kind"]["remote"]["n"], dep["by_kind"]["comments"]), (1, 3, {"n": 1, "forgotten": []}))

    def test_grading_and_report_agree_on_every_task(self):
        differ = []
        for f in ("tasks-commit-replay.json", "tasks-commit-replay-reserve.json"):
            for t in json.loads((EVAL / "tasks" / f).read_text())["tasks"]:
                paths = [g["path"] for g in t["gold_files"]]
                for changed in ([], paths, t["primary_files"], paths[::2], paths[1::2], paths[:1]):
                    by = editgrade.grade_edit(t, changed, {"passed": True})["dependents"]["by_kind"]
                    n = sum(by[k]["n"] for k in ("docs", "tests", "remote"))
                    missed = sum(len(by[k]["forgotten"]) for k in ("docs", "tests", "remote"))
                    self.assertEqual(n, len(rc.counted(t)), t["id"])
                    self.assertAlmostEqual(rc.recall(t, changed), 1 - missed / n, msg=t["id"])
                if f == "tasks-commit-replay.json" and len(rc.counted(t)) != len(rc.counted(t, comments=True)):
                    differ.append(t["id"].split("-")[0])
        self.assertEqual(differ, ["spel", "test", "deferred", "message", "retryable"])

    def test_the_task_set_has_a_counted_dependent_everywhere(self):
        for f in ("tasks-commit-replay.json", "tasks-commit-replay-reserve.json"):
            for t in json.loads((EVAL / "tasks" / f).read_text())["tasks"]:
                self.assertTrue(rc.counted(t), t["id"])
                self.assertIn("agents_md", t["strata"])


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
        self.assertEqual(a, rc.analyse(run(self.tasks, lambda t, c, r: row(t, c, r, touch[c], tokens=tokens[c])), self.tasks, N))

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
                with mock.patch.object(rc, "lines", lambda a, b, n: rc_lines(a, b, 100)):
                    report.write_report("x")
                    self.assertTrue(report.write_report("x", check=True))
                text = (d / "results/x/REPORT.md").read_text()
                with self.assertRaises(SystemExit):
                    report.regrade("x", "note")
        self.assertIn("## Registered comparisons (#183)", text)
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
