"""#182: the interleaved schedule, the mock-API run mode and the Java task set."""

import argparse
import json
import os
import re
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import agent_eval  # noqa: E402
from aeval import config, mockapi  # noqa: E402
from aeval.grading import grade  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import RunConfig, Runner, StopRun, WorkItem, build_prompt  # noqa: E402

JAVA_TASKS = config.EVAL_DIR / "tasks" / "tasks-java.json"
ARMS = ("none", "C", "D")


def _tasks(n: int) -> list[dict]:
    return [{"id": f"t{i}", "repo": "fzf", "type": "callers", "answer_kind": "symbols", "question": f"q{i}",
             "gold": [], "rationale": "r"} for i in range(n)]


def _runner(n_tasks: int = 30, schedule: str = "interleaved", reps: int = 3, seed: int = 20261112,
            conditions=ARMS) -> Runner:
    cfg = RunConfig(label="unit-schedule", reps=reps, conditions=tuple(conditions), schedule=schedule, seed=seed)
    return Runner(cfg, _tasks(n_tasks), config.load_repos())


class InterleavedSchedule(unittest.TestCase):
    def test_blocks_hold_every_arm_of_one_task(self):
        plan = _runner().plan()
        self.assertEqual(len(plan), 30 * 3 * 3)
        self.assertEqual(len({it.run_id for it in plan}), len(plan))
        self.assertEqual([it.rep for it in plan], sorted(it.rep for it in plan))  # repetition-major
        for i in range(0, len(plan), 3):
            block = plan[i:i + 3]
            self.assertEqual({it.condition for it in block}, set(ARMS))
            self.assertEqual(len({(it.task["id"], it.rep) for it in block}), 1)

    def test_each_arm_takes_each_position_equally_often_per_repetition(self):
        for seed in (1, 2, 20261112):
            plan = _runner(seed=seed).plan()
            for rep in (1, 2, 3):
                rows = [it for it in plan if it.rep == rep]
                pos = Counter((it.condition, i % 3) for i, it in enumerate(rows))
                self.assertEqual(set(pos.values()), {10}, (seed, rep, pos))

    def test_uneven_task_count_is_balanced_within_one(self):
        plan = _runner(n_tasks=31).plan()
        rows = [it for it in plan if it.rep == 1]
        pos = Counter((it.condition, i % 3) for i, it in enumerate(rows))
        self.assertLessEqual(max(pos.values()) - min(pos.values()), 1)

    def test_same_sessions_as_paired_and_deterministic(self):
        inter = [it.run_id for it in _runner().plan()]
        paired = [it.run_id for it in _runner(schedule="paired").plan()]
        self.assertEqual(sorted(inter), sorted(paired))
        self.assertEqual(inter, [it.run_id for it in _runner().plan()])

    def test_paired_order_is_unchanged_by_the_new_field(self):
        # runs before #182 keep their order: the default schedule draws exactly as before
        tasks = _tasks(5)
        old = Runner(RunConfig(label="x", reps=2, conditions=("none", "C")), tasks, config.load_repos()).plan()
        new = Runner(RunConfig(label="x", reps=2, conditions=("none", "C"), schedule="paired"), tasks,
                     config.load_repos()).plan()
        self.assertEqual([it.run_id for it in old], [it.run_id for it in new])

    def test_config_records_the_schedule_only_when_not_the_default(self):
        self.assertNotIn("schedule", RunConfig(label="x").comparable())
        self.assertNotIn("mock_api", RunConfig(label="x").comparable())
        self.assertEqual(RunConfig(label="x", schedule="interleaved").comparable()["schedule"], "interleaved")
        self.assertTrue(RunConfig(label="x", mock_api=True).comparable()["mock_api"])

    def test_interleaved_run_refuses_a_subset_of_its_arms(self):
        runner = _runner()
        with self.assertRaises(SystemExit):
            runner.run(conditions=["C"])
        self.assertFalse(runner.run_dir.exists())  # refused before anything was written


class MixedVersionsAndHookPath(unittest.TestCase):
    """#182 review: a CLI update during a run is disclosed; D's refusals do not show where the gold lies."""

    def test_mixed_claude_versions(self):
        row = lambda i, c, v, rep, final=True: {"run_id": i, "condition": c, "claude_version": v,  # noqa: E731
                                                "rep": rep, "final": final}
        rows = [row("a", "none", "1", 1), row("b", "D", "1", 1), row("c", "D", "2", 3), row("d", "C", "3", 1, False)]
        self.assertEqual(agent_eval.mixed_claude_versions(rows),
                         {"1": {"sessions": {"none": 1, "D": 1}, "reps": [1]}, "2": {"sessions": {"D": 1}, "reps": [3]}})
        self.assertEqual(agent_eval.mixed_claude_versions(rows[:2]), {})

    def test_the_hook_runs_from_a_neutral_copy(self):
        import subprocess
        r = _runner(1)
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory(prefix="hook-") as hd:
            s = json.loads(r._nosearch_settings(Path(tmp), None, Path(hd)).read_text())
            cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            self.assertNotIn(str(config.EVAL_DIR), cmd)
            self.assertNotIn(str(config.cache_dir()), cmd)
            for command, rc in (("grep -rn x .", 2), ("git status", 0)):
                ev = json.dumps({"tool_name": "Bash", "tool_input": {"command": command}})
                p = subprocess.run(cmd, shell=True, input=ev, capture_output=True, text=True)
                self.assertEqual(p.returncode, rc, p.stderr)
                self.assertNotIn(str(config.EVAL_DIR), p.stderr)


class UsageLimit(unittest.TestCase):
    """A session the account's limit cut off is never final: it would count as its arm's failure."""

    def _status(self, *events, timed_out=False, returncode=0):
        r = _runner(1)
        task = dict(r.tasks[0], answer_kind="symbols", gold=[{"name": "A.b"}])
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "t.jsonl"
            p.write_text("".join(json.dumps(e) + "\n" for e in events))
            row = r._summarise(WorkItem(task, "none", 1), 1, p, 1.0, timed_out, returncode)
        return row["status"], row["final"]

    @staticmethod
    def _result(text, is_error=True):
        return {"type": "result", "subtype": "success", "is_error": is_error, "result": text, "num_turns": 1}

    def test_the_clis_messages(self):
        for text in ("You've hit your session limit \u00b7 resets 7pm", "You've hit your weekly limit",
                     "You've reached your Fable 5 limit", "You're out of usage credits", "Usage limit reached"):
            self.assertEqual(self._status(self._result(text)), ("usage_limit", False), text)

    def test_a_limit_that_ends_in_the_timeout(self):
        ev = {"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}}
        self.assertEqual(self._status(ev, timed_out=True), ("usage_limit", False))

    def test_a_session_killed_from_outside_is_retried(self):
        res = {"type": "result", "subtype": "error_during_execution", "is_error": True, "result": ""}
        for code in (-9, -15, 137, 143):
            self.assertEqual(self._status(res, returncode=code), ("crash", False), code)
            self.assertEqual(self._status(returncode=code), ("crash", False), code)
        self.assertEqual(self._status(res, returncode=1), ("error", True))

    def test_ordinary_endings_stay_final(self):
        self.assertEqual(self._status(timed_out=True), ("timeout", True))
        warn = {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed_warning", "utilization": 0.9}}
        self.assertEqual(self._status(warn, self._result("the limit reached here is fine", False)),
                         ("no_answer", True))


class UnembeddedIndex(unittest.TestCase):
    """An index built with `index --skip-embeddings` serves mock-API checks only."""

    def _runner(self, cache: Path, **kw) -> Runner:
        home = cache / "index" / "fzf" / "home" / ".myceliums"
        home.mkdir(parents=True)
        (home / "repos.json").write_text(json.dumps({"repos": {"x": {"analyzed_at": "then"}}}))
        (cache / "index" / "fzf" / "meta.json").write_text(json.dumps({"repo": "fzf", "embeddings": "skipped"}))
        old = os.environ.get("AEVAL_CACHE")
        os.environ["AEVAL_CACHE"] = str(cache)
        try:
            return Runner(RunConfig(label="unit-index", **kw), _tasks(1), config.load_repos())
        finally:
            if old is None:
                os.environ.pop("AEVAL_CACHE")
            else:
                os.environ["AEVAL_CACHE"] = old

    def test_real_run_refuses_it(self):
        with tempfile.TemporaryDirectory() as d:
            runner = self._runner(Path(d) / "cache")
            (Path(d) / "s").mkdir()
            with self.assertRaises(StopRun):
                runner._copy_index("fzf", Path(d) / "s")

    def test_mock_run_may_use_it(self):
        with tempfile.TemporaryDirectory() as d:
            runner = self._runner(Path(d) / "cache", mock_api=True)
            (Path(d) / "s").mkdir()
            data, _ = runner._copy_index("fzf", Path(d) / "s")
            self.assertTrue((data / "repos.json").exists())


def _args(**kw) -> argparse.Namespace:
    base = dict(label="x", model="sonnet", effort="medium", max_turns=40, timeout=900, reps=3, concurrency=1,
                max_retries=0, threshold=0.8, max_utilization=0.97, max_sessions=None, seed=1, tool_search=None,
                claude="claude", myc="", models_dir="", conditions="none,C,D", prompt_budget=False,
                bash=True, profile=None, schedule=None)
    base.update(kw)
    return argparse.Namespace(**base)


class Profile(unittest.TestCase):
    def test_realistic_profile_runs_interleaved(self):
        cfg = agent_eval._run_config(_args(profile="realistic"))
        self.assertEqual((cfg.max_turns, cfg.reps, cfg.schedule, cfg.prompt_budget), (100, 3, "interleaved", False))

    def test_profile_and_a_conflicting_schedule_are_an_error(self):
        with self.assertRaises(SystemExit):
            agent_eval._run_config(_args(profile="realistic", schedule="paired"))

    def test_default_schedule_is_paired(self):
        self.assertEqual(agent_eval._run_config(_args()).schedule, "paired")


class MockAnswers(unittest.TestCase):
    def test_final_answer_is_the_one_for_the_prompt_question(self):
        with tempfile.TemporaryDirectory() as d:
            api = mockapi.MockAPI(Path(d), "x", "y", answers={"Which A?": ["A.a"], "Which B?": ["B.b", "B.c"]})
            body = {"messages": [{"role": "user", "content": [{"type": "text", "text": "Question: Which B?"}]}]}
            self.assertEqual(api.answer_for(body), ["B.b", "B.c"])
            self.assertEqual(api.answer_for({"messages": [{"role": "user", "content": "nothing"}]}), [])

    def test_answer_block_is_graded_like_a_model_answer(self):
        from aeval.grading import extract_answer
        with tempfile.TemporaryDirectory() as d:
            api = mockapi.MockAPI(Path(d), "x", "y", answers={"Q?": ["Foo.bar"]})
            raw = api.answer({"model": "m", "tools": [{"name": "Read"}], "messages": [
                {"role": "user", "content": "Q?"}, {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "a"}, {"type": "tool_result", "tool_use_id": "b"},
                    {"type": "tool_result", "tool_use_id": "c"}]}]}).decode()
            text = "".join(json.loads(line[6:])["delta"]["text"] for line in raw.splitlines()
                           if line.startswith("data: ") and '"text_delta"' in line)
            self.assertEqual(extract_answer(text), ["Foo.bar"])


class JavaTaskSet(unittest.TestCase):
    """The committed #182 set: schema, mix, review record, and that its own gold grades as a success."""

    @classmethod
    def setUpClass(cls):
        cls.data = json.loads(JAVA_TASKS.read_text())
        cls.tasks = cls.data["tasks"]

    def test_schema(self):
        self.assertEqual(check_tasks(self.tasks, config.load_repos(), check_evidence=False), [])
        self.assertEqual(self.data["labelled_by"], "agent")
        self.assertEqual({t["repo"] for t in self.tasks}, {"spring-framework"})
        self.assertEqual(config.load_repos()["spring-framework"]["commit"], self.data["repo_commit"])

    def test_mix(self):
        self.assertEqual(len(self.tasks), 30)
        self.assertEqual(Counter(t["category"] for t in self.tasks),
                         {"concept": 6, "deep_transitive": 6, "dispatch": 6, "by_name": 6, "docs": 6})
        for t in self.tasks:
            self.assertTrue(t.get("gold_method"), t["id"])
            self.assertIn("review", t, t["id"])

    def test_prompts_never_name_a_tool(self):
        # "index" is not banned: SpEL's index accessors are a domain term of one question
        banned = re.compile(r"\b(myceliums|myc|grep|rg|mcp|embeddings?|semantic|graph|knowledge)\b")
        for t in self.tasks:
            prompt = build_prompt(t, config.load_repos()["spring-framework"]).lower()
            self.assertIsNone(banned.search(prompt), t["id"])

    def test_gold_grades_as_full_success(self):
        for t in self.tasks:
            g = grade(t, [x["name"] for x in t["gold"]])
            self.assertEqual(g.f1, 1.0, t["id"])
            g = grade(t, [x["name"] for x in t["gold"]] + [o["name"] for o in t.get("optional", [])])
            self.assertEqual(g.f1, 1.0, t["id"])  # optional answers are neutral


class ThreeArmReports(unittest.TestCase):
    """#182: the token report and the variant report cover every arm, not just the first two."""

    @staticmethod
    def _row(task: str, cond: str, myc: float) -> dict:
        tok = {"base": 1000.0, "search": 0.0 if cond == "D" else 500.0, "myceliums": myc, "output": 100.0}
        return {"attribution_method": "usage-delta", "attribution_residual": {"input_tokens": 0},
                "condition": cond, "task_id": task, "rep": 1, "run_id": f"{task}__{cond}__r1", "success": True,
                "tokens_by_category": tok, "result_tokens_by_category": {}, "cost_by_category": {"base": 0.001},
                "cost_by_class": {"cache_read": 0.001}, "priced_cost_usd": 0.001, "cost_usd": 0.001,
                "models": ["claude-sonnet-5-5"], "total_input_tokens": int(sum(tok.values()) - 100),
                "search_audit": {"bash_blocked": 0, "bash_calls": 0, "blind_reads": 0, "files_read": 0,
                                 "flagged": False, "grep_glob_calls": 0, "interpreted_calls": 0,
                                 "interpreted_ran": 0, "max_reads_per_turn": 0, "reads": 0, "search_attempts": 0,
                                 "search_blocked": 0, "search_leaks": 0}}

    def test_token_report_has_a_section_per_arm(self):
        from aeval import report_tokens
        with tempfile.TemporaryDirectory() as d:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(d)
            try:
                res = Path(d) / "three"
                res.mkdir()
                (res / "meta.json").write_text(json.dumps({"conditions": ["none", "C", "D"]}))
                rows = [self._row(t, c, 0.0 if c == "none" else 300.0) for t in ("a", "b", "c") for c in ARMS]
                (res / "tokens.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                text, summary = report_tokens.render("three", n_resamples=200)
            finally:
                config.RESULTS_DIR = old
        self.assertIn("## All sessions: `C` against `none`", text)
        self.assertIn("## All sessions: `D` against `none`", text)
        self.assertEqual(summary["arms"], ["C", "D"])
        self.assertEqual(set(summary["analysis_by_arm"]), {"C", "D"})

    def test_myc_runs_from_bash_count_where_the_session_had_myceliums(self):
        from aeval.report_variants import _myc_calls
        self.assertEqual(_myc_calls({"condition": "D", "mcp_tool_calls": 1, "myc_cli_calls": 2}), 3)
        self.assertEqual(_myc_calls({"condition": "C", "mcp_tool_calls": 0, "myc_cli_calls": 1}), 1)
        self.assertEqual(_myc_calls({"condition": "none", "mcp_tool_calls": 0, "myc_cli_calls": 1}), 0)
        self.assertEqual(_myc_calls({"condition": "A", "mcp_tool_calls": 2}), 2)  # rows before #163


class PairwiseReport(unittest.TestCase):
    """#182: an interleaved run's report compares every pair (McNemar, bootstrap, spend per solved task,
    multiplicity) and gives recall and success per category; earlier runs keep their baseline report."""

    @staticmethod
    def _row(task: str, cond: str, rep: int, ok: bool) -> dict:
        return {"run_id": f"{task}__{cond}__r{rep}", "task_id": task, "condition": cond, "rep": rep,
                "status": "ok", "success": ok, "f1": 1.0 if ok else 0.5, "precision": 1.0,
                "recall": 1.0 if ok else 0.4, "total_input_tokens": 1000, "first_turn_input_tokens": 100,
                "output_tokens": 50, "cost_usd": 0.01, "tool_calls": 4, "num_turns": 5, "wall_s": 10.0,
                "mcp_tool_calls": 0 if cond == "none" else 2, "myc_cli_calls": 0, "tool_calls_by_name": {"Read": 2},
                "tool_errors": {}, "first_tool": "Read", "subagent_mcp_tool_calls": 0}

    def _render(self, comparisons: str | None):
        from aeval import report_variants
        fails = {"none": set(), "C": {"t0", "t1"}, "D": {"t0", "t1", "t2", "t3"}}
        rows = [self._row(f"t{i}", c, r, f"t{i}" not in fails[c]) for i in range(6) for c in ARMS for r in (1, 2, 3)]
        tasks = [{"id": f"t{i}", "repo": "fzf", "type": "callers", "answer_kind": "symbols", "n_gold": 3,
                  "question": "q", "category": "concept" if i < 3 else "docs"} for i in range(6)]
        meta = {"conditions": list(ARMS), "success_threshold": 0.8, "reps": 3, "bash": True,
                **({"comparisons": comparisons} if comparisons else {})}
        with tempfile.TemporaryDirectory() as d:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(d)
            try:
                res = Path(d) / "pw"
                res.mkdir()
                (res / "meta.json").write_text(json.dumps(meta))
                (res / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (res / "tasks_snapshot.json").write_text(json.dumps(tasks))
                return report_variants.render("pw", n_resamples=200)
            finally:
                config.RESULTS_DIR = old

    def test_every_pair_without_a_baseline(self):
        text, summary = self._render("pairwise")
        self.assertIsNone(summary["baseline"])
        pairs = {(p["a"], p["b"]): p for p in summary["pairwise"]["pairs"]}
        self.assertEqual(set(pairs), {("none", "C"), ("none", "D"), ("C", "D")})
        self.assertEqual((pairs[("none", "C")]["wins"], pairs[("none", "C")]["losses"]), (2, 0))
        self.assertEqual((pairs[("none", "D")]["wins"], pairs[("none", "D")]["losses"]), (4, 0))
        self.assertEqual((pairs[("C", "D")]["wins"], pairs[("C", "D")]["losses"]), (2, 0))
        self.assertAlmostEqual(pairs[("none", "D")]["mcnemar_p"], 0.125)
        self.assertGreaterEqual(pairs[("none", "C")]["mcnemar_p_holm"], pairs[("none", "C")]["mcnemar_p"])
        # D solved 2 of 6 tasks (6 sessions) against none's 18 sessions, at the same spend per session
        self.assertAlmostEqual(pairs[("none", "D")]["tokens_per_solved"]["ratio"], 6 / 18)
        self.assertNotIn("Paired differences vs", text)
        self.assertIn("## Pairwise comparisons", text)
        self.assertIn("**Multiplicity.**", text)
        cats = summary["by_category"]
        self.assertEqual(cats["concept"]["D"]["success"], 0.0)
        self.assertEqual(cats["docs"]["D"]["success"], 2 / 3)
        self.assertAlmostEqual(cats["concept"]["C"]["recall"], (0.4 + 0.4 + 1.0) / 3)

    def test_runs_without_the_flag_keep_the_baseline_report(self):
        text, summary = self._render(None)
        self.assertEqual(summary["baseline"], "none")
        self.assertIn("Paired differences vs none", text)
        self.assertNotIn("Pairwise comparisons", text)


class RequireOwner(unittest.TestCase):
    """#182: a task with ``require_owner`` does not accept a bare method name."""

    TASK = {"id": "x", "repo": "spring-framework", "answer_kind": "symbols", "require_owner": True,
            "gold": [{"name": "RequestMappingInfoHandlerMapping.getMappingComparator"}],
            "optional": [{"name": "RequestMappingInfo.compareTo"}]}

    def test_bare_name_is_a_false_positive(self):
        g = grade(self.TASK, ["getMappingComparator"])
        self.assertEqual((g.tp, g.fp), (0, 1))
        g = grade(self.TASK, ["compareTo"])  # not even neutral
        self.assertEqual((g.neutral, g.fp), (0, 1))

    def test_owner_and_qualified_forms_match(self):
        for ans in ("RequestMappingInfoHandlerMapping.getMappingComparator",
                    "org.springframework.web.servlet.mvc.method.RequestMappingInfoHandlerMapping#getMappingComparator",
                    "RequestMappingInfoHandlerMapping.getMappingComparator(HttpServletRequest)"):
            self.assertEqual(grade(self.TASK, [ans]).f1, 1.0, ans)

    def test_other_sets_still_accept_bare_names(self):
        task = {k: v for k, v in self.TASK.items() if k != "require_owner"}
        self.assertEqual(grade(task, ["getMappingComparator"]).f1, 1.0)

    def test_every_java_symbol_task_requires_the_owner(self):
        tasks = json.loads(JAVA_TASKS.read_text())["tasks"]
        self.assertTrue(all(t.get("require_owner") for t in tasks if t["answer_kind"] == "symbols"))


if __name__ == "__main__":
    unittest.main()
