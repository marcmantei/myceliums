"""#183: the control arm `none-docs`: one sentence, the hook of the recorded run, the registered comparison and design."""

import importlib.util
import io
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

EVAL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL))

import agent_eval  # noqa: E402
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
        full = Runner(RunConfig(label="x", conditions=("none", "none-docs", "C"), reps=3, schedule="interleaved"), tasks, {"r": {"name": "r"}}).plan()
        self.assertEqual(len(full), 180)
        for k in range(0, 180, 3):  # the three arms of a task are adjacent
            self.assertEqual({i.condition for i in full[k:k + 3]}, {"none", "none-docs", "C"})
            self.assertEqual(len({(i.task["id"], i.rep) for i in full[k:k + 3]}), 1)

    def test_the_29_sessions_and_their_order(self):
        tasks = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]
        rec = json.loads((EVAL / "groundtruth/control_plan_183.json").read_text())
        self.assertEqual(rec["sessions"], control.plan(tasks, rec["seed"]))
        s = [(x["task"], x["condition"], x["rep"]) for x in rec["sessions"]]
        nine = control.docs_tasks(tasks)
        self.assertEqual((len(s), len(set(s)), len(nine)), (29, 29, 9))
        self.assertEqual(sorted(t for t, c, _ in s if c == "none-docs"), sorted(t["id"] for t in tasks))
        self.assertEqual(sorted(t for t, c, _ in s if c == "none"), nine)
        first = [c for k, (t, c, _) in enumerate(s) if t in nine and (k == 0 or s[k - 1][0] != t)]
        self.assertEqual(first, ["none", "none-docs"] * 4 + ["none"])  # the pairs are adjacent, the arm that comes first alternates
        runner = Runner(RunConfig(label="x", conditions=("none", "none-docs"), reps=3, schedule="interleaved", plan=tuple(s)), tasks, {t["repo"]: {"name": t["repo"]} for t in tasks})
        self.assertEqual([(i.task["id"], i.condition, i.rep) for i in runner.plan()], s)
        self.assertEqual(json.loads(json.dumps(runner.cfg.comparable()))["plan"], [list(x) for x in s])
        for bad in (s + s[:1], [("nope", "none", 1)], [(s[0][0], "C", 1)]):
            runner.cfg.plan = tuple(bad)
            with self.assertRaises(SystemExit):
                runner.plan()
        args = mock.Mock(plan=str(EVAL / "groundtruth/control_plan_183.json"))
        self.assertEqual(list(agent_eval._planned(args.plan)), s)
        self.assertEqual(agent_eval._planned(None), ())

    def test_old_run_configs_are_unchanged(self):
        c = RunConfig(label="x", conditions=("none",)).comparable()
        self.assertNotIn("hook_cd", c)
        self.assertNotIn("plan", c)
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
                words = shlex.split(hook)
                self.assertEqual(words[words.index("--allow-json") + 2:], want)
                self.assertEqual(words[words.index("--search") + 1], "yes")


class Export(unittest.TestCase):
    def test_the_task_file_is_recorded_with_and_without_the_flags(self):
        for more, want in (({}, {}), ({"hook_cd": True, "plan": [["t", "none", 1]]}, {"hook_cd": True, "planned_sessions": 1})):
            with tempfile.TemporaryDirectory() as d:
                d = Path(d)
                (d / "runs/x").mkdir(parents=True)
                (d / "myc").write_text("x")
                (d / "tasks.json").write_text(json.dumps({"tasks": []}))
                (d / "runs/x/results.jsonl").write_text("")
                cfg = {"label": "x", "model": "sonnet", "effort": "medium", "max_turns": 100, "timeout_s": 1, "reps": 1, "success_threshold": 0.8,
                       "tool_search": None, "myc": str(d / "myc"), "conditions": ["none"], **more}
                (d / "runs/x/config.json").write_text(json.dumps(cfg))
                got = {}
                with mock.patch.object(config, "RUNS_DIR", d / "runs"), mock.patch.object(config, "tasks_file", lambda: d / "tasks.json"), \
                        mock.patch.object(config, "load_repos", lambda: {}), mock.patch.object(agent_eval.prep, "index_meta", lambda: {}), \
                        mock.patch.object(agent_eval.report, "export", lambda label, meta: got.update(meta) or d):
                    agent_eval.cmd_export(mock.Mock(label="x", date="2026-11-24", myc_commit="c", note=None))
                self.assertEqual(got["tasks_file"], os.path.relpath(d / "tasks.json", config.EVAL_DIR))
                self.assertEqual({k: got[k] for k in ("hook_cd", "planned_sessions") if k in got}, want)


def _diff(path, old, new):
    import difflib
    body = "".join(difflib.unified_diff(old.splitlines(True), new.splitlines(True), f"a/{path}" if old else "/dev/null", f"b/{path}" if new else "/dev/null"))
    return f"diff --git a/{path} b/{path}\nindex 1..2 100644\n{body}"


class Coverage(unittest.TestCase):
    def test_a_file_is_rebuilt_from_its_diff(self):
        old = "".join(f"line {i}\n" for i in range(60))
        new = old.replace("line 3\n", "").replace("line 30\n", "line thirty\nand one more\n").replace("line 59\n", "the end\n")
        both = _diff("x/a.adoc", "p\n", "q\n") + _diff("x/b.adoc", old, new) + _diff("x/b.adoc.txt", "1\n", "2\n")
        self.assertEqual(control.after(old, control.file_diff(both, "x/b.adoc")), new)
        self.assertEqual(control.after(None, _diff("n", "", "a\nb\n")), "a\nb\n")
        self.assertIsNone(control.after("a\n", _diff("n", "a\n", "")))
        self.assertEqual(control.file_diff(both, "x/c.adoc"), "")
        with self.assertRaises(ValueError):
            control.after(old.replace("line 30", "other"), control.file_diff(both, "x/b.adoc"))

    def test_the_criteria_are_for_every_unit_and_nothing_else(self):
        tasks = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]
        rec = json.loads((EVAL / "groundtruth/coverage_183.json").read_text())["units"]
        self.assertEqual({(t["id"], u["path"]) for t in tasks for u in control.units(t)}, {(t, p) for t, v in rec.items() for p in v})
        self.assertEqual(sum(len(v) for v in rec.values()), 27)
        for v in rec.values():
            for x in v.values():
                self.assertTrue(x["wrong"])
                self.assertTrue(set(x["decoy"]["breaks"]) <= {c["id"] for c in x["criteria"]} and x["decoy"]["breaks"])
                self.assertEqual(("replace" in x["decoy"]) + ("gold_with_a_test" in x["decoy"]), 1)
                self.assertEqual([c["id"] for c in x["criteria"]], [f"c{i}" for i in range(1, len(x["criteria"]) + 1)])
                self.assertTrue(all(set(c) <= {"id", "text", "absent", "present"} and c["text"] for c in x["criteria"]))

    def test_the_commit_meets_the_mechanical_criteria_and_the_parent_does_not(self):
        tasks = {t["id"]: t for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]}
        rec, seen = json.loads((EVAL / "groundtruth/coverage_183.json").read_text())["units"], 0
        for i, v in rec.items():
            t = tasks[i]
            for p, x in v.items():
                parent, mech = config.cache_dir() / "repos" / t["repo"] / p, [c for c in x["criteria"] if "absent" in c or "present" in c]
                group = next((g for g in t.get("equivalents") or [] if p in g), [p])
                gold = [f for f in (config.gold_dir(t["repo"]) / "files" / q for q in group) if f.is_file()]
                if not parent.is_file() or not gold or not mech:
                    continue
                seen += 1
                self.assertTrue(all(control.mechanical(c, gold[-1].read_text()) for c in mech), p)
                self.assertFalse(all(control.mechanical(c, parent.read_text()) for c in mech), p)
        if not seen:
            self.skipTest("no checkouts here")

    def test_the_decoys_fit_and_break_what_they_say(self):
        tasks = {t["id"]: t for t in json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]}
        rec, seen = json.loads((EVAL / "groundtruth/coverage_183.json").read_text())["units"], 0
        for i, v in rec.items():
            t = tasks[i]
            for p, x in v.items():
                group = next((g for g in t.get("equivalents") or [] if p in g), [p])
                parent = config.cache_dir() / "repos" / t["repo"] / p
                gold = [f for f in (config.gold_dir(t["repo"]) / "files" / q for q in group) if f.is_file()]
                if not parent.is_file() or not gold:
                    continue
                seen += 1
                made = control.decoy_of(x["decoy"], parent.read_text(), gold[-1].read_text())
                self.assertNotIn(made, (parent.read_text(), gold[-1].read_text()), p)
                for c in x["criteria"]:  # what it breaks must not be decided by a pattern that passes
                    if c["id"] in x["decoy"]["breaks"] and {"absent", "present"} & set(c):
                        self.assertFalse(control.mechanical(c, made), (p, c["id"]))
        if not seen:
            self.skipTest("no checkouts here")

    def test_the_decoy_of_the_batch_tests_breaks_a_batch_test(self):
        """Its criterion is about the batch tests only."""
        x = json.loads((EVAL / "groundtruth/coverage_183.json").read_text())["units"]["jdbcclient-batch-entry-dsl"]
        x = next(v for p, v in x.items() if p.endswith("JdbcClientIntegrationTests.java"))
        self.assertIn("existing batch tests", x["criteria"][1]["text"])
        self.assertEqual(x["decoy"], {"breaks": ["c2"], "gold_with_a_test": "disabled", "test": "batchUpdateWithJdbcIndexParameters"})

    def test_a_decoy_of_a_test_file(self):
        gold = "class T {\n\n\t@Test\n\tvoid added() {\n\t\tx();\n\t}\n\n\t@Test\n\t@SuppressWarnings(\"x\")\n\tvoid kept() throws Exception {\n\t\ty();\n\t\tif (a) {\n\t\t}\n\t}\n}\n"
        parent = "class T {\n\t@Test\n\tvoid kept() {\n\t\ty();\n\t}\n}\n"
        off = control.decoy_of({"gold_with_a_test": "disabled"}, parent, gold)
        self.assertEqual(off.replace('\t@org.junit.jupiter.api.Disabled("to be adapted")\n', ""), gold)
        self.assertLess(off.index("added"), off.index("Disabled"))  # a test the parent has, not the new one
        empty = control.decoy_of({"gold_with_a_test": "emptied"}, parent, gold)
        self.assertIn("void kept() throws Exception {\n\t}\n}\n", empty)
        self.assertIn("x();", empty)
        self.assertIn("Disabled", control.decoy_of({"gold_with_a_test": "disabled"}, "class T { }", gold))  # renamed: any test
        with self.assertRaises(ValueError):
            control.decoy_of({"gold_with_a_test": "disabled"}, parent, "class T {}")
        with self.assertRaises(ValueError):
            control.decoy_of({"gold_with_a_test": "disabled", "test": "added"}, parent, gold)  # named, but not one of the parent's
        self.assertEqual(control.decoy_of({"gold_with_a_test": "disabled", "test": "kept"}, parent, gold), off)
        with self.assertRaises(ValueError):
            control.decoy_of({"replace": [["nowhere", "x"]]}, "a", "b")
        self.assertEqual(control.decoy_of({"replace": [["a", "b"]]}, "a a", "g"), "b a")

    def test_the_excerpt_shows_changes_the_commits_places_and_matches(self):
        before = "".join(f"line {i}\n" for i in range(400))
        left = before.replace("line 50\n", "line fifty\n")
        gold = before.replace("line 200\n", "line two hundred\n")
        text = control.excerpt(before, left, gold, [r"line 350$"])
        for want in ("line fifty", "line 200", "line 350", "line 347"):
            self.assertIn(want, text)
        for no in ("line 100", "line 300\n", "line 399"):
            self.assertNotIn(no, text)
        self.assertRegex(text, r"(?m)^   51  line fifty$")
        self.assertEqual(text.count("..."), 4)

    def _sheet(self, seed=5, **kw):
        tasks = [syn.task(i) for i in range(1, 21)]
        crit = {t["id"]: {u["path"]: {"wrong": "w", "criteria": [{"id": "c1", "text": "says it anew", "present": "new"}, {"id": "c2", "text": "reads well"}],
                                      "decoy": {"breaks": ["c2"], "replace": [["old", "newish but bad"]]}} for u in control.units(t)} for t in tasks}
        sets = {control.RECORDED: {t["id"]: syn.row(t, "none", touch=1) for t in tasks}, control.CONCURRENT: {t["id"]: syn.row(t, "none", touch=0) for t in tasks[:9]},
                control.NEW: {t["id"]: syn.row(t, "none-docs", touch=4) for t in tasks}}
        diff_of = lambda s, i: "".join(_diff(u["path"], "old\n", ("new" if s == control.NEW else "other") + "\n") for t in tasks if t["id"] == i for u in control.units(t))  # noqa: E731
        return control.sheet(sets, tasks, crit, diff_of, lambda i, p: "old\n", lambda i, p: "the new way\n", seed)

    def test_the_sheet_is_blind_and_complete(self):
        items, key = self._sheet()
        # Touched docs and forced tests only (no remote code, nothing untouched), and for each of the 60 units the commit's edit and a decoy.
        self.assertEqual({s: sum(k["set"] == s for k in key.values()) for s in ("recorded", "concurrent", "none-docs", "gold", "decoy")},
                         {"recorded": 20, "concurrent": 0, "none-docs": 60, "gold": 60, "decoy": 60})
        self.assertEqual([x["item"] for x in items], sorted(key))
        self.assertEqual(self._sheet(), (items, key))
        self.assertEqual(sorted(json.dumps({k: v for k, v in x.items() if k != "item"}) for x in self._sheet(seed=6)[0]),
                         sorted(json.dumps({k: v for k, v in x.items() if k != "item"}) for x in items))  # the second judge's: the same, in another order
        self.assertNotEqual([key[n]["set"] for n in sorted(key)], [k["set"] for _, k in sorted(self._sheet(seed=6)[1].items())])
        blob = json.dumps(items)
        for word in ("none-docs", "recorded", "concurrent", "mechanical", "absent", "decoy", "gold", "breaks", "__r1", "run_id", "index 1..2"):
            self.assertNotIn(word, blob)
        self.assertEqual({k for x in items for k in x}, {"item", "task", "unit", "change", "wrong_before", "criteria", "diff", "file_after_excerpt"})
        heads = {tuple(y[:4] for y in x["diff"].splitlines()[:3]) for x in items}
        self.assertEqual(heads, {("--- ", "+++ ", "@@ -")})  # one form, whoever made the edit
        order = [key[x["item"]]["set"] for x in items]
        self.assertNotEqual(order, sorted(order))

    def test_who_decides_what(self):
        items, key = self._sheet()
        good = lambda n: key[n]["set"] in (control.NEW, control.GOLD)  # noqa: E731
        judge = {n: {"c1": good(n), "c2": good(n)} for n in key}
        first = control.judged_units(key, judge, "first")
        self.assertEqual(control.judge_errors(first), [])
        self.assertEqual({k[0] for k, ok in first.items() if ok}, {control.NEW, control.GOLD})
        # A mechanical pass decides: the judge's no on c1 does not count where the pattern is there.
        self.assertTrue(all(control.judged_units(key, {n: {"c1": False, "c2": good(n)} for n in key}, "first")[k] for k in first if k[0] == control.GOLD))
        # A mechanical failure never decides: the judge's yes on c1 counts without the pattern.
        yes = {n: dict(x) for n, x in judge.items()}
        one = next(n for n in key if key[n]["set"] == control.RECORDED)
        self.assertIs(key[one]["mechanical"]["c1"], False)
        self.assertIsNone(key[one]["mechanical"]["c2"])
        yes[one] = {"c1": True, "c2": True}
        unit = (control.RECORDED, key[one]["task"], key[one]["unit"])
        self.assertTrue(control.judged_units(key, yes, "first")[unit])
        # A not covered goes to the second judge: agreed no, or undecided; a covered stands.
        self.assertIs(control.verdicts(first, first)[unit], False)
        self.assertIsNone(control.verdicts(first, {**first, unit: True})[unit])
        new = next(k for k in first if k[0] == control.NEW)
        self.assertIs(control.verdicts(first, {**first, new: False})[new], True)
        with self.assertRaises(SystemExit):
            control.verdicts(first, {k: v for k, v in first.items() if k != unit})
        for bad in ({n: x for n, x in judge.items() if n != one}, {**judge, "999": {}}, {**judge, one: {"c1": True}}, {**judge, one: {"c1": 1, "c2": "yes"}}):
            with self.assertRaises(SystemExit):
                control.judged_units(key, bad, "first")

    def test_a_judge_is_checked_on_the_commits_edits_and_the_decoys(self):
        items, key = self._sheet()
        good = lambda n: key[n]["set"] in (control.NEW, control.GOLD)  # noqa: E731
        of = lambda s, kind: [n for n in sorted(key) if key[n]["set"] == s and key[n]["kind"] == kind]  # noqa: E731

        def judge(wrong):
            return control.judged_units(key, {n: {"c1": good(n) != (n in wrong), "c2": good(n) != (n in wrong)} for n in key}, "second")
        tests = of(control.DECOY, "forced_tests")[:2] + of(control.GOLD, "forced_tests")[:2]
        docs = of(control.DECOY, "docs")[:2] + of(control.GOLD, "docs")[:2]
        self.assertEqual((control.JUDGE_ERRORS, control.JUDGE_ERRORS_DOCS), (5, 2))
        for wrong in (tests[:3] + docs[:2], tests + docs[:1], docs[1:3]):  # five, of them two on docs, are let pass, and said
            self.assertEqual(len(control.judge_errors(judge(wrong))), len(wrong))
        for wrong in (tests + docs[:2], docs[:3], tests[:1] + docs[1:]):  # six; three on docs, whether decoys or the commit's
            with self.assertRaisesRegex(SystemExit, f"second judge: {len(wrong)} wrong answers"):
                judge(wrong)
        sessions = [n for n in key if key[n]["set"] == control.RECORDED]
        self.assertEqual(control.judge_errors(judge(sessions)), [])  # the sessions' own items are no check

    def test_a_renamed_test_is_one_item_with_the_file_that_is_left(self):
        t = {**syn.task(1), "equivalents": [["m1/src/test/java/a/PTests.java", "m1/src/test/java/a/QTests.java"]]}
        crit = {"t01": {u["path"]: {"wrong": "w", "criteria": [{"id": "c1", "text": "x", "absent": "old"}], "decoy": {"breaks": ["c1"], "replace": [["old", "still old"]]}}
                        for u in control.units(t)}}
        old, new = t["equivalents"][0]
        r = syn.row(t, "none-docs", touch=0)
        r["edit"]["files_changed"] += [old, new]
        both = _diff(old, "old\n", "") + _diff(new, "", "fresh\n")
        items, key = control.sheet({control.NEW: {"t01": r}}, [t], crit, lambda s, i: both, lambda i, p: None if p == new else "old\n",
                                   lambda i, p: None if p == old else "commit\n")
        mine = [(x, key[x["item"]]) for x in items if x["unit"] == old]
        left = {k["set"]: (x["file_after_excerpt"].split("  ", 1)[-1], k["mechanical"]["c1"]) for x, k in mine}
        self.assertEqual({s: v[1] for s, v in left.items()}, {control.NEW: True, control.GOLD: True, control.DECOY: False})
        self.assertIn("fresh", left[control.NEW][0])
        session = next(x for x, k in mine if k["set"] == control.NEW)
        self.assertIn("+++ /dev/null", session["diff"])
        self.assertIn(f"+++ b/{new}", session["diff"])


class Comparison(unittest.TestCase):
    def setUp(self):
        self.tasks = [syn.task(i) for i in range(1, 21)]  # docs, docs, remote, forced test; every task has docs
        self.ids = [t["id"] for t in self.tasks]

    def rows(self, cond, touch, **kw):
        return [syn.row(t, cond, touch=touch, finished_at=f"2026-11-24T1{i % 10}:00:00", **kw) for i, t in enumerate(self.tasks)]

    def without_docs(self, rows, tasks=None):
        for r in rows:
            if tasks is None or r["task_id"] in tasks:
                r["edit"]["files_changed"] = [f for f in r["edit"]["files_changed"] if "adoc" not in f]
        return rows

    def compare(self, recorded, concurrent, new, covered=True, n=300):
        """``covered``: True (every touched unit is), False, or the units that are not (``(set, task, path)``)."""
        v = None
        if covered is not None:
            v = {(s, i, u["path"]): covered is True or covered is not False and (s, i, u["path"]) not in covered
                 for s in (control.RECORDED, control.CONCURRENT, control.NEW) for i, t in zip(self.ids, self.tasks) for u in control.units(t)}
        return control.compare(recorded, concurrent + new, self.tasks, v, n)

    def test_the_registered_rule(self):
        def S(points, iv=(0.6, 1.0), judged=True):  # touched, covered strict, covered lenient: (recorded, concurrent)
            ivs = iv if isinstance(iv, list) else [iv] * 3
            return control.outcome({k: {"recorded": a, "concurrent": b, "interval": x, "judged": judged or k == "touched"} for k, (a, b), x in zip(control.WAYS, points, ivs)})
        low = (0.0, 0.4)
        self.assertEqual(S([(0.9, 0.8), (0.7, 0.6), (0.8, 0.7)]), "i")
        self.assertEqual(S([(0.2, 0.1), (0.0, -0.3), (0.1, 0.0)], low), "ii")
        self.assertEqual(S([(0.9, 0.8), (0.3, 0.2), (0.3, 0.2)], [(0.6, 1.0), low, low]), "undecided")  # touched yes, covered no
        self.assertEqual(S([(0.9, 0.9), (0.4, 0.4), (0.9, 0.9)], [(0.6, 1.0), low, (0.6, 1.0)]), "undecided")  # it hangs on the undecided units
        self.assertEqual(S([(0.9, 0.4)] * 3), "undecided")  # the two bases disagree
        self.assertEqual(S([(0.9, 0.9)] * 3, [(0.5, 1.0), (0.6, 1.0), (0.6, 1.0)]), "undecided")  # an interval contains 50 %, at its end too
        self.assertEqual(S([(0.4, 0.4)] * 3, [(0.0, 0.4), (0.0, 0.4), (0.1, 0.5)]), "undecided")
        self.assertEqual(S([(0.5, 0.5)] * 3, low), "ii")  # exactly half is not most
        self.assertEqual(S([(0.9, None), (0.9, 0.9), (0.9, 0.9)]), "undecided")  # a base without a docs gap
        self.assertEqual(S([(0.9, 0.9), (None, None), (None, None)], judged=False), "unjudged")
        for k in ("i", "ii", "undecided", "unjudged"):
            self.assertIn(k, control.MEANING)
        self.assertIn("says nothing about tokens", control.MEANING["i"])
        self.assertNotIn("no main run", control.MEANING["i"])
        self.assertIn("Holm over four tests (C vs `none`, C vs `none-docs`; recall and tokens)", control.MEANING["ii"])

    def test_types_bases_and_outcome(self):
        rec, con = self.without_docs(self.rows("none", 4)), self.without_docs(self.rows("none", 4))
        new = self.without_docs(self.rows("none-docs", 4), self.ids[:4])  # 16 of 20 tasks find their docs
        c = self.compare(rec, con, new)
        self.assertEqual((c["summary"]["recorded"]["gap"]["docs"], c["summary"]["none-docs"]["gap"]["docs"]), (0.5, 0.1))
        for w in control.WAYS:
            x = c["docs_gap_closed"][w]
            self.assertAlmostEqual(x["recorded"], 0.8)
            self.assertAlmostEqual(x["concurrent"], 0.8)
            self.assertTrue(0.5 < x["interval"][0] < 0.8 < x["interval"][1] <= 1)
        self.assertEqual(c["outcome"], "i")
        self.assertAlmostEqual(c["difference"]["recall"]["diff_mean"], 0.4)  # none-docs minus none
        self.assertEqual(c["summary"]["none-docs"]["by_type"]["docs"], {"units": 40, "touched": 32, "tasks": 20, "tasks_all_touched": 16, "tasks_any_touched": 16})
        self.assertEqual([k for k, _ in c["tasks"]["t20"]["found_now"]], ["docs", "docs"])
        self.assertEqual(len(c["tasks"]["t01"]["missed_by_both"]), 2)
        text = "\n".join(control.lines(c))
        for want in ("Outcome (i)", "nothing is tested", SENTENCE, "drift", "| touched | 0.80 |", "| covered, undecided ones not | 0.80 |", "docs: units covered (undecided: not - too) | 0/40 | 0/40 | 32/40"):
            self.assertIn(want, text)
        self.assertNotIn("nan", text)
        # Found, but not covered: half of the new units.
        bad = {(control.NEW, i, f"framework-docs/{f}.adoc") for i in self.ids[4:] for f in "xy"}
        c = self.compare(rec, con, new, covered=bad)
        self.assertEqual((c["docs_gap_closed"]["covered_strict"]["recorded"], c["outcome"]), (0.0, "undecided"))
        # The same units undecided between the judges: counted both ways, so the outcome hangs on them.
        v = {(s, i, u["path"]): None if (s, i, u["path"]) in bad else True for s in (control.RECORDED, control.CONCURRENT, control.NEW)
             for i, t in zip(self.ids, self.tasks) for u in control.units(t)}
        c = control.compare(rec, con + new, self.tasks, v, 100)
        self.assertEqual([round(c["docs_gap_closed"][w]["recorded"], 6) for w in control.WAYS], [0.8, 0.0, 0.8])
        self.assertEqual((c["outcome"], len(c["tasks"]["t20"]["undecided_units"]["none-docs"])), ("undecided", 2))
        self.assertIn("docs: units covered (undecided: not - too) | 0/40 | 0/40 | 0-32/40", "\n".join(control.lines(c)))
        self.assertEqual(self.compare(rec, con, new, covered=None)["outcome"], "unjudged")
        self.assertIn("No outcome yet", "\n".join(control.lines(self.compare(rec, con, new, covered=None))))
        c = self.compare(rec, con, self.without_docs(self.rows("none-docs", 4)))
        self.assertEqual((c["outcome"], c["docs_gap_closed"]["touched"]["recorded"]), ("ii", 0.0))
        c = self.compare(rec, self.rows("none", 4), new)  # the concurrent none found everything: no gap to close there
        self.assertEqual((c["outcome"], c["docs_gap_closed"]["touched"]["concurrent"]), ("undecided", None))
        self.assertEqual(len(c["tasks"]["t07"]["flipped_in_none"]), 2)  # noise and drift are seen
        with self.assertRaisesRegex(SystemExit, "no verdict"):
            control.compare(rec, con + new, self.tasks, {}, 10)

    def test_over_editing_by_type(self):
        rec, con, new = self.rows("none", 4), self.rows("none", 4), self.rows("none-docs", 4)
        new[0]["edit"]["files_changed"] += ["framework-docs/z.adoc", "README.md", "m1/src/test/java/a/XTests.java", "m1/src/main/java/z/Z.java"]
        new[1]["edit"]["files_changed"] += ["framework-docs/z.adoc", "m2/src/test/java/a/MoreTests.java"]  # the second is in the commit: a new test
        c = self.compare(rec, con, new, n=50)
        self.assertEqual(c["summary"]["none-docs"]["extra"], {"docs": {"files": 3, "sessions": 2}, "tests": {"files": 1, "sessions": 1}, "code": {"files": 1, "sessions": 1}})
        self.assertEqual(c["tasks"]["t01"]["extra_files"]["docs"], ["README.md", "framework-docs/z.adoc"])
        self.assertEqual((c["summary"]["none-docs"]["new_tests_touched"], c["summary"]["recorded"]["new_tests_touched"]), (1, 0))
        t = {**self.tasks[0], "equivalents": [["m1/src/main/java/b/R.java", "m1/src/main/java/b/Other.java"]]}
        self.assertEqual(control.extra_files(t, ["m1/src/main/java/b/Other.java"]), {"docs": [], "tests": [], "code": []})

    def test_only_the_registered_sessions(self):
        rec, con, new = self.rows("none", 4), self.rows("none", 4), self.rows("none-docs", 4)
        bad = ((rec[:-1], con, new), (rec, con[:-1], new), (rec, con, new[:-1]), (rec, con, new + new[:1]), (new, con, new), (rec, con, [{**new[0], "rep": 2}, *new[1:]]),
               (rec, con, [{**new[0], "final": False}, *new[1:]]), (rec, con, [{**r, "claude_version": "2.1.400"} for r in new]),
               (rec, [{**r, "model": "other"} for r in con], new), (rec, [{**con[0], "condition": "C"}, *con[1:]], new))
        for x in bad:
            with self.assertRaises(SystemExit):
                self.compare(*x, n=10)

    def test_against_the_recorded_sessions(self):
        rec = [json.loads(x) for x in (EVAL / "results/none20-183/sessions.jsonl").read_text().splitlines()]
        tasks = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())["tasks"]
        nine = control.docs_tasks(tasks)
        run = [r for r in rec if r["task_id"] in nine] + [{**r, "condition": "none-docs"} for r in rec]
        c = control.compare(rec, run, tasks, None, 100)
        s = c["summary"]["recorded"]
        self.assertEqual({k: round(v, 5) for k, v in s["gap"].items()}, {"docs": 0.25375, "remote": 0.005, "forced_tests": 0.08})  # the gap the rule is about
        self.assertEqual((s["by_type"]["docs"]["touched"], s["by_type"]["docs"]["units"], s["by_type"]["docs"]["tasks_all_touched"], s["by_type"]["docs"]["tasks"]), (2, 13, 2, 9))
        self.assertEqual(s["extra"], {"docs": {"files": 0, "sessions": 0}, "tests": {"files": 0, "sessions": 0}, "code": {"files": 2, "sessions": 1}})
        self.assertEqual((s["new_tests_touched"], s["optional_touched"]), (3, 0))
        self.assertEqual((c["docs_gap_closed"]["touched"]["recorded"], c["docs_gap_closed"]["touched"]["concurrent"], c["outcome"]), (0.0, 0.0, "unjudged"))
        self.assertIn("docs", control.lines(c)[0] + "".join(control.lines(c)))


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
        for part in (rows[:1], [r for r in rows if r["condition"] != "D"], [], [{"condition": "A"}]):  # an arm is missing or foreign: refused, no silent default
            with self.assertRaisesRegex(SystemExit, "no registered design"):
                rc.design_of(part)
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
        words = shlex.split(next(x for x in out.stdout.splitlines() if x.startswith("+")))
        for drop in ("--hook-cd",):
            words.remove(drop)
        got = {w: words[i + 1] for i, w in enumerate(words[:-1]) if w.startswith("--")}
        self.assertEqual((got.pop("--conditions"), got.pop("--label")), ("none,none-docs", "2026-11-24-control-none-docs"))
        self.assertEqual({k: v for k, v in got.items() if k in ("--profile", "--timeout", "--concurrency", "--max-sessions")},
                         {"--profile": "realistic", "--timeout": "2400", "--concurrency": "1", "--max-sessions": "35"})
        self.assertTrue(got["--plan"].endswith("/groundtruth/control_plan_183.json"))
        self.assertNotIn("--rep", words)
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
