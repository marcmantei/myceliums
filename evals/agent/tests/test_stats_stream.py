import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval.stats import paired_bootstrap, percentile  # noqa: E402
from aeval.stream import check_isolation, parse_transcript  # noqa: E402


class Bootstrap(unittest.TestCase):
    def test_constant_difference_has_a_degenerate_interval(self):
        ci = paired_bootstrap([(0.9, 0.4)] * 10, n_resamples=500)
        self.assertAlmostEqual(ci.diff_mean, 0.5)
        self.assertAlmostEqual(ci.diff_lo, 0.5)
        self.assertAlmostEqual(ci.diff_hi, 0.5)

    def test_interval_contains_the_mean_and_is_deterministic(self):
        pairs = [(i / 10, (i % 3) / 3) for i in range(12)]
        a = paired_bootstrap(pairs, n_resamples=2000, seed=1)
        b = paired_bootstrap(pairs, n_resamples=2000, seed=1)
        self.assertEqual(a, b)
        self.assertLessEqual(a.diff_lo, a.diff_mean)
        self.assertLessEqual(a.diff_mean, a.diff_hi)
        self.assertLessEqual(a.a_lo, a.a_mean)
        self.assertLessEqual(a.a_mean, a.a_hi)

    def test_pairing_shrinks_the_difference_interval(self):
        # Large between-task spread, tiny within-pair difference: the paired
        # difference interval must be far narrower than either mean's interval.
        pairs = [(t + 0.05, t) for t in (0.0, 0.2, 0.4, 0.6, 0.8, 1.0) * 3]
        ci = paired_bootstrap(pairs, n_resamples=2000)
        self.assertLess(ci.diff_hi - ci.diff_lo, 0.2 * (ci.a_hi - ci.a_lo) + 1e-9)

    def test_empty(self):
        self.assertEqual(paired_bootstrap([]).n_tasks, 0)

    def test_percentile(self):
        self.assertEqual(percentile([1.0, 2.0, 3.0], 0.5), 2.0)
        self.assertEqual(percentile([1.0, 3.0], 0.25), 1.5)


def _write(events):
    fh = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    for ev in events:
        fh.write(json.dumps(ev) + "\n")
    fh.write("not json\n")
    fh.close()
    return fh.name


class Transcript(unittest.TestCase):
    def setUp(self):
        self.path = _write([
            {"type": "system", "subtype": "init", "model": "m", "mcp_servers": [],
             "tools": ["Read", "Grep", "Glob"], "slash_commands": [], "skills": []},
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "1", "name": "Grep", "input": {}},
                {"type": "tool_use", "id": "2", "name": "mcp__myceliums__get_callers", "input": {}}]}},
            {"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "2", "is_error": True, "content": "boom"}]}},
            {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed_warning", "utilization": 0.91,
                                                             "unifiedWindows": {"seven_day": {"utilization": 0.93}}}},
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "done"}]}},
            {"type": "result", "subtype": "success", "result": "final", "num_turns": 2,
             "modelUsage": {"a": {"inputTokens": 1, "outputTokens": 2, "cacheReadInputTokens": 3,
                                  "cacheCreationInputTokens": 4},
                            "b": {"inputTokens": 10, "outputTokens": 20, "cacheReadInputTokens": 30,
                                  "cacheCreationInputTokens": 40}}},
        ])

    def test_parse(self):
        t = parse_transcript(self.path)
        self.assertEqual(t.bad_lines, 1)
        self.assertEqual(t.final_text, "final")
        self.assertEqual(t.tool_counts["Grep"], 1)
        self.assertEqual(t.tool_errors["mcp__myceliums__get_callers"], 1)
        self.assertAlmostEqual(t.max_utilization, 0.93)
        self.assertFalse(t.rate_limit_rejected)
        u = t.usage()
        self.assertEqual((u["input_tokens"], u["output_tokens"], u["total_input_tokens"]), (11, 22, 88))

    def test_isolation(self):
        init = {"tools": ["Read", "Grep", "Glob", "ToolSearch"], "mcp_servers": [], "slash_commands": [], "skills": []}
        req = {"Read", "Grep", "Glob"}
        self.assertEqual(check_isolation(init, req, False, {"ToolSearch"}), [])
        self.assertTrue(check_isolation(init, req, False))  # ToolSearch not allowed here
        leaked = dict(init, tools=init["tools"] + ["Bash"])
        self.assertTrue(check_isolation(leaked, req, False, {"ToolSearch"}))
        with_mcp = dict(init, tools=init["tools"] + ["mcp__myceliums__get_callers"],
                        mcp_servers=[{"name": "myceliums", "status": "connected"}])
        self.assertEqual(check_isolation(with_mcp, req, True, {"ToolSearch"}), [])
        self.assertTrue(check_isolation(with_mcp, req, False, {"ToolSearch"}))
        other = dict(with_mcp, mcp_servers=[{"name": "myceliums", "status": "connected"}, {"name": "x"}])
        self.assertTrue(check_isolation(other, req, True, {"ToolSearch"}))
        failed = dict(with_mcp, mcp_servers=[{"name": "myceliums", "status": "failed"}])
        self.assertTrue(check_isolation(failed, req, True, {"ToolSearch"}))
        skills = dict(init, skills=["deploy"])
        self.assertTrue(check_isolation(skills, req, False, {"ToolSearch"}))
        self.assertTrue(check_isolation(None, req, False))


if __name__ == "__main__":
    unittest.main()
