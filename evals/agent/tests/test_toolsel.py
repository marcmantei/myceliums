import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, toolsel  # noqa: E402


class Intents(unittest.TestCase):
    def test_intent_file(self):
        intents = config.load_json(config.INTENTS_FILE)["intents"]
        self.assertGreaterEqual(len(intents), 40)
        self.assertEqual(len({i["id"] for i in intents}), len(intents))
        for i in intents:
            self.assertTrue(i["expected"], i["id"])
            self.assertNotIn("mcp__", " ".join(i["expected"]))

    def test_check_intents_flags_unknown_tools(self):
        intents = [{"id": "a", "intent": "x", "expected": ["get_callers"]},
                   {"id": "b", "intent": "y", "expected": ["no_such_tool"]},
                   {"id": "b", "intent": "z", "expected": []}]
        problems = toolsel.check_intents(intents, [{"name": "get_callers"}])
        self.assertEqual(len(problems), 3)  # unknown tool, duplicate id, empty expected

    def test_digest_ignores_order_of_keys_only(self):
        a = [{"name": "x", "description": "d", "inputSchema": {"a": 1}}]
        b = [{"inputSchema": {"a": 1}, "description": "d", "name": "x"}]
        c = [{"name": "x", "description": "changed", "inputSchema": {"a": 1}}]
        self.assertEqual(toolsel.tools_digest(a), toolsel.tools_digest(b))
        self.assertNotEqual(toolsel.tools_digest(a), toolsel.tools_digest(c))


class Render(unittest.TestCase):
    def test_accuracy_agreement_and_confusions(self):
        intents = [{"id": "i1", "intent": "who calls f", "expected": ["get_callers", "symbol_context"]},
                   {"id": "i2", "intent": "dead code", "expected": ["find_dead_code"]}]
        rows = [
            {"intent_id": "i1", "pass": 1, "chosen": "get_callers", "expected": intents[0]["expected"], "correct": True},
            {"intent_id": "i2", "pass": 1, "chosen": "get_stats", "expected": ["find_dead_code"], "correct": False},
            {"intent_id": "i1", "pass": 2, "chosen": "symbol_context", "expected": intents[0]["expected"], "correct": True},
            {"intent_id": "i2", "pass": 2, "chosen": None, "text_choice": "find_dead_code",
             "expected": ["find_dead_code"], "correct": False},
        ]
        text, summary = toolsel.render(rows, intents, {"n_tools": 54}, "lbl")
        self.assertEqual(summary["passes"], {1: {"n": 2, "correct": 1}, 2: {"n": 2, "correct": 1}})
        self.assertEqual(summary["agreement"], {"same": 0, "pairs": 2})
        self.assertIn(["find_dead_code", "get_stats", 1], summary["confusions"])
        self.assertIn("written out as text", text)
        self.assertIn("right tool", text)


if __name__ == "__main__":
    unittest.main()
