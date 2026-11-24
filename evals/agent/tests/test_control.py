"""#183: the control arm `none-docs`: one sentence, the hook of the recorded run, the registered comparison and design."""

import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

EVAL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL))

from aeval import bashcmd, config, control, report, report_commit as rc, report_variants  # noqa: E402
from aeval.runner import RunConfig, Runner, WorkItem, build_prompt  # noqa: E402

_spec = importlib.util.spec_from_file_location("synthetic_sessions", EVAL / "tests" / "test_report_commit.py")
syn = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(syn)
SENTENCE = "Also update existing documentation and tests that the change makes wrong or incomplete."


class Arm(unittest.TestCase):
    def test_one_sentence_and_nothing_else(self):
        self.assertEqual(config.DOCS_SENTENCE, SENTENCE)
        self.assertEqual({c: v.prompt_extra for c, v in config.VARIANTS.items() if v.prompt_extra}, {"none-docs": SENTENCE})
        a, b = (dict(vars(config.VARIANTS[c])) for c in ("none", "none-docs"))
        self.assertEqual((a.pop("prompt_extra"), b.pop("prompt_extra")), ("", SENTENCE))
        self.assertEqual(a, b)  # every other layer is the same
        with mock.patch.dict(os.environ, {"AEVAL_TASKS": str(EVAL / "tasks/tasks-commit-replay.json")}):
            repos = config.load_repos()
            for t in config.load_tasks():
                plain = build_prompt(t, repos[t["repo"]])
                more = build_prompt(t, repos[t["repo"]], None, config.VARIANTS["none-docs"].prompt_extra)
                self.assertEqual(more.count(" " + SENTENCE), 1)
                self.assertEqual(more.replace(" " + SENTENCE, ""), plain, t["id"])
                self.assertIn("and nothing else. Do not commit. " + SENTENCE + "\n\nChange: ", more)
                self.assertNotIn("{", plain.replace(t["question"], "").replace("{0}", ""))
        with self.assertRaises(ValueError):
            build_prompt({"id": "q", "answer_kind": "symbols", "question": "x"}, {}, None, SENTENCE)

    def test_a_session_gets_the_sentence_of_its_arm(self):
        """The runner asks the variant; everything the runner gives a session is the same otherwise."""
        seen = {}
        task = {"id": "t", "repo": "r", "answer_kind": "diff", "question": "q", "test_command": "x"}
        repo = {"name": "r", "display_name": "R", "language_name": "Java", "isolated": True}
        for c in ("none", "none-docs"):
            with tempfile.TemporaryDirectory() as d, mock.patch.object(config, "RUNS_DIR", Path(d)), \
                    mock.patch.object(Runner, "_prepare_workspace", side_effect=RuntimeError("stop here")), \
                    mock.patch.object(config, "session_root", lambda r: Path(d)):
                r = Runner(RunConfig(label="x", conditions=(c,), bash=True), [task], {"r": repo})
                r.run_dir.mkdir(parents=True)
                with self.assertRaisesRegex(RuntimeError, "stop here"):
                    r._session(WorkItem(task, c, 1), 1)
                seen[c] = (Path(d) / "x/sessions" / f"t__{c}__r1" / "prompt.txt").read_text()
                r.cfg.claude = "claude"
                seen[c, "cmd"] = r._command(WorkItem(task, c, 1), Path("mcp.json"), Path("s.json"))
                seen[c, "tools"] = r._builtin_tools(WorkItem(task, c, 1))
        self.assertEqual(seen["none-docs"].replace(" " + SENTENCE, ""), seen["none"])
        self.assertNotEqual(seen["none-docs"], seen["none"])
        for k in ("cmd", "tools"):
            self.assertEqual(seen["none", k], seen["none-docs", k])

    def test_the_schedule_knows_it(self):
        tasks = [{"id": f"t{i:02d}", "repo": "r", "answer_kind": "diff"} for i in range(20)]
        plan = lambda conds, reps: Runner(RunConfig(label="x", conditions=conds, reps=reps, schedule="interleaved"), tasks, {"r": {"name": "r"}}).plan()  # noqa: E731
        # The 20 sessions come in the order of the 20 of arm none (same seed).
        self.assertEqual([i.task["id"] for i in plan(("none",), 3)[:20]], [i.task["id"] for i in plan(("none-docs",), 3)[:20]])
        full = plan(("none", "none-docs", "C"), 3)
        self.assertEqual(len(full), 180)
        for k in range(0, 180, 3):  # the three arms of a task are adjacent
            self.assertEqual({i.condition for i in full[k:k + 3]}, {"none", "none-docs", "C"})
            self.assertEqual(len({(i.task["id"], i.rep) for i in full[k:k + 3]}), 1)

    def test_old_run_configs_are_unchanged(self):
        c = RunConfig(label="x", conditions=("none",)).comparable()
        self.assertNotIn("hook_cd", c)
        self.assertTrue(RunConfig(label="x", hook_cd=True).comparable()["hook_cd"])


class Hook(unittest.TestCase):
    def test_cd_is_let_through_only_on_request(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d).resolve() / "work"
            (root / "spring-core").mkdir(parents=True)
            (root / "spring-core/a").write_text("x")

            def run(cmd, *more):
                out = io.StringIO()
                bashcmd.isolated_hook(["--root", str(root), "--search", "yes", "--allow-json", "[]", *more],
                                      io.StringIO(json.dumps({"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": str(root)})), out)
                return out.getvalue()
            for cmd in ("cd spring-core", "cd spring-core && cat a", "cd spring-core; ls"):
                self.assertIn("the shell already stands at the top", run(cmd))
                self.assertIn("the shell already stands at the top", run(cmd, "--cd", "no"))
                self.assertEqual(run(cmd, "--cd", "yes"), "", cmd)
            for cmd in ("cd ..", "cd /etc", "cd nowhere; cat a", "cd spring-core && cat ../../x", "pushd spring-core"):  # still confined
                self.assertNotEqual(run(cmd, "--cd", "yes"), "", cmd)
            self.assertNotIn("Bash allows cd", run("curl x", "--cd", "yes"))  # the refusal text of that run did not offer it

    def test_the_runner_passes_it_on(self):
        for flag, want in ((False, []), (True, ["--cd", "yes"])):
            with tempfile.TemporaryDirectory() as d:
                t = {"id": "t", "repo": "r", "answer_kind": "diff", "question": "q", "test_command": "x"}
                r = Runner(RunConfig(label="x", conditions=("none-docs",), bash=True, hook_cd=flag), [t], {"r": {"name": "r", "isolated": True}})
                with mock.patch("aeval.isolation.available", lambda: True), mock.patch("aeval.isolation.problem", lambda env: None), \
                        mock.patch("aeval.isolation.session_environment", lambda tmp, g="gh": {}), \
                        mock.patch("aeval.isolation.session_settings", lambda tmp, w, env, myc, hook: {"hook": hook}):
                    hook = json.loads(r._isolated_settings(WorkItem(t, "none-docs", 1), Path(d), Path(d) / "w").read_text())["hook"]
                import shlex
                words = shlex.split(hook)
                self.assertEqual(words[words.index("--allow-json") + 2:], want)
                self.assertEqual(words[words.index("--search") + 1], "yes")


class Comparison(unittest.TestCase):
    def setUp(self):
        self.tasks = [syn.task(i) for i in range(1, 21)]  # docs, docs, remote, forced test

    def rows(self, cond, touch, **kw):
        return [syn.row(t, cond, touch=touch(i) if callable(touch) else touch, finished_at=f"2026-11-24T1{i % 10}:00:00", **kw) for i, t in enumerate(self.tasks)]

    def drop(self, rows, word):
        for r in rows:
            r["edit"]["files_changed"] = [f for f in r["edit"]["files_changed"] if word not in f]
        return rows

    def test_types_missed_units_and_the_rule(self):
        base = self.drop(self.rows("none", 4), "adoc")  # every docs unit missed
        half = self.rows("none-docs", 4)
        for r in half[:10]:
            r["edit"]["files_changed"] = [f for f in r["edit"]["files_changed"] if "adoc" not in f]
        c = control.compare(base, half, self.tasks, 300)
        self.assertEqual((c["summary"]["none"]["gap"]["docs"], c["summary"]["none-docs"]["gap"]["docs"]), (0.5, 0.25))
        self.assertEqual((c["gap_closed"]["docs"], c["outcome"]), (0.5, "ii"))  # exactly half is not most
        self.assertEqual(c["gap_closed"]["remote"], None)  # nothing was missing
        self.assertAlmostEqual(c["difference"]["recall"]["diff_mean"], 0.25)  # first minus second: none-docs minus none
        self.assertEqual(c["summary"]["none-docs"]["by_type"]["docs"], {"units": 40, "touched": 20, "tasks": 20, "tasks_all_touched": 10, "tasks_any_touched": 10})
        self.assertEqual([k for k, _ in c["tasks"]["t20"]["found_now"]], ["docs", "docs"])
        self.assertEqual(len(c["tasks"]["t01"]["missed_by_both"]), 2)
        half[0]["edit"]["files_changed"] += ["framework-docs/x.adoc"]
        c = control.compare(base, half, self.tasks, 300)
        self.assertEqual(c["outcome"], "i")
        lo, hi = c["docs_gap_closed_95"]
        self.assertTrue(0 <= lo < c["gap_closed"]["docs"] < hi <= 1)
        text = "\n".join(control.lines(c))
        self.assertIn("Outcome (i)", text)
        self.assertIn("The interval contains 0.50: the rule decides by the point, the data do not.", text)
        clear = control.lines(control.compare(base, self.rows("none-docs", 4), self.tasks, 300))
        self.assertNotIn("The interval contains", "\n".join(clear))
        self.assertIn("nothing is tested", text)
        self.assertIn(SENTENCE, text)
        self.assertIn("drift", text)
        self.assertNotIn("nan", text)
        self.assertEqual(json.dumps(json.loads(json.dumps(c))), json.dumps(c))  # as text: a ratio to nothing is nan
        worse = self.drop(self.rows("none-docs", 4), "/b/R.java")
        c = control.compare(self.rows("none", 4), worse, self.tasks, 300)
        self.assertEqual((c["outcome"], c["gap_closed"]["docs"], len(c["tasks"]["t01"]["missed_now_only"])), ("ii", None, 1))

    def test_the_rule(self):
        self.assertEqual([control.outcome(x) for x in (None, -0.2, 0.0, 0.5, 0.500001, 1.0)], ["ii", "ii", "ii", "ii", "i", "i"])
        for k in ("i", "ii"):
            self.assertIn(f"Outcome ({k})", control.MEANING[k])
        self.assertIn("Holm over four tests (C vs `none`, C vs `none-docs`; recall and tokens)", control.MEANING["ii"])

    def test_over_editing_by_type(self):
        base, new = self.rows("none", 4), self.rows("none-docs", 4)
        new[0]["edit"]["files_changed"] += ["framework-docs/z.adoc", "README.md", "m1/src/test/java/a/XTests.java", "m1/src/main/java/z/Z.java"]
        new[1]["edit"]["files_changed"] += ["framework-docs/z.adoc", "m2/src/test/java/a/MoreTests.java"]  # the second is in the commit: a new test
        c = control.compare(base, new, self.tasks, 100)
        self.assertEqual(c["summary"]["none-docs"]["extra"], {"docs": {"files": 3, "sessions": 2}, "tests": {"files": 1, "sessions": 1}, "code": {"files": 1, "sessions": 1}})
        self.assertEqual(c["tasks"]["t01"]["extra_files"]["docs"], ["README.md", "framework-docs/z.adoc"])
        self.assertEqual((c["summary"]["none-docs"]["new_tests_touched"], c["summary"]["none"]["new_tests_touched"]), (1, 0))
        t = {**self.tasks[0], "equivalents": [["m1/src/main/java/b/R.java", "m1/src/main/java/b/Other.java"]]}
        self.assertEqual(control.extra_files(t, ["m1/src/main/java/b/Other.java"]), {"docs": [], "tests": [], "code": []})

    def test_only_the_registered_sessions(self):
        base, new = self.rows("none", 4), self.rows("none-docs", 4)
        bad = ((base[:-1], new), (base, new[:-1]), (base, new + new[:1]), (new, base), (base, base), (base, [{**new[0], "rep": 2}, *new[1:]]),
               (base, [{**new[0], "final": False}, *new[1:]]), (base, [{**r, "claude_version": "2.1.400"} for r in new]), (base, [{**r, "model": "other"} for r in new]))
        for b, n in bad:
            with self.assertRaises(SystemExit):
                control.compare(b, n, self.tasks, 10)

    def test_against_the_recorded_sessions(self):
        base = [json.loads(x) for x in (EVAL / "results/none20-183/sessions.jsonl").read_text().splitlines()]
        tasks = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]
        c = control.compare(base, [{**r, "condition": "none-docs"} for r in base], tasks, 100)
        s = c["summary"]["none"]
        self.assertEqual({k: round(v, 5) for k, v in s["gap"].items()}, {"docs": 0.25375, "remote": 0.005, "forced_tests": 0.08})  # the gap the rule is about
        self.assertEqual((s["by_type"]["docs"]["touched"], s["by_type"]["docs"]["units"], s["by_type"]["docs"]["tasks_all_touched"], s["by_type"]["docs"]["tasks"]), (2, 13, 2, 9))
        self.assertEqual(s["extra"], {"docs": {"files": 0, "sessions": 0}, "tests": {"files": 0, "sessions": 0}, "code": {"files": 2, "sessions": 1}})
        self.assertEqual((c["gap_closed"]["docs"], c["outcome"]), (0.0, "ii"))


class ControlledDesign(unittest.TestCase):
    """The main run registered for outcome (ii): none, none-docs, C."""

    def setUp(self):
        self.tasks = [syn.task(i) for i in range(1, 21)]
        touch, tokens = {"none": 1, "none-docs": 3, "C": 3}, {"none": 2000, "none-docs": 2100, "C": 1000}
        self.rows = [syn.row(t, c, r, touch[c], tokens=tokens[c]) for t in self.tasks for c in touch for r in (1, 2, 3)]

    def test_holm_over_four_tests(self):
        self.assertIs(rc.design_of(self.rows), rc.CONTROLLED)
        a = rc.analyse(self.rows, self.tasks, syn.N)
        co = a["co_primary"]
        self.assertEqual(list(co), ["C vs none", "C vs none-docs", "none-docs vs none"])
        self.assertEqual(sum("p_holm" in s[k] for s in co.values() for k in s), 4)
        for k in ("recall", "tokens"):  # described: no p at all
            self.assertNotIn("p", co["none-docs vs none"][k])
            self.assertNotIn("p_holm", co["none-docs vs none"][k])
        self.assertAlmostEqual(co["C vs none"]["recall"]["diff_mean"], 0.5)
        self.assertAlmostEqual(co["C vs none-docs"]["recall"]["diff_mean"], 0.0)
        self.assertEqual(co["C vs none-docs"]["recall"]["p_holm"], 1.0)  # a gain over none alone is not more than a reminder
        self.assertAlmostEqual(co["C vs none"]["recall"]["p_holm"], min(1.0, 4 * co["C vs none"]["recall"]["p"]), 6)
        self.assertEqual((a["missing"], list(a["arms"])), ([], ["none", "none-docs", "C"]))
        self.assertEqual(sum("p_holm" in s[k] for s in rc.analyse(self.rows, self.tasks, syn.N, sole=True)["co_primary"].values() for k in s), 2)

    def test_the_report(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.object(rc, "DECISION", Path(d) / "decision.json"):
            syn.decided(rc.DECISION, config.tasks_file())
            text = "\n".join(rc.lines(self.rows, self.tasks, syn.N)[0])
        self.assertIn("Holm over the four tests (two pairs: `C` vs `none`, `C` vs `none-docs` × two outcomes)", text)
        self.assertIn("`none-docs` vs `none` is described, not tested. **Only a gain of C over `none-docs` counts as more than a reminder**", text)
        self.assertIn("| `none-docs` vs `none` | 20 |", text)
        self.assertNotIn("nan", text)
        self.assertNotIn("`D`", text)

    def test_the_registered_design_of_before_is_unchanged(self):
        rows = [syn.row(t, c, r, 2) for t in self.tasks for c in rc.ARMS for r in (1, 2, 3)]
        self.assertIs(rc.design_of(rows), rc.REGISTERED)
        self.assertIs(rc.design_of(rows[:1]), rc.REGISTERED)
        self.assertEqual((rc.REGISTERED.arms, rc.REGISTERED.pairs, rc.REGISTERED.described), (("none", "C", "D"), rc.PAIRS, ()))
        with tempfile.TemporaryDirectory() as d, mock.patch.object(rc, "DECISION", Path(d) / "decision.json"):
            syn.decided(rc.DECISION, config.tasks_file())
            text = "\n".join(rc.lines(rows, self.tasks, syn.N)[0])
        self.assertIn("**Co-primary outcomes: recall and tokens.** Holm over the six tests (three pairs × two outcomes).\n", text)
        self.assertNotIn("none-docs", text)
        self.assertNotIn("reminder", text)

    def test_export_and_report_know_the_arm(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "tasks.json").write_text(json.dumps({"repos": [], "tasks": self.tasks}))
            (d / "runs/x").mkdir(parents=True)
            (d / "runs/x/results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in self.rows))
            with mock.patch.object(config, "RUNS_DIR", d / "runs"), mock.patch.object(config, "RESULTS_DIR", d / "results"), \
                    mock.patch.object(config, "load_tasks", lambda: self.tasks), mock.patch.object(config, "tasks_file", lambda: d / "tasks.json"), \
                    mock.patch.object(rc, "DECISION", d / "decision.json"):
                syn.decided(rc.DECISION, d / "tasks.json")
                report.export("x", {"conditions": ["none", "none-docs", "C"], "comparisons": "pairwise", "success_threshold": 0.8, "reps": 3, "repos": {}})
                lines = rc.lines
                with mock.patch.object(rc, "lines", lambda a, b, n: lines(a, b, 100)):
                    report.write_report("x")
                text = (d / "results/x/REPORT.md").read_text()
        self.assertIn("## Registered comparisons (#183)", text)
        self.assertIn("prompt (but for the one sentence of `none-docs`), model,", text)
        self.assertIn(SENTENCE, text)
        self.assertNotIn("Pairwise comparisons", text.replace("see *Pairwise comparisons*", ""))
        self.assertIn("none-docs", report_variants.DESCRIPTIONS)


class Script(unittest.TestCase):
    SCRIPT = EVAL / "groundtruth/run_none_docs_183.sh"

    def test_the_command_is_that_of_arm_none_but_for_three_things(self):
        out = subprocess.run(["/bin/bash", str(self.SCRIPT), "--dry-run"], capture_output=True, text=True, env={"PATH": "/usr/bin:/bin", "HOME": "/h"})
        self.assertEqual((out.returncode, out.stderr), (0, ""))
        words = next(x for x in out.stdout.splitlines() if x.startswith("+")).split()
        for drop in ("--hook-cd",):
            words.remove(drop)
        got = {w: words[i + 1] for i, w in enumerate(words[:-1]) if w.startswith("--")}
        self.assertEqual((got.pop("--conditions"), got.pop("--label")), ("none-docs", "2026-11-24-none-docs20"))
        self.assertEqual({k: v for k, v in got.items() if k in ("--rep", "--profile", "--timeout", "--concurrency", "--max-sessions")},
                         {"--rep": "1", "--profile": "realistic", "--timeout": "2400", "--concurrency": "1", "--max-sessions": "24"})
        self.assertIn("--bash", words)
        self.assertEqual(subprocess.run(["/bin/bash", str(self.SCRIPT), "--x"], capture_output=True).returncode, 64)

    def test_it_pins_what_the_20_sessions_had(self):
        text, d = self.SCRIPT.read_text(), json.loads((EVAL / "groundtruth/decision_183.json").read_text())
        self.assertIn(f"TASKS_SHA={d['tasks_sha256']}", text)
        self.assertIn(f"CLAUDE_VERSION={d['claude_version']}", text)
        meta = json.loads((EVAL / "results/none20-183/meta.json").read_text())
        self.assertIn(f"MYC_SHA={meta['myc_sha256']}", text)
        self.assertEqual((meta["profile"], meta["timeout_s"], meta["max_turns"], meta["effort"], meta["model_alias"]), ("realistic", 2400, 100, "medium", "sonnet"))


if __name__ == "__main__":
    unittest.main()
