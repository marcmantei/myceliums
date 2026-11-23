"""#182 supplement: which answer items appeared in a session's tool output."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import supplement  # noqa: E402


class Seen(unittest.TestCase):
    def test_symbols_need_both_segments_as_words(self):
        self.assertEqual(supplement.seen("A.b", "class A { void b() }"), (True, True))
        self.assertEqual(supplement.seen("x.A.b(int)", "A b"), (True, True))
        self.assertEqual(supplement.seen("A.b", "class AB { void b() }"), (False, False))

    def test_paths_strict_and_lenient(self):
        self.assertEqual(supplement.seen("m/dir/F.java", "./m/dir/F.java:3"), (True, True))
        self.assertEqual(supplement.seen("m/dir/F.java", "dir/\n  F.java"), (False, True))
        self.assertEqual(supplement.seen("m/dir/F.java", "F.java"), (False, False))

    def test_only_tool_results_count(self):
        evs = [{"type": "assistant", "message": {"content": [{"type": "text", "text": "A.b"}]}},
               {"type": "user", "message": {"content": [{"type": "tool_result", "content": "C.d"}]}},
               {"type": "user", "message": {"content": "E.f"}}]
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "t.jsonl"
            p.write_text("".join(json.dumps(e) + "\n" for e in evs))
            self.assertEqual(supplement.tool_output(p), "C.d")


class PairNotes(unittest.TestCase):
    """#182 review: a confound found after the run is printed next to the pair's numbers."""

    def test_notes_of_a_pair(self):
        from aeval import config
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            self.assertEqual(config.pair_notes(d), [])
            (d / "posthoc.json").write_text(json.dumps({"pair_notes": [{"pair": ["C", "none"], "text": "x"},
                                                                       {"pair": ["D", "C"], "text": "y"}]}))
            self.assertEqual(config.pair_notes(d, "none", "C"), ["> **C ÷ none:** x", ""])
            self.assertEqual(config.pair_notes(d, "D", "none"), [])
            self.assertEqual(len(config.pair_notes(d)), 4)


if __name__ == "__main__":
    unittest.main()
