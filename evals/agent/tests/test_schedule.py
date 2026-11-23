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
from aeval.runner import RunConfig, Runner, StopRun, build_prompt  # noqa: E402

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


if __name__ == "__main__":
    unittest.main()
