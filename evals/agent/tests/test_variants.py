"""The variant matrix (#125): conditions, commands, hook logs, probe parsing,
the hard task sets and the variant report."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, probe, report  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import RunConfig, Runner, WorkItem, hook_stats  # noqa: E402
from aeval.stream import parse_transcript  # noqa: E402


def _runner(conditions=("A", "C", "E")):
    tasks = config.load_json(config.HARD_TASKS_FILE)["tasks"][:3]
    return Runner(RunConfig(label="unit", reps=2, conditions=conditions), tasks, config.load_repos()), tasks


class Variants(unittest.TestCase):
    def test_every_condition_has_a_variant(self):
        self.assertEqual(set(config.VARIANTS), set(config.CONDITIONS))
        self.assertTrue(config.VARIANTS["with"].legacy and config.VARIANTS["without"].legacy)
        self.assertFalse(any(config.VARIANTS[c].legacy for c in ("none", "A", "C", "E")))

    def test_layers_grow_from_a_to_e(self):
        a, c, e = (config.VARIANTS[x] for x in "ACE")
        self.assertTrue(a.mcp and c.mcp and e.mcp)
        self.assertEqual((a.setup_claude, c.setup_claude, e.setup_claude), (False, True, True))
        self.assertEqual((a.agent, c.agent, e.agent), (False, False, True))

    def test_plan_pairs_all_conditions_per_task(self):
        runner, tasks = _runner()
        plan = runner.plan()
        self.assertEqual(len(plan), len(tasks) * 3 * 2)
        for i in range(0, len(plan), 3):
            block = plan[i:i + 3]
            self.assertEqual({it.condition for it in block}, {"A", "C", "E"})
            self.assertEqual(len({(it.task["id"], it.rep) for it in block}), 1)

    def test_pilot_config_keeps_its_shape(self):
        d = RunConfig(label="x").comparable()
        self.assertNotIn("conditions", d)
        self.assertNotIn("tasks_file", d)
        self.assertNotIn("extra_env", d)
        self.assertEqual(RunConfig(label="x", conditions=("A", "C")).comparable()["conditions"], ["A", "C"])


class Commands(unittest.TestCase):
    def setUp(self):
        self.runner, self.tasks = _runner()
        self.t = self.tasks[0]

    def cmd(self, cond):
        return self.runner._command(WorkItem(self.t, cond, 1), Path("/x/mcp.json"))

    def test_a_and_c_differ_only_in_the_working_copy(self):
        self.assertEqual(self.cmd("A"), self.cmd("C"))

    def test_variants_load_project_settings_and_hook_events(self):
        for cond in ("none", "A", "C", "E"):
            cmd = self.cmd(cond)
            self.assertEqual(cmd[cmd.index("--setting-sources") + 1], "project")
            self.assertIn("--include-hook-events", cmd)

    def test_e_adds_only_the_explorer_subagent(self):
        a, e = self.cmd("A"), self.cmd("E")
        self.assertEqual(e[e.index("--tools") + 1], a[a.index("--tools") + 1] + ",Agent")
        self.assertTrue(e[e.index("--allowedTools") + 1].endswith(",Agent(code-explorer)"))
        denied = e[e.index("--disallowedTools") + 1].split(",")
        self.assertEqual(denied, [f"Agent({x})" for x in config.BUILTIN_AGENTS])
        self.assertNotIn("Agent(code-explorer)", denied)
        self.assertNotIn("--disallowedTools", a)

    def test_none_has_no_mcp_permission(self):
        allowed = self.cmd("none")[self.cmd("none").index("--allowedTools") + 1]
        self.assertNotIn("mcp__", allowed)

    def test_session_env(self):
        legacy = config.session_env(legacy=True)
        self.assertEqual(legacy["CLAUDE_CODE_DISABLE_CLAUDE_MDS"], "1")
        variant = config.session_env(legacy=False, bin_dir=Path("/tmp/b"))
        self.assertNotIn("CLAUDE_CODE_DISABLE_CLAUDE_MDS", variant)
        self.assertTrue(variant["PATH"].startswith("/tmp/b:"))
        self.assertEqual(variant["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1")

    def test_wrapper_pins_the_index_and_logs_hooks(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.runner.cfg.myc = "/opt/myc"
            w = self.runner._write_wrapper(Path(tmp) / "bin", {"MYCELIUMS_DATA_DIR": "/d", "HOME": "/h"},
                                           Path(tmp) / "hooks")
            text = w.read_text()
            self.assertIn("export MYCELIUMS_DATA_DIR=/d HOME=/h", text)
            self.assertIn("hook-grep|session|format-hook", text)
            self.assertIn("exec /opt/myc", text)
            self.assertTrue(w.stat().st_mode & 0o100)


class Hooks(unittest.TestCase):
    def test_hook_stats(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "hook-grep-1.out").write_text('{"hookSpecificOutput": {}}\n')
            (d / "hook-grep-2.out").write_text("")
            (d / "hook-grep-2.err").write_text('myc hook-grep: silent: no identifier-shaped search pattern\n')
            (d / "hook-grep-3.out").write_text("")
            (d / "hook-grep-3.err").write_text('myc hook-grep: silent: no identifier-shaped search pattern\n')
            (d / "session-4.out").write_text('{"systemMessage":"[myceliums] fzf ready"}\n')
            (d / "format-hook-5.out").write_text("{}")
            s = hook_stats(d)
            self.assertEqual((s["hook_grep_calls"], s["hook_grep_hints"]), (3, 1))
            self.assertEqual(s["hook_grep_silent_reasons"], {"no identifier-shaped search pattern": 2})
            self.assertIn("fzf ready", s["session_hook"])
            self.assertEqual(s["format_hook_calls"], 1)

    def test_missing_log_dir(self):
        self.assertEqual(hook_stats(Path("/nonexistent"))["hook_grep_calls"], 0)


class Stream(unittest.TestCase):
    def test_subagent_calls_and_hook_events(self):
        events = [
            {"type": "system", "subtype": "init", "tools": ["Task", "Read"]},
            {"type": "system", "subtype": "hook_response", "hook_event": "PreToolUse"},
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "a1", "name": "Agent", "input": {}}]}},
            {"type": "assistant", "parent_tool_use_id": "a1", "message": {"content": [
                {"type": "tool_use", "id": "s1", "name": "mcp__myceliums__find_callers", "input": {}}]}},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as fh:
            fh.write("\n".join(json.dumps(e) for e in events))
        t = parse_transcript(fh.name)
        Path(fh.name).unlink()
        self.assertEqual([c["parent"] for c in t.tool_calls], [None, "a1"])
        self.assertEqual(len(t.hook_events), 1)


class Probe(unittest.TestCase):
    def test_agents_offered(self):
        text = ("intro\nAvailable agent types for the Agent tool:\n- code-explorer: Answers questions (Tools: x)\n"
                "\nWhen you launch")
        self.assertEqual(probe._agents_offered(text), ["code-explorer"])
        self.assertEqual(probe._agents_offered("nothing"), [])


class HardTasks(unittest.TestCase):
    def test_both_sets_pass_the_schema_check(self):
        repos = config.load_repos()
        for path in (config.HARD_TASKS_FILE, config.EVAL_DIR / "tasks" / "tasks-hard-v1.json"):
            tasks = config.load_json(path)["tasks"]
            self.assertEqual(len(tasks), 20, path)
            self.assertEqual(check_tasks(tasks, repos, check_evidence=False), [], path)
            types = {t["type"] for t in tasks}
            self.assertEqual(types, {"transitive_callers", "blast_radius", "ambiguous", "tests", "flow"})

    def test_the_matrix_set_is_larger_than_the_calibrated_one(self):
        v1 = config.load_json(config.EVAL_DIR / "tasks" / "tasks-hard-v1.json")["tasks"]
        v2 = config.load_json(config.HARD_TASKS_FILE)["tasks"]
        self.assertGreater(sum(len(t["gold"]) for t in v2), 1.5 * sum(len(t["gold"]) for t in v1))
        self.assertFalse({t["id"] for t in v1} & {t["id"] for t in v2})


def _row(task, cond, rep, f1, tokens, mcp):
    return {"run_id": f"{task['id']}__{cond}__r{rep}", "task_id": task["id"], "repo": task["repo"],
            "type": task["type"], "condition": cond, "rep": rep, "attempt": 1, "status": "ok", "answer": [],
            "f1": f1, "precision": f1, "recall": f1, "success": f1 >= 0.8, "tp": 0, "fp": 0, "neutral": 0,
            "n_gold": 1, "n_answer": 1, "missed": [], "false_positives": [], "num_turns": 3, "tool_calls": 4,
            "tool_calls_by_name": {"Grep": 4 - mcp, **({"mcp__myceliums__find_callers": mcp} if mcp else {})},
            "mcp_tool_calls": mcp, "mcp_tools_used": [], "tool_errors": {}, "first_tool": "Grep",
            "n_tools_offered": 13, "n_mcp_tools_offered": 9, "input_tokens": 1, "cache_creation_input_tokens": 1,
            "cache_read_input_tokens": 1, "total_input_tokens": tokens, "first_turn_input_tokens": 10,
            "output_tokens": 10, "cost_usd": 0.01, "duration_ms": 1000, "duration_api_ms": 900, "wall_s": 2.0,
            "model": "m", "models_used": ["m"], "claude_version": "x", "finished_at": "2026-11-20T00:00:00+00:00",
            "agent_calls": 1 if cond == "E" else 0, "subagent_tool_calls": 0, "subagent_mcp_tool_calls": 0,
            "first_mcp_call_index": 0, "hook_grep_calls": 2 if cond != "A" else 0,
            "hook_grep_hints": 1 if cond != "A" else 0, "hook_grep_silent_reasons": {"no index": 1},
            "session_hook": None, "format_hook_calls": 0}


class VariantReport(unittest.TestCase):
    def test_render_check_and_paired_differences(self):
        tasks = config.load_json(config.HARD_TASKS_FILE)["tasks"][:4]
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            try:
                d = Path(tmp) / "lbl"
                d.mkdir()
                rows = []
                for t in tasks:
                    for rep in (1, 2):
                        rows.append(_row(t, "A", rep, 0.5, 1000, 0))
                        rows.append(_row(t, "C", rep, 1.0, 1500, 2))
                        rows.append(_row(t, "E", rep, 0.75, 500, 1))
                (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (d / "meta.json").write_text(json.dumps({
                    "success_threshold": 0.8, "reps": 2, "repos": {}, "attempts_total": len(rows),
                    "conditions": ["A", "C", "E"], "tasks_file": "tasks/tasks-hard.json",
                    "sessions_launched_ledger": len(rows)}))
                snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                             "n_gold": len(t["gold"]), "question": t["question"]} for t in tasks]
                (d / "tasks_snapshot.json").write_text(json.dumps(snapshot))
                self.assertTrue(report.write_report("lbl"))
                self.assertTrue(report.write_report("lbl", check=True))
                text = (d / "REPORT.md").read_text()
                self.assertIn("## Paired differences vs A", text)
                self.assertIn("**+0.500**", text)  # F1 C - A
                self.assertIn("**+0.250**", text)  # F1 E - A
                summary = json.loads((d / "summary.json").read_text())
                vs = summary["analysis"]["vs_baseline"]
                self.assertAlmostEqual(vs["C"]["total_input_tokens"]["diff_mean"], 500)
                self.assertAlmostEqual(vs["E"]["myc_used"]["diff_mean"], 1.0)
                (d / "REPORT.md").write_text(text + "x\n")
                self.assertFalse(report.write_report("lbl", check=True))
            finally:
                config.RESULTS_DIR = old


if __name__ == "__main__":
    unittest.main()
