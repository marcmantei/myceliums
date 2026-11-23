"""#178: per-tool token attribution, cost by cache class, spend per solved
task, arm D ("myceliums only"), edit-task grading and the realistic profile."""

import io
import json
import os
import shlex
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


# Commands #163 agents ran, and the bypasses the review of #185 listed.
SEARCH_FORMS = [
    "cd django; grep -rn x . | head", 'bash -c "grep -rn x ."', "sh -c 'rg foo'", "timeout 10 grep x f",
    "timeout -s KILL 5 rg x", "nice -n 5 grep x", "stdbuf -oL grep x", "xargs -n 1 grep x", "git -C . grep foo",
    "git --no-pager grep foo", "\\grep foo", "eval grep foo", "ugrep x", "awk '/x/' f", "perl -ne 'print if /x/' f",
    "sed -n '/re/p' f", "git ls-files", "ls -R src", "tree src", "git log -S foo", "echo $(find . -name '*.py')",
    'echo "== $(grep -rl x .)"', "for f in a b; do grep -n x $f; done",
    "cd p; for s in a.ts:1; do f=${s%:*}; n=${s#*:}; awk -v n=$n 'NR<=n' $f; done",
]
HEREDOC_WRITE = "cat " + ">" + " f.py <<'EOF'\ngrep x\nEOF"


class BashClassifier(unittest.TestCase):
    def test_programs_of_a_pipeline(self):
        self.assertEqual(bashcmd.classify("sed -n 1,9p a.py"), "read")
        self.assertEqual(bashcmd.classify("sed -n 1,9p a.py && rg foo"), "search")
        self.assertEqual(bashcmd.classify("myc callers x 2>&1 | grep -v y"), "myceliums")
        self.assertEqual(bashcmd.classify("/tmp/bin/myc impact X"), "myceliums")
        self.assertEqual(bashcmd.classify("sed -i 's/a/b/' f.py"), "edit")
        self.assertEqual(bashcmd.classify("ls -la src && git log -1"), "other")
        self.assertEqual(bashcmd.classify('myc search_code "a|b; grep"'), "myceliums")  # quoted: one word
        self.assertEqual(bashcmd.classify(HEREDOC_WRITE), "read")  # the heredoc body is not run
        self.assertEqual(bashcmd.classify("python3 - <<'E'\nimport os\nE"), "other")
        self.assertTrue(bashcmd.interpreted("cd x; python3 - <<'E'\nprint(1)\nE"))

    def test_every_search_form_is_search(self):
        for cmd in SEARCH_FORMS:
            self.assertEqual(bashcmd.classify(cmd), "search", cmd)

    def test_d_allowlist(self):
        for cmd in SEARCH_FORMS + ["python3 - <<'E'\nimport os\nE", "cat a.py", "head -5 a.py", "echo hi",
                                   "myc callers x | head -5", "node -e 'x'", 'myc x "unclosed', HEREDOC_WRITE]:
            self.assertFalse(bashcmd.allowed(cmd)[0], cmd)
        for cmd in ("myc callers parseAccept", "cd src && myc impact X 2>/dev/null", "git status", "git diff --stat",
                    "git add -A && git commit -m x", "git log -p -3", "mkdir -p a && touch a/b.py", "true"):
            self.assertTrue(bashcmd.allowed(cmd)[0], cmd)
        self.assertTrue(bashcmd.allowed("python -m pytest tests/x.py -q", ["python -m pytest tests/x.py -q"])[0])
        self.assertFalse(bashcmd.allowed("python -m pytest x; grep y z", ["python -m pytest x"])[0])
        self.assertFalse(bashcmd.allowed("python -c 'import os'", ["python -m pytest"])[0])

    def test_hook_blocks_outside_the_allowlist(self):
        err = io.StringIO()
        ev = {"tool_name": "Bash", "tool_input": {"command": "cat x.py | rg foo"}}
        self.assertEqual(bashcmd.hook([], io.StringIO(json.dumps(ev)), err), 2)
        self.assertIn("`cat`, `rg`", err.getvalue())
        self.assertIn("This session has no search", err.getvalue())
        ok = {"tool_name": "Bash", "tool_input": {"command": "./gradlew test --tests X"}}
        self.assertEqual(bashcmd.hook(["--allow", "./gradlew test --tests X"], io.StringIO(json.dumps(ok)),
                                      io.StringIO()), 0)
        rule = json.dumps([{"prefix": "./gradlew test", "args": r"--tests [\w.]+"}])
        self.assertEqual(bashcmd.hook(["--allow-json", rule], io.StringIO(json.dumps(ok)), io.StringIO()), 0)
        self.assertEqual(bashcmd.hook([], io.StringIO(json.dumps(ok)), io.StringIO()), 2)
        self.assertEqual(bashcmd.hook([], io.StringIO(json.dumps({"tool_name": "Read"})), io.StringIO()), 0)
        self.assertEqual(bashcmd.hook([], io.StringIO("not json"), io.StringIO()), 2)

    def test_hook_runs_standalone_in_isolated_mode(self):
        # The session runs `python3 -I bashcmd.py hook ...`: no package imports allowed.
        script = str(Path(bashcmd.__file__))
        for cmd, rc in (('bash -c "grep -rn x ."', 2), ("myc callers x", 0), ("make check", 0)):
            ev = json.dumps({"tool_name": "Bash", "tool_input": {"command": cmd}})
            res = subprocess.run([sys.executable, "-I", script, "hook", "--allow", "make check"], input=ev,
                                 capture_output=True, text=True, cwd=tempfile.gettempdir())
            self.assertEqual(res.returncode, rc, (cmd, res.stderr))

    def test_tool_categories(self):
        self.assertEqual(category_of("mcp__myceliums__find_callers", {}), "myceliums")
        self.assertEqual(category_of("Grep", {}), "search")
        self.assertEqual(category_of("Glob", {}), "search")
        self.assertEqual(category_of("Read", {}), "read")
        self.assertEqual(category_of("Edit", {}), "edit")
        self.assertEqual(category_of("ToolSearch", {}), "other")
        self.assertEqual(category_of("Bash", {"command": "head -50 a.py"}), "read")


class DBypasses(unittest.TestCase):
    """Regression tests for the holes the reviews of #185 found in arm D."""

    def test_heredoc_terminators_follow_bash(self):
        deny = [
            "true <<-EOF\n\tEOF\ngrep -rn foo .",  # `<<-`: a tab-indented terminator closes the heredoc
            "true <<- EOF\n\t\tEOF\ngrep -rn foo .",
            "true <<-'EOF'\n\tEOF\nrg foo",
            "true <<EOF\ngrep -rn foo .",  # never closed: deny the whole command
            "true <<EOF\n  EOF\ngrep x",  # `<<`: an indented line does not close it, so it never closes
            "true <<A <<B\nA\nB\ngrep x",
        ]
        for cmd in deny:
            self.assertFalse(bashcmd.allowed(cmd)[0], cmd)
        for cmd in ("true <<-EOF\n\tEOF\nmyc callers x", "true <<EOF\ngrep inside the body is data\nEOF\ngit status"):
            self.assertTrue(bashcmd.allowed(cmd)[0], cmd)
        self.assertEqual(bashcmd.classify("true <<-EOF\n\tEOF\ngrep -rn foo ."), "search")

    def test_git_cannot_be_made_to_run_a_program(self):
        for cmd in ("git -c core.fsmonitor='grep -rn foo .' status",
                    "git -c diff.external='sh -c \"grep -rn foo .\"' diff",
                    "git -ccore.pager='grep foo' log", "git --config-env=core.pager=X log",
                    "git --exec-path=/tmp/x status", "git --git-dir=/tmp/x status", "git --work-tree=/ status",
                    "GIT_EXTERNAL_DIFF=/tmp/x git diff", "GIT_DIR=/tmp/x git status", "env GIT_DIR=/tmp/x git status",
                    "PAGER='grep x' git log", "nice git status", "mkdir -p x/.git", "mv dotgit .git",
                    "cp cfg .git/config", "rm -rf .git/hooks"):
            self.assertFalse(bashcmd.allowed(cmd)[0], cmd)
        for cmd in ("git -C sub status", "git --no-pager diff --stat", "git add .gitignore", "cp a.py b.py"):
            self.assertTrue(bashcmd.allowed(cmd)[0], cmd)

    def test_redirection_targets_and_file_operands_are_checked(self):
        wc = "/w/repo"
        deny = [
            "git show :payload.txt > .git/config", "git show :p >> .git/hooks/pre-commit", "myc callers x &> .git/x",
            "git show :p >& .git/config", "npx vitest run src/utils/accept.test.ts > .git/config",
            "cp x .gi?/config", "cp x .g*/config", "cd .g*", "cp x .gi[t]/config", "cp x .{git,x}/config",
            "cp x ~/.gitconfig", "cp x $HOME/.gitconfig", "touch ~/.config/git/config", "cd", "cd ~",
            "cp a.py /tmp/b.py", "git show :p > /tmp/x", "cd /tmp && touch x", "mv a.py ../a.py",
            "cd sub && cp x ../../y", "git diff --output=x.patch", "touch .gitconfig",
        ]
        for cmd in deny:
            self.assertFalse(bashcmd.allowed(cmd, ["npx vitest run src/utils/accept.test.ts"], wc)[0], cmd)
        ok = ["myc callers x 2>&1", "myc callers x 2>/dev/null", "git show :p > out.txt", "cd sub && touch ../x.py",
              "cp a.py /w/repo/b.py", "git diff > changes.patch", "mkdir -p src/new && touch src/new/a.py",
              "npx vitest run src/utils/accept.test.ts 2>&1"]
        for cmd in ok:
            self.assertTrue(bashcmd.allowed(cmd, ["npx vitest run src/utils/accept.test.ts"], wc)[0], cmd)
        _, targets = bashcmd.parse("a > x 2>&1 >> y < z <<<w &>v")
        self.assertEqual([t for _, t in targets], ["x", "y", "z", "v"])

    def test_writes_stay_in_the_working_copy(self):
        home = os.path.expanduser("~")
        for path, rc in (("/w/repo/src/a.py", 0), ("src/a.py", 0), ("/w/repo/../x.py", 2), ("/tmp/x.py", 2),
                         (home + "/.gitconfig", 2), (home + "/.config/git/config", 2), ("/w/repo/.git/config", 2)):
            ev = json.dumps({"tool_name": "Write", "cwd": "/w/repo", "tool_input": {"file_path": path}})
            self.assertEqual(bashcmd.hook([], io.StringIO(ev), io.StringIO()), rc, path)
        ev = json.dumps({"tool_name": "Bash", "cwd": "/w/repo", "tool_input": {"command": "git show :p > .git/config"}})
        err = io.StringIO()
        self.assertEqual(bashcmd.hook([], io.StringIO(ev), err), 2)
        self.assertIn("redirection", err.getvalue())

    def test_no_edits_inside_dot_git(self):
        for tool, inp, rc in (("Write", {"file_path": "/w/.git/config"}, 2),
                              ("Edit", {"file_path": "/w/.git/hooks/pre-commit"}, 2),
                              ("Write", {"file_path": "/tmp/x/.git/config"}, 2),
                              ("Write", {"file_path": "/w/src/a.py"}, 0),
                              ("Edit", {"file_path": "/w/.gitignore"}, 0)):
            ev = json.dumps({"tool_name": tool, "tool_input": inp})
            self.assertEqual(bashcmd.hook([], io.StringIO(ev), io.StringIO()), rc, inp)

    def test_task_commands_match_exactly_or_by_their_pattern(self):
        exact = ["npx vitest run src/a.test.ts"]
        self.assertTrue(bashcmd.allowed("npx vitest run src/a.test.ts", exact)[0])
        for cmd in ("npx vitest run src/a.test.ts --config evil.ts", "npx vitest run", "npx vitest"):
            self.assertFalse(bashcmd.allowed(cmd, exact)[0], cmd)
        pattern = [{"prefix": "npx vitest run", "args": r"src/[\w/]+\.test\.ts"}]
        self.assertTrue(bashcmd.allowed("npx vitest run src/utils/b.test.ts", pattern)[0])
        for cmd in ("npx vitest run src/b.test.ts --config x", "npx vitest run ../x.test.ts", "npx vitest run"):
            self.assertFalse(bashcmd.allowed(cmd, pattern)[0], cmd)
        # A compound test command allows each of its simple commands exactly.
        both = ["cd packages/x && npm test -- --run"]
        self.assertTrue(bashcmd.allowed("cd packages/x && npm test -- --run", both)[0])
        self.assertFalse(bashcmd.allowed("npm test -- --run --watch", both)[0])
        self.assertTrue(check_tasks([dict(EDIT_TASK, allowed_commands=exact + pattern)], config.load_repos(),
                                    check_evidence=False) == [])
        bad = dict(EDIT_TASK, allowed_commands=[{"prefix": "npx", "args": "("}, {"prefix": "npx"}, ""])
        self.assertEqual(len(check_tasks([bad], config.load_repos(), check_evidence=False)), 3)

    def test_the_session_git_env_disables_hooks_and_fsmonitor(self):
        from aeval.runner import nosearch_git_env
        with tempfile.TemporaryDirectory() as tmp:
            w = Path(tmp) / "w"
            w.mkdir()
            _git("init", "-q", cwd=w)
            marker = Path(tmp) / "ran"
            hook = w / ".git" / "hooks" / "pre-commit"
            hook.write_text(f"#!/bin/sh\ntouch {marker}\n")
            hook.chmod(0o755)
            _git("config", "core.fsmonitor", f"touch {marker}.fsmonitor; false", cwd=w)
            (w / "a.py").write_text("x = 1\n")
            base = {"PATH": "/usr/bin:/bin", "HOME": tmp, "GIT_CONFIG_GLOBAL": "/dev/null"}
            env = {**base, **nosearch_git_env(Path(tmp))}
            self.assertEqual(env["GIT_CONFIG_GLOBAL"], "/dev/null")
            self.assertEqual(env["GIT_CONFIG_NOSYSTEM"], "1")
            _git("config", "core.pager", f"touch {marker}.pager; cat", cwd=w)
            got = subprocess.run(["git", "config", "core.pager"], cwd=w, env=env, capture_output=True, text=True)
            self.assertEqual(got.stdout.strip(), "cat")  # the session's pin wins over the repository's config
            for args in (["add", "a.py"], ["-c", "user.email=e@x", "-c", "user.name=n", "commit", "-qm", "x"],
                         ["status"]):
                subprocess.run(["git", *args], cwd=w, env=env, check=True, capture_output=True)
            self.assertFalse(marker.exists())
            self.assertFalse(Path(f"{marker}.fsmonitor").exists())
            # Without it, the same commit would have run the hook.
            (w / "b.py").write_text("y = 1\n")
            subprocess.run(["git", "add", "b.py"], cwd=w, env=base, check=True, capture_output=True)
            subprocess.run(["git", "-c", "user.email=e@x", "-c", "user.name=n", "commit", "-qm", "y"], cwd=w,
                           env=base, capture_output=True)
            self.assertTrue(marker.exists())


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

    def test_one_hour_writes_come_before_five_minute_writes(self):
        # Call 2 writes 100 tokens at 1 h and 200 at 5 min: the earlier part of its growth (the reply,
        # then the Grep result) is the 1-hour write.
        mixed = {"input_tokens": 0, "cache_creation_input_tokens": 300, "cache_read_input_tokens": 1000,
                 "output_tokens": 1, "cache_creation": {"ephemeral_1h_input_tokens": 100,
                                                        "ephemeral_5m_input_tokens": 200}}
        ev = [_asst("m1", [{"type": "text", "text": "r" * 400}], _usage(0, 1000, 0)),
              _results(),
              {"type": "user", "message": {"content": "u" * 800}},
              _asst("m2", [{"type": "text", "text": "done"}], mixed),
              _final(0, 1300, 1000, 2)]
        a = attribute(_write(ev))
        self.assertAlmostEqual(a.tokens["assistant"]["cache_write_1h"], 100)
        self.assertAlmostEqual(a.tokens["assistant"]["cache_write_5m"], 0)
        self.assertAlmostEqual(a.tokens["other"]["cache_write_5m"], 200)

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
        self.assertEqual(row["models"], [M])
        self.assertAlmostEqual(row["estimated_tokens_by_category"]["myceliums"], 100.0)
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


def _calls_session(rng, true):
    """A session whose context growth follows ``true`` tokens per char for each part."""
    ev, ctx = [], 1000
    ev.append(_asst("m0", [_tool("t0", "Grep"), _tool("c1", "mcp__myceliums__x"), _tool("r1", "Read")],
                    _usage(0, ctx, 0)))
    for i in range(1, 25):
        myc, grep, read = (rng.randint(0, 4000) for _ in range(3))
        ev.append(_results(("t%d" % (i - 1), "g" * grep)))
        ev[-1]["message"]["content"].append({"type": "tool_result", "tool_use_id": "c%d" % i, "content": "m" * myc})
        ev[-1]["message"]["content"].append({"type": "tool_result", "tool_use_id": "r%d" % i, "content": "x" * read})
        reply = 3 * len("{}") + len("Grep") + len("mcp__myceliums__x") + len("Read")  # the previous reply
        grow = int(true["search"] * grep + true["myceliums"] * myc + true["read"] * read + true["reply"] * reply
                   + 30 + rng.randint(-3, 3))
        ev.append(_asst("m%d" % i, [_tool("t%d" % i, "Grep"), _tool("c%d" % (i + 1), "mcp__myceliums__x"),
                                    _tool("r%d" % (i + 1), "Read")], _usage(0, grow, ctx)))
        ctx += grow
    ev.append(_final(0, ctx, 0, 1))
    return ev


class CrossCheck(unittest.TestCase):
    def test_the_fit_recovers_known_rates(self):
        import random
        from aeval.tokens import fit_rates
        rng = random.Random(3)
        true = {"search": 0.40, "myceliums": 0.50, "read": 0.30, "reply": 0.45}
        samples = []
        for _ in range(4):
            samples += attribute(_write(_calls_session(rng, true))).samples
        fit = fit_rates(samples)
        self.assertGreater(fit["r2"], 0.99)
        for f in ("search", "myceliums", "read"):
            self.assertAlmostEqual(fit["features"][f]["tokens_per_char"], true[f], delta=0.01)
        self.assertNotIn("thinking", fit["features"])

    def test_fitted_rates_move_the_split(self):
        from aeval.tokens import rates_of
        ev = [_asst("m1", [_tool("t1", "Grep"), _tool("t2", "mcp__myceliums__x")], _usage(0, 1000, 0)),
              _results(("t1", "g" * 400), ("t2", "m" * 400)),
              _asst("m2", [{"type": "text", "text": "x"}], _usage(0, 230, 1000)),
              _final(0, 1230, 1000, 2)]
        plain = attribute(_write(ev))
        fitted = attribute(_write(ev), rates=rates_of({"features": {"myceliums": {"coef": 2.0},
                                                                    "search": {"coef": 1.0}}}))
        self.assertAlmostEqual(plain.result_tokens["myceliums"], plain.result_tokens["search"])
        self.assertAlmostEqual(fitted.result_tokens["myceliums"] / fitted.result_tokens["search"], 2.0, places=1)
        self.assertAlmostEqual(fitted.total_input(), plain.total_input())

    def test_committed_crosscheck(self):
        # The fit over the 850 main-agent calls of #163, as committed: R² and the rates of
        # myceliums results and grep output agree within 10 %.
        cross = config.load_json(config.RESULTS_DIR / "2026-11-22-scale" / "tokens_crosscheck.json")
        fit = cross["fit"]
        self.assertEqual(fit["n_calls"], 850)
        self.assertGreater(fit["r2"], 0.95)
        m, g = fit["features"]["myceliums"]["coef"], fit["features"]["search"]["coef"]
        self.assertLess(abs(m / g - 1), 0.10)


class SearchAudit(unittest.TestCase):
    def test_counts_and_flags(self):
        from aeval.searchaudit import audit
        blocked = "PreToolUse:Bash hook error: [...]: This session has no search and only a few shell commands"
        ev = [
            _asst("m1", [_tool("b1", "Bash", command="git -C . grep foo"),
                         _tool("b2", "Bash", command="myc callers foo"),
                         _tool("r1", "Read", file_path="/w/src/known.ts")], _usage(0, 10, 0)),
            _results(("b1", blocked), ("b2", "foo: 2 callers in src/caller.ts"), ("r1", "...")),
            _asst("m2", [_tool("r2", "Read", file_path="/w/src/caller.ts")]
                  + [_tool("x%d" % i, "Read", file_path="/w/src/blind%d.ts" % i) for i in range(5)],
                  _usage(0, 10, 10)),
            _asst("m3", [_tool("b3", "Bash", command="python3 - <<'E'\nimport os\nE")], _usage(0, 10, 20)),
            _results(("b3", "a\nb")),
            _final(0, 30, 30, 1),
        ]
        a = audit(_write(ev), prompt="Who calls foo in src/known.ts?")
        self.assertEqual((a["search_attempts"], a["search_blocked"], a["search_leaks"]), (1, 1, 0))
        self.assertEqual((a["interpreted_calls"], a["interpreted_ran"]), (1, 1))
        self.assertEqual((a["reads"], a["files_read"], a["blind_reads"], a["max_reads_per_turn"]), (7, 7, 5, 6))
        self.assertTrue(a["flagged"])
        clean = audit(_write(ev[:2] + [_final(0, 10, 0, 1)]), prompt="src/known.ts")
        self.assertFalse(clean["flagged"])
        self.assertEqual(clean["blind_reads"], 0)


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
            "cost_by_class": {"cache_read": cost / 2, "output": cost / 2}, "priced_cost_usd": cost, "models": [M]}


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
                (base / "tokens_crosscheck.json").write_text(json.dumps({"available": True, "fit": {
                    "n_calls": 40, "r2": 0.97, "constant": 30.0, "left_out": ["edit"],
                    "features": {"search": {"coef": 1.7, "n_calls": 40, "tokens_per_char": 0.425}}}}))
                self.assertTrue(report_tokens.write("base"))
                self.assertTrue(report_tokens.write("base", check=True))
                text = (base / "TOKENS.md").read_text()
                summary = json.loads((base / "tokens_summary.json").read_text())
                self.assertIn("Tables cover the 3 tasks", text)
                self.assertIn("| myceliums results (MCP + `myc` CLI) | 0 | 300 | +300 |", text)
                self.assertIn("## All budgets together", text)
                self.assertIn("`claude-sonnet-5-5`: uncached input $2", text)
                self.assertIn("https://platform.claude.com/docs/en/about-claude/pricing", text)
                self.assertIn("| search | 40 | 1.700 | 0.425 |", text)
                self.assertIn("zero by construction", text)
                self.assertEqual(summary["models"], [M])
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
                            {"path": "docs/accept.md", "status": "A"}, "src/helper/accepts/accepts.ts",
                            "src/utils/mime.ts"],
             "primary_files": ["src/utils/accept.ts"], "test_command": "npx vitest run src/utils/accept.test.ts",
             "precheck": {"fails_on_parent": True, "returncode": 1, "timed_out": False,
                          "commit": "f6fbaf0299c48eea9e656e03e2ffe7ac755e8823", "command": "npx vitest run src/utils/accept.test.ts",
                          "checked_at": "2026-11-23T00:00:00Z"}}


class ArmD(unittest.TestCase):
    def test_d_offers_no_search_and_blocks_it(self):
        r, tasks = _runner(bash=True)
        it = WorkItem(tasks[0], "D", 1)
        with tempfile.TemporaryDirectory() as tmp:
            settings = r._nosearch_settings(Path(tmp), EDIT_TASK)
            cmd = r._command(it, Path("/x/mcp.json"), settings)
            hook = json.loads(settings.read_text())["hooks"]["PreToolUse"][0]
            # The hook command, run as the session runs it, admits the task's test command only.
            argv = shlex.split(hook["hooks"][0]["command"])
            for command, rc in (("npx vitest run src/utils/accept.test.ts", 0), ("npx tsx x.ts", 2),
                                ("git -C . grep x", 2)):
                ev = json.dumps({"tool_name": "Bash", "tool_input": {"command": command}})
                res = subprocess.run(argv, input=ev, capture_output=True, text=True)
                self.assertEqual(res.returncode, rc, (command, res.stderr))
        tools = cmd[cmd.index("--tools") + 1].split(",")
        self.assertEqual(sorted(tools), ["Bash", "Read", "ToolSearch"])
        denied = cmd[cmd.index("--disallowedTools") + 1].split(",")
        self.assertTrue({"Grep", "Glob", "Bash(grep:*)", "Bash(rg:*)", "Bash(find:*)", "Bash(git grep:*)"}
                        <= set(denied))
        self.assertIn("mcp__myceliums", cmd[cmd.index("--allowedTools") + 1])
        self.assertEqual(cmd[cmd.index("--settings") + 1], str(settings))
        self.assertEqual(hook["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit")
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
        cases = {
            "src/utils/accept.ts": "code", "src/utils/accept.test.ts": "tests", "tests/test_views.py": "tests",
            "django/test/utils.py": "code",  # Django's test framework is library code
            "django/contrib/auth/test_utils.py": "tests", "pkg/server_test.go": "tests",
            "spring-test/src/main/java/org/x/MockMvc.java": "code",  # src/main wins over the module name
            "spring-web/src/test/java/org/x/FooTests.java": "tests",
            "spring-core/src/testFixtures/java/org/x/Fixture.java": "tests",
            "spring-core/src/integrationTest/java/org/x/ItTest.java": "tests",
            "spring-core/src/main/java/org/x/FooTest.java": "code",
            "docs/ref/settings.txt": "docs", "framework-docs/modules/ROOT/pages/web.adoc": "docs",
            "CHANGELOG.md": "docs", "README.rst": "docs", "CMakeLists.txt": "code", "requirements.txt": "code",
            "django/conf/global_settings.py": "code",
        }
        for path, kind in cases.items():
            self.assertEqual(editgrade.file_kind(path), kind, path)

    def test_grading_by_kind_and_forgotten_dependents(self):
        g = editgrade.grade_edit(EDIT_TASK, ["src/utils/accept.ts", "src/utils/mime.ts", "src/other.ts"], None)
        self.assertEqual(g["files_by_kind"]["code"], {"precision": 0.6667, "recall": 0.6667, "f1": 0.6667, "tp": 2,
                                                      "n_gold": 3, "n_changed": 3})
        self.assertEqual(g["files_by_kind"]["tests"]["recall"], 0.0)
        self.assertEqual(g["files_by_kind"]["docs"]["recall"], 0.0)
        dep = g["dependents"]
        self.assertEqual(dep["n"], 4)  # every gold file except the primary one
        self.assertEqual(dep["by_kind"]["tests"], {"n": 1, "forgotten": ["src/utils/accept.test.ts"]})
        self.assertEqual(dep["by_kind"]["docs"], {"n": 1, "forgotten": ["docs/accept.md"]})
        self.assertEqual(dep["by_kind"]["remote"], {"n": 1, "forgotten": ["src/helper/accepts/accepts.ts"]})
        self.assertEqual(dep["by_kind"]["local"], {"n": 1, "forgotten": []})  # mime.ts sits next to accept.ts
        self.assertEqual(len(g["dependents_forgotten"]), 3)
        self.assertEqual(g["files_extra"], ["src/other.ts"])
        self.assertAlmostEqual(g["file_recall"], 2 / 5, places=3)
        # With a test command, the test decides; without, code-file F1 against the threshold.
        self.assertFalse(g["success"])
        self.assertTrue(editgrade.grade_edit(EDIT_TASK, ["src/other.ts"], {"passed": True})["success"])
        self.assertFalse(editgrade.grade_edit(EDIT_TASK, list(editgrade.gold_paths(EDIT_TASK)),
                                              {"passed": False})["success"])
        code = ["src/utils/accept.ts", "src/utils/mime.ts", "src/helper/accepts/accepts.ts"]
        self.assertTrue(editgrade.grade_edit(EDIT_TASK, code, None)["success"])
        # Without primary files every gold file would be a dependent (= 1 - recall): not reported.
        no_primary = {k: v for k, v in EDIT_TASK.items() if k != "primary_files"}
        g = editgrade.grade_edit(no_primary, [], None)
        self.assertIsNone(g["dependents"])
        self.assertIsNone(g["dependents_forgotten"])

    def test_precheck_requires_the_test_to_fail_before_the_change(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, scratch = Path(tmp) / "repo", Path(tmp) / "scratch"
            repo.mkdir()
            scratch.mkdir()
            (repo / "a.py").write_text("x = 1\n")
            task = {"id": "t", "test_command": "grep -q 'x = 2' a.py"}
            rec = editgrade.precheck(task, repo, scratch, "abc")
            self.assertTrue(rec["fails_on_parent"])
            self.assertEqual((rec["commit"], rec["command"], rec["returncode"]), ("abc", task["test_command"], 1))
            self.assertFalse(editgrade.precheck(dict(task, test_command="true"), repo, scratch, "abc")["fails_on_parent"])
            self.assertEqual(list(scratch.iterdir()), [])  # the copy is removed

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
        bad = dict(EDIT_TASK, gold_files=[{"path": "x", "status": "Z"}], primary_files=["y"], type="callers",
                   dependents=["x"])
        problems = check_tasks([bad], repos, check_evidence=False)
        self.assertEqual(len(problems), 4, problems)
        # The test command must have been prechecked, on this pin, and fail there.
        for pre, word in ((None, "no precheck"), (dict(EDIT_TASK["precheck"], commit="other"), "no precheck"),
                          (dict(EDIT_TASK["precheck"], fails_on_parent=False), "does not fail")):
            t = dict(EDIT_TASK, precheck=pre)
            self.assertTrue(any(word in p for p in check_tasks([t], repos, check_evidence=False)), word)

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
        self.assertEqual(row["missed"], ["docs/accept.md", "src/helper/accepts/accepts.ts", "src/utils/mime.ts"])
        self.assertAlmostEqual(row["recall"], 2 / 5, places=3)
        self.assertIn("search_audit", row)
        self.assertEqual(row["edit"]["files_by_kind"]["tests"]["recall"], 1.0)


if __name__ == "__main__":
    unittest.main()
