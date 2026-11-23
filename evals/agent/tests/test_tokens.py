"""#178: per-tool token attribution, cost by cache class, spend per solved
task, arm D ("myceliums only"), edit-task grading and the realistic profile."""

import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import bashcmd, config, editgrade, mockapi, report_tokens  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import RunConfig, Runner, WorkItem, build_prompt  # noqa: E402
from aeval.stats import per_success_bootstrap  # noqa: E402
from aeval.tokens import PRICES, attribute, category_of, load_prices, price_of  # noqa: E402

M = "claude-sonnet-5-5"


def _usage(inp, cw, cr, out=1, w1h=True):
    return {"input_tokens": inp, "cache_creation_input_tokens": cw, "cache_read_input_tokens": cr,
            "output_tokens": out, "cache_creation": {"ephemeral_1h_input_tokens": cw if w1h else 0,
                                                     "ephemeral_5m_input_tokens": 0 if w1h else cw}}


def _asst(mid, blocks, usage, parent=None):
    return {"type": "assistant", "parent_tool_use_id": parent,
            "message": {"id": mid, "model": M, "content": blocks, "usage": usage}}


def _results(*pairs, parent=None):
    return {"type": "user", "parent_tool_use_id": parent, "message": {"content": [
        {"type": "tool_result", "tool_use_id": tid, "content": text} for tid, text in pairs]}}


def _final(inp, cw, cr, out, cost=None, thinking=0):
    return {"type": "result", "subtype": "success", "total_cost_usd": cost,
            "modelUsage": {M: {"inputTokens": inp, "cacheCreationInputTokens": cw, "cacheReadInputTokens": cr,
                               "outputTokens": out, "thinkingTokens": thinking}}}


def _write(events) -> str:
    fh = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    fh.write("".join(json.dumps(e) + "\n" for e in events))
    fh.close()
    return fh.name


def _tool(tid, name, **inp):
    return {"type": "tool_use", "id": tid, "name": name, "input": inp}


class BashClassifier(unittest.TestCase):
    def test_programs_of_a_pipeline(self):
        self.assertEqual(bashcmd.classify("cd django; grep -rn x . | head"), "search")
        self.assertEqual(bashcmd.classify("sed -n 1,9p a.py"), "read")
        self.assertEqual(bashcmd.classify("sed -n 1,9p a.py && rg foo"), "search")
        self.assertEqual(bashcmd.classify("myc callers x 2>&1 | grep -v y"), "myceliums")
        self.assertEqual(bashcmd.classify("/tmp/bin/myc impact X"), "myceliums")
        self.assertEqual(bashcmd.classify("sed -i 's/a/b/' f.py"), "edit")
        self.assertEqual(bashcmd.classify("ls -la src && git log -1"), "other")
        self.assertEqual(bashcmd.classify("FOO=1 xargs -0 grep x < files"), "search")
        self.assertEqual(bashcmd.classify("git grep -n parseAccept"), "search")
        self.assertEqual(bashcmd.classify("echo $(find . -name '*.py')"), "search")

    def test_search_programs(self):
        self.assertEqual(bashcmd.search_programs("cat a | grep b; find . -name c; ag d; fd e"),
                         ["grep", "find", "ag", "fd"])
        self.assertEqual(bashcmd.search_programs("myc callers grep_helper; cat grep.py"), [])

    def test_hook_blocks_search_and_lets_the_rest_through(self):
        err = io.StringIO()
        ev = {"tool_name": "Bash", "tool_input": {"command": "cat x.py | rg foo"}}
        self.assertEqual(bashcmd.hook(io.StringIO(json.dumps(ev)), err), 2)
        self.assertIn("`rg`", err.getvalue())
        for cmd in ("myc callers parseAccept", "sed -n 1,40p src/a.ts", "ls src"):
            ev = {"tool_name": "Bash", "tool_input": {"command": cmd}}
            self.assertEqual(bashcmd.hook(io.StringIO(json.dumps(ev)), io.StringIO()), 0, cmd)
        self.assertEqual(bashcmd.hook(io.StringIO("not json"), io.StringIO()), 0)

    def test_hook_runs_standalone_in_isolated_mode(self):
        # The session runs `python3 -I bashcmd.py hook`: no package imports allowed.
        ev = json.dumps({"tool_name": "Bash", "tool_input": {"command": "grep -rn x ."}})
        res = subprocess.run([sys.executable, "-I", str(Path(bashcmd.__file__)), "hook"], input=ev,
                             capture_output=True, text=True, cwd=tempfile.gettempdir())
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("Searching is not available", res.stderr)

    def test_tool_categories(self):
        self.assertEqual(category_of("mcp__myceliums__find_callers", {}), "myceliums")
        self.assertEqual(category_of("Grep", {}), "search")
        self.assertEqual(category_of("Glob", {}), "search")
        self.assertEqual(category_of("Read", {}), "read")
        self.assertEqual(category_of("Edit", {}), "edit")
        self.assertEqual(category_of("ToolSearch", {}), "other")
        self.assertEqual(category_of("Bash", {"command": "head -50 a.py"}), "read")


class Attribution(unittest.TestCase):
    def _session(self):
        # Three calls. Call 2 sees the reply of call 1 (two tool calls, 72 chars),
        # a 400-char myceliums result, a 200-char Grep result and a 74-char hook text.
        # Call 3 sees reply 2 (a 100-token thinking estimate) and a 400-char Read result.
        hook = {"hookSpecificOutput": {"additionalContext": "h" * 74}}
        return [
            {"type": "system", "subtype": "init", "tools": ["Read"]},
            _asst("m1", [_tool("t1", "mcp__myceliums__find_callers", symbol="abcdefgh")], _usage(2, 1000, 0)),
            _asst("m1", [_tool("t2", "Grep", pattern="abc")], _usage(2, 1000, 0)),  # same call, second block
            {"type": "system", "subtype": "hook_response", "output": json.dumps(hook)},
            _results(("t1", "m" * 400), ("t2", "g" * 200)),
            {"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 60},
            {"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 100},
            _asst("m2", [{"type": "thinking", "thinking": "", "signature": "s"}], _usage(2, 300, 1000)),
            _asst("m2", [_tool("t3", "Read", file_path="a.py")], _usage(2, 300, 1000)),
            _results(("t3", "r" * 400)),
            _asst("m3", [{"type": "text", "text": "done"}], _usage(1, 250, 1300)),
            _final(5, 1550, 2300, 900, cost=0.0123, thinking=100),
        ]

    def test_sums_reconcile_and_growth_is_split_by_size(self):
        a = attribute(_write(self._session()))
        self.assertEqual(a.method, "usage-delta")
        self.assertEqual(a.calls, 3)
        self.assertEqual(a.residual, {"input_tokens": 0, "cache_creation_input_tokens": 0,
                                      "cache_read_input_tokens": 0})
        self.assertAlmostEqual(a.total_input(), 1002 + 1302 + 1551)
        cats = a.input_by_category()
        self.assertAlmostEqual(cats["base"], 1002 * 3)
        # Growth 1 (300): reply 1 = (28 + 22 + 4 + 18) / 4 = 18 (names + JSON inputs), myc 100,
        # grep 50, hook 18.5.
        w = {"assistant": 18.0, "myceliums": 100.0, "search": 50.0, "hook": 18.5}
        g1 = {k: 300 * v / sum(w.values()) for k, v in w.items()}
        # Growth 2 (249): reply 2 = thinking 100 + Read call (4 + 21)/4 = 106.25, read 100
        g2 = {"assistant": 249 * 106.25 / 206.25, "read": 249 * 100 / 206.25}
        self.assertAlmostEqual(a.result_tokens["myceliums"], g1["myceliums"])
        self.assertAlmostEqual(a.result_tokens["read"], g2["read"])
        self.assertAlmostEqual(cats["myceliums"], 2 * g1["myceliums"])  # re-read by calls 2 and 3
        self.assertAlmostEqual(cats["read"], g2["read"])
        self.assertAlmostEqual(cats["assistant"], 2 * g1["assistant"] + g2["assistant"])
        self.assertEqual(a.output_tokens, 900)
        self.assertEqual(a.thinking_tokens, 100)

    def test_cache_classes_follow_the_call_usage(self):
        a = attribute(_write(self._session()))
        by_class = {k: sum(v[k] for v in a.tokens.values()) for k in
                    ("uncached", "cache_write_5m", "cache_write_1h", "cache_read")}
        self.assertAlmostEqual(by_class["cache_read"], 2300)
        self.assertAlmostEqual(by_class["cache_write_1h"], 1550)
        self.assertAlmostEqual(by_class["uncached"], 5)
        # Call 2 reads the base from cache and writes its growth: the myceliums
        # result is a cache write once, a cache read once.
        myc = a.tokens["myceliums"]
        self.assertAlmostEqual(myc["cache_write_1h"], myc["cache_read"], delta=1.0)
        self.assertAlmostEqual(a.tokens["base"]["cache_read"], 1000 + 1002)

    def test_cost_by_class_matches_the_price_table(self):
        a = attribute(_write(self._session()))
        p = PRICES[M]
        expect = (5 * p["input"] + 1550 * p["cache_write_1h"] + 2300 * p["cache_read"] + 900 * p["output"]) / 1e6
        self.assertAlmostEqual(a.cost_usd, expect)
        self.assertAlmostEqual(sum(a.cost.values()), expect)
        self.assertAlmostEqual(a.cost["output"], 900 * p["output"] / 1e6)
        self.assertEqual(a.cli_cost_usd, 0.0123)

    def test_the_table_reproduces_the_cli_cost_of_a_real_session(self):
        # Totals of t40/django-amb-color-make-style__C__r1 (#163) and the CLI's cost.
        p = PRICES[M]
        cost = (14 * p["input"] + 9977 * p["cache_write_1h"] + 104141 * p["cache_read"] + 2485 * p["output"]) / 1e6
        self.assertAlmostEqual(cost, 0.0752001, places=9)

    def test_residual_is_kept_as_unattributed(self):
        ev = self._session()
        ev[-1] = _final(5 + 7, 1550 + 30, 2300, 900)  # a call the transcript does not show
        a = attribute(_write(ev))
        self.assertEqual(a.residual["input_tokens"], 7)
        self.assertEqual(a.residual["cache_creation_input_tokens"], 30)
        self.assertAlmostEqual(a.input_by_category()["unattributed"], 37)
        self.assertAlmostEqual(a.total_input(), 3855 + 37)

    def test_subagent_has_its_own_context(self):
        ev = [
            _asst("m1", [_tool("a1", "Agent", prompt="who calls x")], _usage(0, 1000, 0)),
            _asst("s1", [_tool("s-t1", "mcp__myceliums__find_callers")], _usage(0, 500, 0), parent="a1"),
            _results(("s-t1", "x" * 400), parent="a1"),
            _asst("s2", [{"type": "text", "text": "x is called by y"}], _usage(0, 120, 500), parent="a1"),
            _results(("a1", "x is called by y")),
            _asst("m2", [{"type": "text", "text": "done"}], _usage(0, 40, 1000)),
            _final(0, 1660, 1500, 10),
        ]
        a = attribute(_write(ev))
        self.assertEqual(a.shrinks, 0)
        cats = a.input_by_category()
        self.assertAlmostEqual(cats["base"], 1000 * 2 + 500 * 2)
        self.assertGreater(cats["myceliums"], 0)
        self.assertAlmostEqual(a.total_input(), 1000 + 500 + 620 + 1040)
        self.assertEqual(sum(a.residual.values()), 0)

    def test_a_shrinking_context_scales_the_segments(self):
        ev = [
            _asst("m1", [_tool("t1", "Grep")], _usage(0, 1000, 0)),
            _results(("t1", "g" * 400)),
            _asst("m2", [_tool("t2", "Read")], _usage(0, 200, 1000)),
            _results(("t2", "r")),
            _asst("m3", [{"type": "text", "text": "x"}], _usage(0, 600, 0)),  # compacted
            _final(0, 1800, 1000, 3),
        ]
        a = attribute(_write(ev))
        self.assertEqual(a.shrinks, 1)
        self.assertAlmostEqual(a.total_input(), 1000 + 1200 + 600)

    def test_without_usage_it_falls_back_to_characters(self):
        ev = [
            {"type": "assistant", "message": {"id": "m1", "model": M, "content": [_tool("t1", "Grep")]}},
            _results(("t1", "g" * 400)),
            {"type": "assistant", "message": {"id": "m2", "model": M, "content": [{"type": "text", "text": "x"}]}},
            _final(10, 0, 0, 5),
        ]
        a = attribute(_write(ev))
        self.assertEqual(a.method, "chars/4")
        self.assertGreater(a.input_by_category()["search"], 0)
        self.assertAlmostEqual(a.total_input(), 10)  # scaled to the session total
        self.assertEqual(a.residual, {})
        self.assertAlmostEqual(a.cost_usd, (10 * PRICES[M]["input"] + 5 * PRICES[M]["output"]) / 1e6)

    def test_row_fields(self):
        row = attribute(_write(self._session())).as_row()
        self.assertEqual(row["attribution_method"], "usage-delta")
        self.assertEqual(set(row["tokens_by_category"]) - {"output"},
                         {"base", "myceliums", "search", "hook", "read", "assistant"})
        self.assertIn("cache_write_1h", row["cost_by_class"])

    def test_prices(self):
        self.assertIs(price_of("claude-sonnet-5-5-20261101", PRICES), PRICES[M])
        self.assertIsNone(price_of("gpt-x", PRICES))
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            json.dump({M: {"output": 99.0}, "my-model": {"input": 1, "cache_write_5m": 1, "cache_write_1h": 1,
                                                         "cache_read": 1, "output": 1}}, fh)
        table = load_prices(fh.name)
        self.assertEqual(table[M]["output"], 99.0)
        self.assertEqual(table[M]["input"], PRICES[M]["input"])
        self.assertIn("my-model", table)

    def test_unknown_model_is_not_priced(self):
        ev = [dict(e) for e in self._session()]
        a = attribute(_write(ev), prices={})
        self.assertIsNone(a.cost_usd)
        self.assertEqual(a.unpriced, [M])


class PerSolved(unittest.TestCase):
    def test_ratio_of_sums(self):
        # Task 3 is solved by nobody in A but still adds its spend.
        per_task = [(10.0, 1, 20.0, 1), (10.0, 1, 20.0, 1), (10.0, 0, 20.0, 2)]
        s = per_success_bootstrap(per_task, n_resamples=500)
        self.assertAlmostEqual(s["a"], 15.0)
        self.assertAlmostEqual(s["b"], 15.0)
        self.assertAlmostEqual(s["ratio"], 1.0)
        self.assertLessEqual(s["ratio_lo"], 1.0)
        self.assertGreaterEqual(s["ratio_hi"], 1.0)

    def test_constant_ratio_has_a_degenerate_interval(self):
        s = per_success_bootstrap([(2.0, 1, 1.0, 1)] * 8, n_resamples=200)
        self.assertAlmostEqual(s["ratio_lo"], 2.0)
        self.assertAlmostEqual(s["ratio_hi"], 2.0)
        self.assertEqual(s["skipped"], 0)

    def test_no_success_at_all(self):
        s = per_success_bootstrap([(1.0, 0, 1.0, 1)], n_resamples=50)
        self.assertNotEqual(s["a"], s["a"])  # NaN
        self.assertEqual(s["skipped"], 50)


def _trow(task, cond, b, cost, solved, cats):
    return {"run_id": f"t{b}/{task}__{cond}__r1", "task_id": task, "condition": cond, "rep": 1, "budget": b,
            "success": solved, "total_input_tokens": sum(v for k, v in cats.items() if k != "output"),
            "output_tokens": cats.get("output", 0), "cost_usd": cost, "attribution_method": "usage-delta",
            "tokens_by_category": cats, "result_tokens_by_category": {k: v / 4 for k, v in cats.items()},
            "tokens_by_class": {}, "thinking_tokens": 0, "attribution_residual": {"input_tokens": 0},
            "cost_by_category": {k: cost * v / sum(cats.values()) for k, v in cats.items()},
            "cost_by_class": {"cache_read": cost / 2, "output": cost / 2}, "priced_cost_usd": cost}


class TokenReport(unittest.TestCase):
    def test_render_check_and_rerun_comparison(self):
        with tempfile.TemporaryDirectory() as tmp:
            old = config.RESULTS_DIR
            config.RESULTS_DIR = Path(tmp)
            try:
                base, rerun = Path(tmp) / "base", Path(tmp) / "rerun"
                base.mkdir()
                rerun.mkdir()
                rows = []
                for b in (10, 40):
                    for i in range(4):
                        rows.append(_trow(f"t{i}", "none", b, 0.10, True, {"base": 1000, "search": 600, "output": 50}))
                        rows.append(_trow(f"t{i}", "C", b, 0.08, i > 0,
                                          {"base": 1500, "myceliums": 300, "search": 100, "output": 40}))
                (base / "tokens.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
                (base / "meta.json").write_text(json.dumps({"primary_exclude": {"t3": "no data"}}))
                self.assertTrue(report_tokens.write("base"))
                self.assertTrue(report_tokens.write("base", check=True))
                text = (base / "TOKENS.md").read_text()
                summary = json.loads((base / "tokens_summary.json").read_text())
                self.assertIn("Tables cover the 3 tasks", text)
                self.assertIn("| myceliums results (MCP + `myc` CLI) | 0 | 300 | +300 |", text)
                self.assertIn("## All budgets together", text)
                g = summary["analysis"]["10"]
                self.assertEqual((g["none"]["solved"], g["C"]["solved"]), (3, 2))
                self.assertAlmostEqual(g["per_solved_cost"]["b"], 0.10)
                self.assertAlmostEqual(g["per_solved_cost"]["a"], 0.08 * 3 / 2)
                (base / "TOKENS.md").write_text(text + "x")
                self.assertFalse(report_tokens.write("base", check=True))
                # A re-run of C is compared with the base run's none rows.
                new = [_trow(f"t{i}", "C", 10, 0.05, True, {"base": 1200, "myceliums": 200, "output": 30})
                       for i in range(4)]
                (rerun / "tokens.jsonl").write_text("".join(json.dumps(r) + "\n" for r in new))
                (rerun / "meta.json").write_text(json.dumps({"rerun_of": "base", "primary_exclude": {"t3": "x"}}))
                report_tokens.write("rerun")
                s2 = json.loads((rerun / "tokens_summary.json").read_text())
                self.assertEqual(list(s2["analysis"]), ["10"])
                self.assertEqual(s2["analysis"]["10"]["none"]["sessions"], 3)
                self.assertEqual(s2["reconciliation"]["sessions"], 4)  # only the run's own sessions
            finally:
                config.RESULTS_DIR = old

    def test_committed_token_reports_are_current(self):
        for d in sorted(config.RESULTS_DIR.iterdir()):
            if (d / "tokens.jsonl").exists():
                self.assertTrue(report_tokens.write(d.name, check=True), d.name)

    def test_committed_attribution_reconciles(self):
        # The #163 / #171 sessions: zero residual, and the categories add up to
        # each session's total input (to the 0.1-token rounding of tokens.jsonl).
        for label in ("2026-11-22-scale", "2026-11-22-rerun-c"):
            rows = report_tokens._rows(config.RESULTS_DIR / label / "tokens.jsonl")
            self.assertTrue(rows)
            for r in rows:
                self.assertFalse(any(r["attribution_residual"].values()), r["run_id"])
                got = sum(v for k, v in r["tokens_by_category"].items() if k != "output")
                self.assertLess(abs(got - r["total_input_tokens"]), 1.0, r["run_id"])
                self.assertLess(abs(r["priced_cost_usd"] - r["cost_usd"]), 1e-5, r["run_id"])


def _runner(conditions=("A", "D"), **kw):
    tasks = config.load_tasks()[:2]
    return Runner(RunConfig(label="unit", conditions=conditions, claude="claude", **kw), tasks,
                  config.load_repos()), tasks


EDIT_TASK = {"id": "hono-edit-x", "repo": "hono", "type": "edit", "answer_kind": "diff",
             "question": "Rename parseAccept to parseAcceptHeader everywhere.", "rationale": "unit",
             "gold_files": ["src/utils/accept.ts", {"path": "src/utils/accept.test.ts", "status": "M"},
                            {"path": "docs/accept.md", "status": "A"}],
             "dependents": ["src/utils/accept.test.ts"], "test_command": "true"}


class ArmD(unittest.TestCase):
    def test_d_offers_no_search_and_blocks_it(self):
        r, tasks = _runner(bash=True)
        it = WorkItem(tasks[0], "D", 1)
        with tempfile.TemporaryDirectory() as tmp:
            settings = r._nosearch_settings(Path(tmp))
            cmd = r._command(it, Path("/x/mcp.json"), settings)
            hook = json.loads(settings.read_text())["hooks"]["PreToolUse"][0]
        tools = cmd[cmd.index("--tools") + 1].split(",")
        self.assertEqual(sorted(tools), ["Bash", "Read", "ToolSearch"])
        denied = cmd[cmd.index("--disallowedTools") + 1].split(",")
        self.assertTrue({"Grep", "Glob", "Bash(grep:*)", "Bash(rg:*)", "Bash(find:*)", "Bash(git grep:*)"}
                        <= set(denied))
        self.assertIn("mcp__myceliums", cmd[cmd.index("--allowedTools") + 1])
        self.assertEqual(cmd[cmd.index("--settings") + 1], str(settings))
        self.assertEqual(hook["matcher"], "Bash")
        self.assertIn("bashcmd.py", hook["hooks"][0]["command"])
        self.assertTrue(config.VARIANTS["D"].mcp and not config.VARIANTS["D"].setup_claude)

    def test_a_is_unchanged(self):
        r, tasks = _runner(bash=True)
        cmd = r._command(WorkItem(tasks[0], "A", 1), Path("/x/mcp.json"))
        self.assertNotIn("--disallowedTools", cmd)
        self.assertNotIn("--settings", cmd)
        self.assertEqual(cmd[cmd.index("--tools") + 1], "Read,Grep,Glob,ToolSearch,Bash")

    def test_isolation_check_of_a_d_session(self):
        r, tasks = _runner()
        it = WorkItem(tasks[0], "D", 1)
        init = {"type": "system", "subtype": "init", "tools": ["Read", "ToolSearch", "mcp__myceliums__find_callers"],
                "mcp_servers": [{"name": "myceliums", "status": "connected"}], "slash_commands": [], "skills": [],
                "model": M}
        with tempfile.TemporaryDirectory() as tmp:
            ok = Path(tmp) / "ok.jsonl"
            ok.write_text(json.dumps(init) + "\n" + json.dumps(_final(1, 0, 0, 1)) + "\n")
            self.assertEqual(r._summarise(it, 1, ok, 1.0, False, 0)["isolation_problems"], [])
            leaked = Path(tmp) / "leaked.jsonl"
            leaked.write_text(json.dumps(dict(init, tools=init["tools"] + ["Grep"])) + "\n")
            row = r._summarise(it, 1, leaked, 1.0, False, 0)
            self.assertEqual(row["status"], "isolation_error")
            self.assertIn("tokens_by_category", row)

    def test_mock_api_scripts_a_blocked_grep_then_myc(self):
        with tempfile.TemporaryDirectory() as tmp:
            api = mockapi.MockAPI(Path(tmp), "parseAccept", "parseAccept")
            tools = [{"name": "Bash"}, {"name": "Read"}]
            first = api.answer({"tools": tools, "messages": []}).decode()
            self.assertIn("grep -rl parseAccept", first)
            msgs = [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x"}]}]
            second = api.answer({"tools": tools, "messages": msgs}).decode()
            self.assertIn("myc callers parseAccept", second)
            self.assertNotIn("grep", second.split("myc callers")[1].split('"')[0])
            api.server.server_close()

    def test_d_is_a_known_condition(self):
        import agent_eval
        self.assertIn("D", config.CONDITIONS)
        with self.assertRaises(SystemExit):
            agent_eval._run_config(_args(conditions="A,Q"))


def _args(**kw):
    import argparse
    base = dict(label="x", model="sonnet", effort="medium", max_turns=40, timeout=900, reps=3, concurrency=1,
                max_retries=0, threshold=0.8, max_utilization=0.97, max_sessions=None, seed=1, tool_search=None,
                claude="/bin/echo", myc="", models_dir="", conditions="A,D", prompt_budget=False, bash=False,
                profile=None)
    base.update(kw)
    return argparse.Namespace(**base)


class Profile(unittest.TestCase):
    def test_realistic_is_a_cap_of_100_with_three_reps(self):
        import agent_eval
        cfg = agent_eval._run_config(_args(profile="realistic", max_turns=10, reps=1))
        self.assertEqual((cfg.max_turns, cfg.reps, cfg.prompt_budget, cfg.profile), (100, 3, False, "realistic"))
        self.assertEqual(cfg.comparable()["profile"], "realistic")
        self.assertNotIn("profile", agent_eval._run_config(_args()).comparable())
        with self.assertRaises(SystemExit):
            agent_eval._run_config(_args(profile="realistic", prompt_budget=True))

    def test_the_cap_is_not_in_the_prompt(self):
        repos = config.load_repos()
        task = config.load_tasks()[0]
        r = Runner(RunConfig(label="x", max_turns=100, profile="realistic"), [task], repos)
        prompt = build_prompt(task, repos[task["repo"]], r.cfg.max_turns if r.cfg.prompt_budget else None)
        self.assertNotIn("100", prompt)
        self.assertNotIn("Budget", prompt)


def _git(*args, cwd):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


class EditTasks(unittest.TestCase):
    def test_file_kind(self):
        self.assertEqual(editgrade.file_kind("src/utils/accept.ts"), "code")
        self.assertEqual(editgrade.file_kind("src/utils/accept.test.ts"), "tests")
        self.assertEqual(editgrade.file_kind("tests/test_views.py"), "tests")
        self.assertEqual(editgrade.file_kind("django/test/utils.py"), "tests")
        self.assertEqual(editgrade.file_kind("pkg/server_test.go"), "tests")
        self.assertEqual(editgrade.file_kind("docs/ref/settings.txt"), "docs")
        self.assertEqual(editgrade.file_kind("CHANGELOG.md"), "docs")
        self.assertEqual(editgrade.file_kind("django/conf/global_settings.py"), "code")

    def test_grading_by_kind_and_forgotten_dependents(self):
        g = editgrade.grade_edit(EDIT_TASK, ["src/utils/accept.ts", "src/other.ts"], None)
        self.assertEqual(g["files_by_kind"]["code"], {"precision": 0.5, "recall": 1.0, "f1": 0.6667, "tp": 1,
                                                      "n_gold": 1, "n_changed": 2})
        self.assertEqual(g["files_by_kind"]["tests"]["recall"], 0.0)
        self.assertEqual(g["files_by_kind"]["docs"]["recall"], 0.0)
        self.assertEqual(g["dependents_forgotten"], ["src/utils/accept.test.ts"])
        self.assertEqual(g["files_extra"], ["src/other.ts"])
        self.assertAlmostEqual(g["file_recall"], 1 / 3, places=3)
        self.assertFalse(g["success"])  # no test result: code F1 0.67 < 0.8
        self.assertTrue(editgrade.grade_edit(EDIT_TASK, ["src/utils/accept.ts"], None)["success"])
        # With a test command, the test decides.
        self.assertTrue(editgrade.grade_edit(EDIT_TASK, ["src/other.ts"], {"passed": True})["success"])
        self.assertFalse(editgrade.grade_edit(EDIT_TASK, list(editgrade.gold_paths(EDIT_TASK)),
                                              {"passed": False})["success"])
        no_dep = dict(EDIT_TASK)
        no_dep.pop("dependents")
        self.assertEqual(len(editgrade.grade_edit(no_dep, [], None)["dependents_forgotten"]), 3)

    def test_changed_files_and_test_command_in_a_working_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            w = Path(tmp)
            _git("init", "-q", cwd=w)
            (w / "a.py").write_text("x = 1\n")
            (w / "b.py").write_text("y = 1\n")
            (w / "old.py").write_text("z = 1\n")
            _git("add", ".", cwd=w)
            _git("-c", "user.email=e@x", "-c", "user.name=n", "commit", "-qm", "init", cwd=w)
            (w / "a.py").write_text("x = 2\n")
            (w / "b.py").unlink()
            _git("mv", "old.py", "new.py", cwd=w)
            (w / "docs").mkdir()
            (w / "docs" / "c.md").write_text("doc\n")
            self.assertEqual(editgrade.changed_files(w), ["a.py", "b.py", "docs/c.md", "new.py", "old.py"])
            t = editgrade.run_test("grep -q 'x = 2' a.py", w)
            self.assertTrue(t["passed"])
            self.assertFalse(editgrade.run_test("exit 3", w)["passed"])
            self.assertTrue(editgrade.run_test("sleep 5", w, timeout_s=1)["timed_out"])

    def test_edit_task_schema(self):
        repos = config.load_repos()
        self.assertEqual(check_tasks([EDIT_TASK], repos, check_evidence=False), [])
        bad = dict(EDIT_TASK, gold_files=[{"path": "x", "status": "Z"}], dependents=["y"], type="callers")
        problems = check_tasks([bad], repos, check_evidence=False)
        self.assertEqual(len(problems), 3, problems)

    def test_edit_tasks_get_the_edit_tools_and_their_own_prompt(self):
        repos = config.load_repos()
        r = Runner(RunConfig(label="x", conditions=("none", "D"), claude="claude"), [EDIT_TASK], repos)
        for cond in ("none", "D"):
            cmd = r._command(WorkItem(EDIT_TASK, cond, 1), Path("/x/mcp.json"))
            tools = cmd[cmd.index("--tools") + 1].split(",")
            self.assertIn("Edit", tools)
            self.assertIn("Write", tools)
        prompt = build_prompt(EDIT_TASK, repos["hono"])
        self.assertIn(EDIT_TASK["question"], prompt)
        self.assertNotIn('"answer"', prompt)
        self.assertIn("Budget", build_prompt(EDIT_TASK, repos["hono"], 10))
        # Question tasks never get them.
        q = config.load_tasks()[0]
        cmd = r._command(WorkItem(q, "none", 1), Path("/x/mcp.json"))
        self.assertNotIn("Edit", cmd[cmd.index("--tools") + 1])

    def test_summarise_an_edit_session(self):
        r = Runner(RunConfig(label="x", conditions=("D",), claude="claude"), [EDIT_TASK], config.load_repos())
        it = WorkItem(EDIT_TASK, "D", 1)
        init = {"type": "system", "subtype": "init", "tools": ["Read", "Edit", "Write",
                                                               "mcp__myceliums__find_callers"],
                "mcp_servers": [{"name": "myceliums", "status": "connected"}], "slash_commands": [], "skills": []}
        edit = editgrade.grade_edit(EDIT_TASK, ["src/utils/accept.ts", "src/utils/accept.test.ts"],
                                    {"passed": True})
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "t.jsonl"
            p.write_text(json.dumps(init) + "\n" + json.dumps(_final(1, 0, 0, 1)) + "\n")
            row = r._summarise(it, 1, p, 1.0, False, 0, edit)
        self.assertEqual(row["status"], "ok")
        self.assertEqual(row["isolation_problems"], [])
        self.assertTrue(row["success"])
        self.assertEqual(row["tp"], 2)
        self.assertEqual(row["missed"], [])
        self.assertAlmostEqual(row["recall"], 2 / 3, places=3)
        self.assertEqual(row["edit"]["files_by_kind"]["tests"]["recall"], 1.0)


if __name__ == "__main__":
    unittest.main()
