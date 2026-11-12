import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, report  # noqa: E402
from aeval.runner import FORMAT_RULES, RunConfig, Runner, build_prompt  # noqa: E402


class TaskFile(unittest.TestCase):
    """The committed task set itself (schema only; evidence needs the checkouts)."""

    def test_schema(self):
        from aeval.prep import check_tasks
        tasks = config.load_tasks()
        self.assertGreaterEqual(len(tasks), 30)
        self.assertEqual(check_tasks(tasks, config.load_repos(), check_evidence=False), [])

    def test_every_task_type_and_repo_is_covered(self):
        tasks = config.load_tasks()
        self.assertEqual({t["type"] for t in tasks},
                         {"callers", "callees", "impact", "where", "dead_code", "flow"})
        self.assertEqual({t["repo"] for t in tasks}, set(config.load_repos()))


class Plan(unittest.TestCase):
    def setUp(self):
        self.tasks = config.load_tasks()[:4]
        self.runner = Runner(RunConfig(label="unit", reps=3), self.tasks, config.load_repos())

    def test_plan_is_rep_major_with_paired_conditions(self):
        plan = self.runner.plan()
        self.assertEqual(len(plan), 4 * 2 * 3)
        reps = [it.rep for it in plan]
        self.assertEqual(reps, sorted(reps))
        for a, b in zip(plan[::2], plan[1::2]):
            self.assertEqual((a.task["id"], a.rep), (b.task["id"], b.rep))
            self.assertEqual({a.condition, b.condition}, {"with", "without"})
        self.assertEqual(len({it.run_id for it in plan}), len(plan))

    def test_plan_is_deterministic(self):
        again = Runner(RunConfig(label="unit", reps=3), self.tasks, config.load_repos()).plan()
        self.assertEqual([it.run_id for it in self.runner.plan()], [it.run_id for it in again])

    def test_command_differs_only_by_the_mcp_permission(self):
        from aeval.runner import WorkItem
        t = self.tasks[0]
        with_cmd = self.runner._command(WorkItem(t, "with", 1), Path("/x/mcp.json"))
        without_cmd = self.runner._command(WorkItem(t, "without", 1), Path("/x/mcp.json"))
        diff = [(a, b) for a, b in zip(with_cmd, without_cmd) if a != b]
        self.assertEqual(len(with_cmd), len(without_cmd))
        self.assertEqual(len(diff), 1)
        self.assertIn("mcp__myceliums", diff[0][0])

    def test_prompt_mentions_neither_condition(self):
        repos = config.load_repos()
        for t in config.load_tasks():
            p = build_prompt(t, repos[t["repo"]])
            self.assertNotIn("myceliums", p.lower())
            self.assertIn(t["question"], p)
            self.assertIn(FORMAT_RULES[t["answer_kind"]], p)


def _row(task_id, cond, rep, f1, tokens):
    return {"run_id": f"{task_id}__{cond}__r{rep}", "task_id": task_id, "repo": "fzf", "type": "callers",
            "condition": cond, "rep": rep, "attempt": 1, "status": "ok", "answer": [], "f1": f1,
            "precision": f1, "recall": f1, "success": f1 >= 0.8, "tp": 0, "fp": 0, "neutral": 0,
            "n_gold": 1, "n_answer": 1, "missed": [], "false_positives": [], "num_turns": 3,
            "tool_calls": 4, "tool_calls_by_name": {"Grep": 3, "mcp__myceliums__get_callers": 1} if cond == "with" else {"Grep": 4},
            "mcp_tool_calls": 1 if cond == "with" else 0, "mcp_tools_used": [], "tool_errors": {},
            "first_tool": "Grep", "n_tools_offered": 4, "n_mcp_tools_offered": 0, "input_tokens": 1,
            "cache_creation_input_tokens": 1, "cache_read_input_tokens": 1, "total_input_tokens": tokens,
            "output_tokens": 10, "cost_usd": 0.01, "duration_ms": 1000, "duration_api_ms": 900, "wall_s": 2.0,
            "model": "m", "models_used": ["m"], "claude_version": "x", "finished_at": "2026-11-12T00:00:00+00:00"}


class Report(unittest.TestCase):
    def test_render_is_deterministic_and_check_mode_works(self):
        tasks = config.load_tasks()
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            report.config.RESULTS_DIR = Path(tmp)
            try:
                d = Path(tmp) / "lbl"
                d.mkdir()
                rows = []
                for i, t in enumerate(tasks[:3]):
                    for rep in (1, 2):
                        rows.append(_row(t["id"], "with", rep, 1.0, 1000 + i))
                        rows.append(_row(t["id"], "without", rep, 0.5, 2000 + i))
                (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (d / "meta.json").write_text(json.dumps({"success_threshold": 0.8, "reps": 2, "repos": {},
                                                         "attempts_total": 12}))
                snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                             "n_gold": len(t["gold"]), "question": t["question"]} for t in tasks]
                (d / "tasks_snapshot.json").write_text(json.dumps(snapshot))
                self.assertTrue(report.write_report("lbl"))
                self.assertTrue(report.write_report("lbl", check=True))
                text = (d / "REPORT.md").read_text()
                self.assertIn("Paired over 3 tasks", text)
                self.assertIn("**+0.500**", text)  # mean F1 difference
                summary = json.loads((d / "summary.json").read_text())
                self.assertAlmostEqual(summary["analysis"]["overall"]["total_input_tokens"]["diff_mean"], -1000)
                (d / "REPORT.md").write_text(text + "tampered\n")
                self.assertFalse(report.write_report("lbl", check=True))
            finally:
                config.RESULTS_DIR = old
                report.config.RESULTS_DIR = old


if __name__ == "__main__":
    unittest.main()
