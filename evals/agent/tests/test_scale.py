"""The outcome eval at scale (#163): Bash in both conditions, myc CLI calls,
the task set over Django and Vue, the exact sign test and the per-budget
report."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, report  # noqa: E402
from aeval.grading import grade  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import FORMAT_RULES, RunConfig, Runner, WorkItem, build_prompt, myc_cli_calls  # noqa: E402
from aeval.stats import exact_sign_test  # noqa: E402

SCALE = config.EVAL_DIR / "tasks" / "tasks-scale.json"
CANDIDATES = config.EVAL_DIR / "tasks" / "tasks-scale-candidates.json"
CALIBRATION = config.RESULTS_DIR / "2026-11-22-scale-calibration"


def bash(cmd):
    return {"name": "Bash", "input": {"command": cmd}}


class MycCliCalls(unittest.TestCase):
    def test_counts_runs_of_myc_and_names_the_subcommand(self):
        calls = [bash("myc callers get_format --depth 2"), bash("cd django && myc impact iri_to_uri | head"),
                 bash("/tmp/x/bin/myc context 'flow'"), bash("echo $(myc --version)"),
                 {"name": "Grep", "input": {"pattern": "myc"}}]
        self.assertEqual(myc_cli_calls(calls), ["callers", "impact", "context", ""])

    def test_ignores_mentions_that_do_not_run_it(self):
        calls = [bash('grep -rn "myc" .'), bash("ls ~/.myceliums"), bash("cat mycelium.py"), bash("rg mycfg")]
        self.assertEqual(myc_cli_calls(calls), [])


class BashCondition(unittest.TestCase):
    def test_bash_is_part_of_the_comparable_config_only_when_set(self):
        self.assertNotIn("bash", RunConfig(label="x").comparable())
        self.assertTrue(RunConfig(label="x", bash=True).comparable()["bash"])

    def test_both_conditions_get_bash(self):
        task = config.load_json(SCALE)["tasks"][0]
        for cond in ("none", "C"):
            r = Runner(RunConfig(label="x", conditions=("none", "C"), bash=True, claude="claude"), [task],
                       config.load_repos())
            cmd = r._command(WorkItem(task, cond, 1), Path("/tmp/mcp.json"))
            tools = cmd[cmd.index("--tools") + 1].split(",")
            allowed = cmd[cmd.index("--allowedTools") + 1].split(",")
            self.assertIn("Bash", tools)
            self.assertIn("Bash", allowed)
            self.assertEqual(("mcp__myceliums" in allowed), cond == "C")

    def test_without_the_flag_there_is_no_shell(self):
        task = config.load_json(SCALE)["tasks"][0]
        r = Runner(RunConfig(label="x", conditions=("none",), claude="claude"), [task], config.load_repos())
        cmd = r._command(WorkItem(task, "none", 1), Path("/tmp/mcp.json"))
        self.assertNotIn("Bash", cmd[cmd.index("--tools") + 1].split(","))


class ScaleTasks(unittest.TestCase):
    def setUp(self):
        self.tasks = config.load_json(SCALE)["tasks"]

    def test_schema(self):
        self.assertEqual(check_tasks(self.tasks, config.load_repos(), check_evidence=False), [])
        self.assertEqual(check_tasks(config.load_json(CANDIDATES)["tasks"], config.load_repos(),
                                     check_evidence=False), [])

    def test_thirty_tasks_fifteen_per_repository_mixed_types(self):
        self.assertEqual(len(self.tasks), 30)
        by_repo, by_type = {}, {}
        for t in self.tasks:
            by_repo[t["repo"]] = by_repo.get(t["repo"], 0) + 1
            by_type[t["type"]] = by_type.get(t["type"], 0) + 1
        self.assertEqual(by_repo, {"django": 15, "vue": 15})
        self.assertEqual(by_type, {"transitive_callers": 6, "blast_radius": 4, "ambiguous": 6, "flow": 4,
                                   "tests": 6, "config": 4})

    def test_vue_is_pinned(self):
        vue = config.load_repos()["vue"]
        self.assertEqual(vue["ref"], "v3.5.46")
        self.assertEqual(vue["commit"], "7de9e41b61b2d890fd4be408068da75cbeb05709")

    def test_questions_give_no_hint_about_the_tool(self):
        for t in self.tasks:
            self.assertNotIn("myc", t["question"].lower(), t["id"])

    def test_calibration_is_recorded_and_matches_the_calibration_run(self):
        results = {json.loads(x)["task_id"]: json.loads(x)
                   for x in (CALIBRATION / "results.jsonl").read_text().splitlines() if x.strip()}
        for t in self.tasks:
            self.assertIn("calibration", t, t["id"])
            if t["calibration"].startswith("F1"):
                self.assertIn(f"F1 {results[t['id']]['f1']:.2f}", t["calibration"])
                self.assertIn(f"{results[t['id']]['tool_calls']} tool calls", t["calibration"])
        # every trivially solved calibration task (F1 1, at most two tool calls) was left out
        frozen = {t["id"] for t in self.tasks}
        for tid, r in results.items():
            if r["f1"] == 1.0 and r["tool_calls"] <= 2:
                self.assertNotIn(tid, frozen)

    def test_calibration_ran_on_the_committed_candidate_file(self):
        meta = config.load_json(CALIBRATION / "meta.json")
        self.assertEqual(meta["tasks_sha256"], config.sha256_file(CANDIDATES))
        self.assertEqual(meta["sessions_launched_ledger"], 15)
        self.assertEqual(meta["max_turns"], 10)
        self.assertTrue(meta["bash"] and meta["prompt_budget"])

    def test_names_kind_has_its_own_format_rule_and_grades_like_symbols(self):
        t = next(t for t in self.tasks if t["answer_kind"] == "names")
        prompt = build_prompt(t, config.load_repos()[t["repo"]], 10)
        self.assertIn(FORMAT_RULES["names"], prompt)
        g = grade(t, [t["gold"][0]["name"].lower(), "NotAThing"])
        self.assertEqual((g.tp, g.fp), (1, 1))


class SignTest(unittest.TestCase):
    def test_values(self):
        self.assertAlmostEqual(exact_sign_test(7, 0), 0.015625)
        self.assertEqual(exact_sign_test(0, 0), 1.0)
        self.assertEqual(exact_sign_test(3, 3), 1.0)
        self.assertAlmostEqual(exact_sign_test(10, 2), 0.03857421875)


def _row(task, cond, budget, f1, mcp=0, cli=0):
    return {"run_id": f"t{budget}/{task['id']}__{cond}__r1", "budget": budget, "task_id": task["id"],
            "repo": task["repo"], "type": task["type"], "condition": cond, "rep": 1, "attempt": 1,
            "status": "ok" if f1 else "max_turns", "answer": [], "f1": f1, "precision": f1, "recall": f1,
            "success": f1 >= 0.8, "tp": 0, "fp": 0, "neutral": 0, "n_gold": 1, "n_answer": 1, "missed": [],
            "false_positives": [], "num_turns": 3, "tool_calls": 4,
            "tool_calls_by_name": {"Bash": 4 - mcp, **({"mcp__myceliums__find_callers": mcp} if mcp else {})},
            "mcp_tool_calls": mcp, "mcp_tools_used": [], "tool_errors": {}, "first_tool": "Bash",
            "bash_calls": 4 - mcp, "myc_cli_calls": cli, "myc_cli_subcommands": ["callers"] if cli else [],
            "n_tools_offered": 14, "n_mcp_tools_offered": 9 if cond == "C" else 0, "input_tokens": 1,
            "cache_creation_input_tokens": 1, "cache_read_input_tokens": 1, "total_input_tokens": 1000 + budget,
            "first_turn_input_tokens": 10, "output_tokens": 10, "cost_usd": 0.01, "duration_ms": 1000,
            "duration_api_ms": 900, "wall_s": 2.0, "model": "m", "models_used": ["m"], "claude_version": "x",
            "finished_at": "2026-11-22T00:00:00+00:00", "agent_calls": 0, "subagent_tool_calls": 0,
            "subagent_mcp_tool_calls": 0, "first_mcp_call_index": None, "hook_grep_calls": 0, "hook_grep_hints": 0,
            "hook_grep_silent_reasons": {}, "session_hook": None, "format_hook_calls": 0}


class ScaleReport(unittest.TestCase):
    def test_per_budget_report_with_sign_test(self):
        tasks = config.load_json(SCALE)["tasks"][:6]
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            try:
                d = Path(tmp) / "scale"
                d.mkdir()
                rows = []
                for b in (4, 10, 40):
                    for i, t in enumerate(tasks):
                        rows.append(_row(t, "none", b, 0.0 if (b == 4 or i < 2) else 1.0))
                        rows.append(_row(t, "C", b, 1.0, mcp=1 if i % 2 else 0, cli=0 if i % 2 else 1))
                (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                runs = {str(b): {"label": f"scale-t{b}", "max_turns": b, "timeout_s": 300, "prompt_budget": b != 40,
                                 "bash": True, "sessions_launched_ledger": 12, "attempts_total": 12,
                                 "runs_final": 12, "retried_runs": []} for b in (4, 10, 40)}
                (d / "meta.json").write_text(json.dumps({
                    "success_threshold": 0.8, "reps": 1, "repos": {}, "budgets": [4, 10, 40], "runs": runs,
                    "conditions": ["none", "C"], "tasks_file": "tasks/tasks-scale.json", "bash": True}))
                snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                             "n_gold": len(t["gold"]), "question": t["question"]} for t in tasks]
                (d / "tasks_snapshot.json").write_text(json.dumps(snapshot))
                self.assertTrue(report.write_report("scale"))
                text = (d / "REPORT.md").read_text()
                self.assertIn("## Budget curve", text)
                self.assertIn("### 4 turns", text)
                self.assertIn("### 40 turns (cap; no budget stated)", text)
                self.assertIn("6 : 0 (ties 0), p = 0.0312", text)  # 4 turns: C wins every task
                self.assertIn("2 : 0 (ties 4), p = 0.5", text)  # 10 and 40 turns
                self.assertIn("| 4 turns | 6 | 6 | 3 | 3 |", text)  # myc used: MCP or CLI
                self.assertIn("No interval or p-value is corrected for multiple comparisons", text)
                self.assertIn("## By task type (exploratory)", text)
                self.assertNotIn("**+", text)  # no bold differences
                summary = json.loads((d / "summary.json").read_text())
                self.assertEqual(summary["analysis"]["overall"]["sign_test"]["c_better"], 6)
                self.assertTrue(report.write_report("scale", check=True))
                # a primary analysis without one task, the full numbers alongside
                meta = json.loads((d / "meta.json").read_text())
                meta["primary_exclude"] = {tasks[0]["id"]: "Not comparable."}
                (d / "meta.json").write_text(json.dumps(meta))
                self.assertTrue(report.write_report("scale"))
                text = (d / "REPORT.md").read_text()
                self.assertIn(f"the 5 tasks without `{tasks[0]['id']}`. Not comparable.", text)
                self.assertIn("5 : 0 (ties 0), p = 0.0625", text)  # 4 turns, primary
                self.assertIn("All 6 tasks: success 0 % → 100 %", text)
                self.assertIn(f"`{tasks[0]['id']}` †", text)
                summary = json.loads((d / "summary.json").read_text())
                self.assertEqual(summary["analysis_all_tasks"]["overall"]["sign_test"]["c_better"], 6)
            finally:
                config.RESULTS_DIR = old


if __name__ == "__main__":
    unittest.main()
