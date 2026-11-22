"""The #171 re-run of condition C: Incomplete/Next hint counting, the
re-run report against a committed #163-style run, and the index option that
drops a repository's extra excludes."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from aeval import config, report  # noqa: E402
from aeval.hints import hint_stats, nexts_of, step_kind  # noqa: E402
from aeval.stream import Transcript, parse_transcript  # noqa: E402

from test_scale import SCALE, _row  # noqa: E402

TRACE = "mcp__myceliums__trace_flow"
BLOCK = ("3 chains from `a` to `b`\n"
         "Incomplete (4 branch(es) not followed):\n"
         "  - a chain of 7 calls exists past max_depth=5\n"
         "Next: trace_flow(repo_id=\"r\", from_symbol=\"a\", to_symbol=\"b\", max_depth=7); "
         "or read the source at the lines listed above\n")


def _t(calls, results):
    t = Transcript()
    t.tool_calls = calls
    t.tool_results = results
    return t


def _call(i, name, **inp):
    return {"id": f"c{i}", "name": name, "input": inp, "parent": None}


def _res(i, text, after):
    return {"id": f"c{i}", "name": "?", "text": text, "is_error": False, "after": after}


class Hints(unittest.TestCase):
    def test_kinds_of_next_steps(self):
        self.assertEqual(step_kind('trace_flow(repo_id="r", from_symbol="a", to_symbol="b", max_depth=7)'), "call")
        self.assertEqual(step_kind("find_callers on the symbols at depth 10"), "call")
        self.assertEqual(step_kind('analyze(repo_id="r", force=true)'), "full_analysis")
        self.assertEqual(step_kind("read the source at the lines listed above"), "read_source")
        self.assertEqual(step_kind("search the source for `warn(` (grep): those call sites may call it"),
                         "grep_source")

    def test_text_block_and_structured_next(self):
        self.assertEqual(len(nexts_of(BLOCK)), 1)
        self.assertIsNone(nexts_of("3 chains, nothing cut"))
        js = json.dumps({"callers": [], "incomplete": {"reasons": ["depth limit 1"], "branches_cut": 2,
                                                       "next": 'find_callers(repo_id="r", symbol_name="x", depth=2)'}})
        self.assertEqual(nexts_of(js), ['find_callers(repo_id="r", symbol_name="x", depth=2)'])
        self.assertIsNone(nexts_of(json.dumps({"callers": [], "incomplete": None})))

    def test_a_deeper_call_after_the_hint_follows_it(self):
        calls = [_call(0, TRACE, from_symbol="a"), _call(1, TRACE, from_symbol="a", max_depth=7)]
        h = hint_stats(_t(calls, [_res(0, BLOCK, 1), _res(1, "ok", 2)]))
        self.assertEqual(h["incomplete_results"], 1)
        self.assertEqual(h["next_followed"], 1)
        self.assertEqual(h["next_kinds"], {"call": 1, "read_source": 1})
        self.assertFalse(h["source_after_hint"])

    def test_reading_the_source_follows_the_read_step(self):
        calls = [_call(0, TRACE), _call(1, "Bash", command="sed -n 1,40p django/x.py")]
        h = hint_stats(_t(calls, [_res(0, BLOCK, 1), _res(1, "...", 2)]))
        self.assertEqual(h["next_followed"], 1)
        self.assertTrue(h["source_after_hint"])

    def test_answering_right_away_does_not_follow(self):
        h = hint_stats(_t([_call(0, TRACE)], [_res(0, BLOCK, 1)]))
        self.assertEqual((h["incomplete_results"], h["next_followed"]), (1, 0))
        self.assertFalse(h["source_after_hint"])

    def test_calls_made_in_the_same_turn_do_not_count(self):
        # Parallel calls of one turn were made before the result arrived.
        calls = [_call(0, TRACE), _call(1, "Read", file_path="x.py")]
        h = hint_stats(_t(calls, [_res(0, BLOCK, 2), _res(1, "...", 2)]))
        self.assertEqual(h["next_followed"], 0)

    def test_grep_step_needs_the_name(self):
        block = ("Incomplete (2 branch(es) not followed):\n  - 2 call site(s) ...\n"
                 "Next: search the source for `warn(` (grep): those call sites may call it\n")
        calls = [_call(0, "mcp__myceliums__find_callers"), _call(1, "Grep", pattern="other"),
                 _call(2, "Bash", command="grep -rn 'warn(' packages")]
        h = hint_stats(_t(calls[:2], [_res(0, block, 1)]))
        self.assertEqual(h["next_followed"], 0)
        h = hint_stats(_t(calls, [_res(0, block, 1)]))
        self.assertEqual(h["next_followed"], 1)

    def test_only_myceliums_results_count(self):
        calls = [_call(0, "Read", file_path="notes.txt"), _call(1, "Bash", command="myc callers x --depth 2"),
                 _call(2, "Bash", command="myc callers x --depth 3")]
        cli = "x: 2 callers\nIncomplete (1 branch(es) not followed):\n  - depth limit 2\nNext: myc callers x --depth 3\n"
        h = hint_stats(_t(calls, [_res(0, BLOCK, 1), _res(1, cli, 2), _res(2, "x: 3 callers", 3)]))
        self.assertEqual(h["incomplete_results"], 1)  # the Read of a file quoting a block is not a hint
        self.assertEqual(h["next_followed"], 1)  # the CLI step through Bash

    def test_parse_transcript_keeps_result_text_and_position(self):
        events = [
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "u1", "name": TRACE, "input": {}},
                {"type": "tool_use", "id": "u2", "name": "Read", "input": {}}]}},
            {"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "u1", "content": [{"type": "text", "text": BLOCK}]},
                {"type": "tool_result", "tool_use_id": "u2", "content": "file text", "is_error": True}]}},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as fh:
            fh.write("".join(json.dumps(e) + "\n" for e in events))
        t = parse_transcript(fh.name)
        Path(fh.name).unlink()
        self.assertEqual([(r["name"], r["after"]) for r in t.tool_results], [(TRACE, 2), ("Read", 2)])
        self.assertEqual(t.tool_results[0]["text"], BLOCK)
        self.assertEqual(t.tool_errors["Read"], 1)


class RerunReport(unittest.TestCase):
    def test_compares_the_new_arm_with_both_committed_arms(self):
        tasks = config.load_json(SCALE)["tasks"][:6]
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            try:
                snapshot = [{"id": t["id"], "repo": t["repo"], "type": t["type"], "answer_kind": t["answer_kind"],
                             "n_gold": len(t["gold"]), "question": t["question"]} for t in tasks]
                base = Path(tmp) / "base"
                base.mkdir()
                rows = []
                for b in (4, 10, 40):
                    for i, t in enumerate(tasks):
                        rows.append(_row(t, "none", b, 1.0))
                        rows.append(_row(t, "C", b, 0.0 if i < 3 else 1.0, mcp=1))
                (base / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (base / "meta.json").write_text(json.dumps({
                    "date": "2026-11-22", "tasks_sha256": "abc", "budgets": [4, 10, 40], "model": "m",
                    "primary_exclude": {tasks[0]["id"]: "No data."},
                    "runs": {str(b): {"prompt_budget": b != 40, "timeout_s": 300} for b in (4, 10, 40)}}))
                d = Path(tmp) / "rerun"
                d.mkdir()
                new = []
                for b in (10, 40):
                    for i, t in enumerate(tasks):
                        r = _row(t, "C", b, 0.0 if i == 0 else 1.0, mcp=2)
                        r.update(incomplete_results=1 if i < 2 else 0, next_followed=1 if i == 1 else 0,
                                 next_kinds={"call": 1, "read_source": 1} if i < 2 else {},
                                 source_after_hint=i == 1)
                        new.append(r)
                (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in new))
                runs = {str(b): {"label": f"rerun-t{b}", "max_turns": b, "timeout_s": 300, "prompt_budget": b != 40,
                                 "bash": True, "sessions_launched_ledger": 6, "attempts_total": 6,
                                 "runs_final": 6, "retried_runs": []} for b in (10, 40)}
                (d / "meta.json").write_text(json.dumps({
                    "date": "2026-11-23", "success_threshold": 0.8, "reps": 1, "repos": {}, "budgets": [10, 40],
                    "runs": runs, "conditions": ["C"], "tasks_file": "tasks/tasks-scale.json", "bash": True,
                    "tasks_sha256": "abc", "rerun_of": "base",
                    "continuity_exclude": {tasks[0]["id"]: "No data."},
                    "index": {"django": {"symbols": 5, "files": 2, "embedded": 5, "index_exclude_extra": []}}}))
                (d / "tasks_snapshot.json").write_text(json.dumps(snapshot))
                self.assertTrue(report.write_report("rerun"))
                text = (d / "REPORT.md").read_text()
                summary = json.loads((d / "summary.json").read_text())
                a = summary["analysis"]["10"]
                self.assertEqual(a["vs_none"]["sign_test"], {"c_better": 0, "none_better": 1, "ties": 5, "p": 1.0})
                self.assertEqual(a["vs_old_c"]["sign_test"]["c_better"], 2)  # tasks 1 and 2
                self.assertEqual(a["vs_old_c"]["sign_test"]["none_better"], 0)
                self.assertEqual(summary["analysis_continuity"]["10"]["vs_none"]["sign_test"]["ties"], 5)
                self.assertEqual(summary["hints"]["all"]["sessions_with_hint"], 4)
                self.assertEqual(summary["hints"]["all"]["followed_results"], 2)
                self.assertIn("the file the #163 results are graded against", text)
                self.assertIn("**Primary analysis:** all 6 tasks", text)
                self.assertIn("5 tasks: success none 100 %", text)
                self.assertIn("| 10 turns | 6 | 2 | 2 | 1 | 1 | 1 | 1 of 2 |", text)
                self.assertIn("Time drift", text)
                self.assertIn("not reviewed", text)
                self.assertIn(f"`{tasks[0]['id']}` †", text)
                self.assertNotIn("4 turns |", text.split("## Per task")[1].split("## Incomplete")[0])
                self.assertTrue(report.write_report("rerun", check=True))
                # The base run's confound kept: the primary analysis leaves its tasks out, all tasks alongside.
                meta = json.loads((d / "meta.json").read_text())
                meta["primary_exclude"] = {tasks[0]["id"]: "Still no data."}
                (d / "meta.json").write_text(json.dumps(meta))
                self.assertTrue(report.write_report("rerun"))
                text = (d / "REPORT.md").read_text()
                summary = json.loads((d / "summary.json").read_text())
                self.assertIn(f"the 5 tasks without `{tasks[0]['id']}`. Still no data.", text)
                self.assertIn("All 6 tasks: success none 100 %", text)
                self.assertIn("| 5 (primary) | 10 turns |", text)
                self.assertIn("| all 6 | 10 turns |", text)
                self.assertEqual(summary["analysis"]["10"]["vs_none"]["sign_test"]["ties"], 5)
                self.assertEqual(summary["analysis_continuity"]["10"]["vs_none"]["sign_test"]["none_better"], 1)
                self.assertIn("not in the primary analysis", text)
                # A different task file is called out.
                meta = json.loads((d / "meta.json").read_text())
                meta["tasks_sha256"] = "def"
                (d / "meta.json").write_text(json.dumps(meta))
                report.write_report("rerun")
                self.assertIn("**differs**", (d / "REPORT.md").read_text())
            finally:
                config.RESULTS_DIR = old


class DefaultConfigIndex(unittest.TestCase):
    def test_flag_is_parsed(self):
        import agent_eval
        seen = {}
        orig = agent_eval.prep.index
        agent_eval.prep.index = lambda *a, **k: seen.update(k)
        try:
            agent_eval.main(["index", "--repo", "django", "--default-config", "django"])
        finally:
            agent_eval.prep.index = orig
        self.assertEqual(seen["default_config"], {"django"})


if __name__ == "__main__":
    unittest.main()
