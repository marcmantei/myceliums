"""The outcome eval (#153): the Django task set, the budgeted prompt, the
ground-truth edges for function-local and star imports, and the report of a
`none` vs `C` run."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, report  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import BUDGET_RULES, RunConfig, build_prompt  # noqa: E402

OUTCOME_TASKS = config.EVAL_DIR / "tasks" / "tasks-outcome.json"
PY_EDGES = config.EVAL_DIR / "groundtruth" / "py_edges.py"


class OutcomeTasks(unittest.TestCase):
    def setUp(self):
        self.tasks = config.load_json(OUTCOME_TASKS)["tasks"]

    def test_schema(self):
        self.assertEqual(check_tasks(self.tasks, config.load_repos(), check_evidence=False), [])

    def test_twelve_django_tasks_four_per_type(self):
        self.assertEqual(len(self.tasks), 12)
        self.assertEqual({t["repo"] for t in self.tasks}, {"django"})
        by_type = {}
        for t in self.tasks:
            by_type[t["type"]] = by_type.get(t["type"], 0) + 1
        self.assertEqual(by_type, {"transitive_callers": 4, "blast_radius": 4, "ambiguous": 4})

    def test_sizes_match_the_levers(self):
        for t in self.tasks:
            if t["type"] == "blast_radius":
                self.assertGreaterEqual(len(t["gold"]), 15, t["id"])
            if t["type"] == "transitive_callers":
                self.assertTrue(any("level 3" in g["note"] for g in t["gold"]), t["id"])

    def test_django_is_pinned(self):
        dj = config.load_repos()["django"]
        self.assertEqual(dj["ref"], "5.1.4")
        self.assertEqual(dj["commit"], "2d4add11fd57b05f7ea48e8b3e89e743c9871aa3")


class BudgetPrompt(unittest.TestCase):
    def setUp(self):
        self.task = config.load_json(OUTCOME_TASKS)["tasks"][0]
        self.repo = config.load_repos()["django"]

    def test_budget_text_only_when_asked(self):
        plain = build_prompt(self.task, self.repo)
        budget = build_prompt(self.task, self.repo, 4)
        self.assertNotIn("Budget:", plain)
        self.assertIn(BUDGET_RULES.format(max_turns=4, tool_turns=3), budget)
        self.assertIn("at most 3 replies with tool calls", budget)
        self.assertNotIn("myceliums", budget.lower())
        self.assertTrue(budget.rstrip().endswith("```"))  # the answer format stays last

    def test_prompt_budget_is_part_of_the_comparable_config(self):
        self.assertNotIn("prompt_budget", RunConfig(label="x").comparable())
        self.assertTrue(RunConfig(label="x", prompt_budget=True).comparable()["prompt_budget"])


class Edges(unittest.TestCase):
    """py_edges.py resolves Django's import styles only with its #153 flags."""

    FILES = {
        "pkg/__init__.py": "",
        "pkg/util.py": "def helper():\n    return 1\n\nclass Base:\n    def __init__(self):\n        pass\n",
        "pkg/models/__init__.py": "from pkg.models.fields import *  # noqa\n",
        "pkg/models/fields.py": "__all__ = ['Field']\n\nclass Field:\n    def __init__(self):\n        pass\n",
        "pkg/app.py": ("from pkg import models\n\n"
                       "def local_import():\n    from pkg.util import helper\n    return helper()\n\n"
                       "def star():\n    return models.Field()\n"),
    }

    def edges(self, *flags):
        with tempfile.TemporaryDirectory() as tmp:
            for rel, text in self.FILES.items():
                path = Path(tmp) / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            out = subprocess.run([sys.executable, "-I", str(PY_EDGES), *flags, tmp, "pkg"],
                                 capture_output=True, text=True, check=True).stdout
        return {tuple(line.split("\t")[:2]) for line in out.splitlines() if line.endswith("\tdirect")}

    def test_default_keeps_the_fastapi_behaviour(self):
        e = self.edges()
        self.assertNotIn(("pkg.app:local_import", "pkg.util:helper"), e)
        self.assertNotIn(("pkg.app:star", "pkg.models.fields:Field.__init__"), e)

    def test_local_and_star_imports(self):
        e = self.edges("--local-imports", "--star-imports")
        self.assertIn(("pkg.app:local_import", "pkg.util:helper"), e)
        self.assertIn(("pkg.app:star", "pkg.models.fields:Field.__init__"), e)


def _row(task, cond, f1, mcp):
    return {"run_id": f"{task['id']}__{cond}__r1", "task_id": task["id"], "repo": "django", "type": task["type"],
            "condition": cond, "rep": 1, "attempt": 1, "status": "ok" if f1 else "max_turns", "answer": [],
            "f1": f1, "precision": f1, "recall": f1, "success": f1 >= 0.8, "tp": 0, "fp": 0, "neutral": 0,
            "n_gold": 1, "n_answer": 1, "missed": [], "false_positives": [], "num_turns": 3, "tool_calls": 4,
            "tool_calls_by_name": {"Grep": 4 - mcp, **({"mcp__myceliums__find_callers": mcp} if mcp else {})},
            "mcp_tool_calls": mcp, "mcp_tools_used": [], "tool_errors": {}, "first_tool": "Grep",
            "n_tools_offered": 13, "n_mcp_tools_offered": 9 if cond == "C" else 0, "input_tokens": 1,
            "cache_creation_input_tokens": 1, "cache_read_input_tokens": 1, "total_input_tokens": 1000,
            "first_turn_input_tokens": 10, "output_tokens": 10, "cost_usd": 0.01, "duration_ms": 1000,
            "duration_api_ms": 900, "wall_s": 2.0, "model": "m", "models_used": ["m"], "claude_version": "x",
            "finished_at": "2026-11-21T00:00:00+00:00", "agent_calls": 0, "subagent_tool_calls": 0,
            "subagent_mcp_tool_calls": 0, "first_mcp_call_index": 0 if mcp else None,
            "hook_grep_calls": 1 if cond == "C" else 0, "hook_grep_hints": 0, "hook_grep_silent_reasons": {},
            "session_hook": None, "format_hook_calls": 0}


class OutcomeReport(unittest.TestCase):
    def test_none_is_the_baseline_of_a_none_vs_c_run(self):
        tasks = config.load_json(OUTCOME_TASKS)["tasks"][:4]
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            try:
                d = Path(tmp) / "lbl"
                d.mkdir()
                rows = [_row(t, "none", 0.0, 0) for t in tasks] + [_row(t, "C", 1.0, 2) for t in tasks]
                (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (d / "meta.json").write_text(json.dumps({
                    "success_threshold": 0.8, "reps": 1, "repos": {}, "attempts_total": len(rows),
                    "conditions": ["none", "C"], "tasks_file": "tasks/tasks-outcome.json", "max_turns": 4,
                    "prompt_budget": True, "sessions_launched_ledger": len(rows)}))
                snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                             "n_gold": len(t["gold"]), "question": t["question"]} for t in tasks]
                (d / "tasks_snapshot.json").write_text(json.dumps(snapshot))
                self.assertTrue(report.write_report("lbl"))
                text = (d / "REPORT.md").read_text()
                self.assertIn("Does myceliums change the outcome", text)
                self.assertIn("## Paired differences vs none", text)
                self.assertIn("the prompt states the turn limit", text)
                self.assertIn("no myceliums at all", text)
                self.assertIn("**+1.000**", text)  # F1 C - none
                self.assertTrue(report.write_report("lbl", check=True))
            finally:
                config.RESULTS_DIR = old


if __name__ == "__main__":
    unittest.main()
