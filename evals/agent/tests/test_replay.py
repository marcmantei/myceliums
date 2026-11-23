"""#181: replaying myceliums calls against two builds — calls from
transcripts, the format-independent comparison, sizes and the report."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import replay  # noqa: E402

EVAL = Path(__file__).resolve().parents[1]

OLD_CALLERS = json.dumps({
    "callers": [
        {"depth": 1, "end_line": 20, "file_path": "src/a.ts", "kind": "Function", "name": "f",
         "qualified_name": "f", "resolved": "import", "start_line": 11},
        {"depth": 2, "end_line": 9, "file_path": "src/b.ts", "kind": "Method", "name": "g",
         "qualified_name": "C.g", "resolved": "name-match", "start_line": 4, "via": "f"},
    ],
    "depth_limited": True, "index_status": "Index: r, commit 1 = HEAD, up to date.",
    "incomplete": {"branches_cut": 1, "next": "find_callers(repo_id=\"r\", symbol_name=\"x\", depth=3)",
                   "reasons": ["depth limit 2: 1 more callers"]},
    "symbol_name": "x", "total_count": 2})

NEW_CALLERS = """Index: r, commit 1 = HEAD, up to date.

2 callers of x up to depth 2:
src/a.ts
  f Function :11-20
src/b.ts
  C.g Method :4-9 [depth 2 via f] (resolved: name-match)
Incomplete (1 branch(es) not followed):
  - depth limit 2: 1 more callers
Next: find_callers(repo_id="r", symbol_name="x", depth=3)
"""

OLD_IMPACT = """Index: r, commit 1 = HEAD, up to date.

Impact of changing q (Function, src/q.ts:1-5): 3 dependents (2 direct) in 2 files, following incoming CALLS and IMPORTS edges up to depth 2.

Direct dependents (call or import it; update them if its signature changes):
  * a (Function) src/a.ts:3
  * b (Function) src/a.ts:9 [imports]
Indirect dependents (depth 2+):
  * c (Method) src/c.ts:7 [depth 2, via a] (resolved: ambiguous)
Files with dependents (nearest depth):
  * src/a.ts (depth 1)
  * src/c.ts (depth 2)
"""

NEW_IMPACT = """Index: r, commit 1 = HEAD, up to date.

Impact of changing q (Function, src/q.ts:1-5): 3 dependents (2 direct) in 2 files, following incoming CALLS and IMPORTS edges up to depth 2.

Direct dependents (call or import it; update them if its signature changes):
src/a.ts
  a Function :3-4
  b Function :9 [imports]

Indirect dependents (depth 2+):
src/c.ts
  c Method :7-8 [depth 2 via a] (resolved: ambiguous)
"""

OLD_TRACE = """Index: r, commit 1 = HEAD, up to date.

Shortest call chain from a to d: 2 calls.

     a (Function) src/a.ts:1
  -> b (Function) src/b.ts:2
  -> d (Function) src/d.ts:4 (resolved: name-match)

Every symbol between them on a chain of 2-3 calls (2):
  * b (Function) src/b.ts:2 [chain of 2, call 1 from the start]
  * c (Function) src/b.ts:8 [chain of 3, call 2 from the start]
"""

NEW_TRACE = """Index: r, commit 1 = HEAD, up to date.

Shortest call chain from a to d: 2 calls.

     a (Function) src/a.ts:1
  -> b (Function) src/b.ts:2
  -> d (Function) src/d.ts:4 (resolved: name-match)

Every symbol between them on a chain of 2-3 calls (2; [chain N, call K]: K calls from the start on a chain of N):
src/b.ts
  b Function :2-3 [chain 2, call 1]
  c Function :8 [chain 3, call 2]
"""


def _replayed(*answers):
    return {"tools": [], "instructions": "",
            "answers": [{"i": i, "tool": tool, "args": {}, "text": text, "is_error": err}
                        for i, (tool, text, err) in enumerate(answers)]}


class Substance(unittest.TestCase):
    def test_every_old_format_matches_its_compact_form(self):
        for old, new in ((OLD_CALLERS, NEW_CALLERS), (OLD_IMPACT, NEW_IMPACT), (OLD_TRACE, NEW_TRACE)):
            so, sn = replay.substance(old), replay.substance(new)
            self.assertTrue(so["hits"], old)
            self.assertEqual(so, sn)

    def test_parsed_fields(self):
        s = replay.substance(NEW_CALLERS)
        self.assertEqual(s["totals"], [[2]])
        self.assertEqual(s["next"], 'find_callers(repo_id="r", symbol_name="x", depth=3)')
        self.assertIn(("src/b.ts", "C.g", 4, (2,), "f", False, "name-match"), s["hits"])
        impact = replay.substance(NEW_IMPACT)
        self.assertIn(("src/a.ts", "b", 9, (), None, True, None), impact["hits"])
        self.assertEqual(sum(impact["hits"].values()), 3, "the old file list is not a hit")
        trace = replay.substance(NEW_TRACE)
        self.assertIn(("src/b.ts", "c", 8, (3, 2), None, False, None), trace["hits"])
        self.assertEqual(trace["totals"], [[2], [2, 3, 2]])

    def test_compare_finds_lost_hits_changed_next_and_errors(self):
        before = _replayed(("find_callers", OLD_CALLERS, False), ("trace_flow", OLD_TRACE, False),
                           ("impact_of_change", OLD_IMPACT, False), ("trace_flow", "not found", True))
        same = _replayed(("find_callers", NEW_CALLERS, False), ("trace_flow", NEW_TRACE, False),
                         ("impact_of_change", NEW_IMPACT, False), ("trace_flow", "not found", True))
        result = replay.compare(before, same)
        self.assertEqual(sum(len(r["differ"]) for r in result.values()), 0, result)
        self.assertEqual(result["trace_flow"], {"compared": 2, "errors": 1, "differ": []})

        lost = NEW_CALLERS.replace("  f Function :11-20\n", "")
        moved = NEW_TRACE.replace("call 2]", "call 1]")
        other_next = NEW_IMPACT + "Incomplete (1 branch(es) not followed):\n  - x\nNext: y\n"
        changed = _replayed(("find_callers", lost, False), ("trace_flow", moved, False),
                            ("impact_of_change", other_next, False), ("trace_flow", "other", True))
        result = replay.compare(before, changed)
        self.assertEqual(result["find_callers"]["differ"][0]["what"], "hits")
        self.assertEqual([d["what"] for d in result["trace_flow"]["differ"]], ["hits", "error"])
        self.assertEqual(result["impact_of_change"]["differ"][0]["what"], "reasons,next")

    def test_an_answer_with_nothing_to_compare_is_reported(self):
        before = _replayed(("trace_flow", "something else entirely", False))
        result = replay.compare(before, before)
        self.assertEqual(result["trace_flow"]["differ"][0]["what"], "nothing parsed")


class Calls(unittest.TestCase):
    def test_calls_come_from_the_transcripts_with_their_repository(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = Path(tmp) / "run-t10" / "sessions" / "django-amb-x__C__r1"
            session.mkdir(parents=True)
            events = [
                {"type": "assistant", "message": {"id": "m1", "content": [
                    {"type": "tool_use", "id": "t1", "name": "mcp__myceliums__find_callers",
                     "input": {"symbol_name": "x", "repo_id": "django-1"}},
                    {"type": "tool_use", "id": "t2", "name": "Grep", "input": {"pattern": "x"}}]}},
                {"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "{}"}]}},
            ]
            (session / "attempt1.jsonl").write_text("\n".join(json.dumps(e) for e in events) + "\n")
            calls = replay.extract_calls([Path(tmp)])
        self.assertEqual(calls, [{"run": "run-t10", "session": "django-amb-x__C__r1", "repo": "django",
                                  "tool": "find_callers", "input": {"symbol_name": "x", "repo_id": "django-1"}}])


class Sizes(unittest.TestCase):
    def test_definitions_are_sized_as_claude_code_sends_them(self):
        tools = [{"name": "find_callers", "description": "Who calls?", "inputSchema": {"type": "object"},
                  "annotations": {"readOnlyHint": True}, "outputSchema": {"type": "object"}}]
        self.assertEqual(replay.model_definitions(tools),
                         '[{"name":"mcp__myceliums__find_callers","description":"Who calls?",'
                         '"input_schema":{"type":"object"}}]')
        before = _replayed(("find_callers", OLD_CALLERS, False))
        after = _replayed(("find_callers", NEW_CALLERS, False))
        before["tools"] = after["tools"] = tools
        summary = replay.summarize(before, after)
        row = summary["answers"]["find_callers"]
        self.assertEqual((row["before"]["calls"], row["before"]["chars"], row["after"]["chars"]),
                         (1, len(OLD_CALLERS), len(NEW_CALLERS)))
        md = replay.render(summary, replay.compare(before, after), {"title": "t", "runs": ["r"]})
        self.assertIn("| `find_callers` (1 calls) |", md)
        self.assertIn("| `find_callers` | 1 | 0 | 0 |", md)

    def test_the_committed_replay_report_is_current(self):
        res = subprocess.run([sys.executable, str(EVAL / "agent_eval.py"), "replay",
                              "2026-11-23-compact-output", "--check"], capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)


if __name__ == "__main__":
    unittest.main()
