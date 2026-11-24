"""#183: commit-replay edit tasks: per-task pins, gold files kept outside the
working copy, the commit's tests overlaid before the test command, prechecks
in both directions, the repository's build environment, and the Gradle
allowlist of arm D."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "groundtruth"))

from aeval import bashcmd, config, editgrade, prep  # noqa: E402
from aeval.prep import check_tasks  # noqa: E402
from aeval.runner import build_prompt  # noqa: E402

import commit_replay  # noqa: E402

GIT_ID = ["-c", "user.email=e@x", "-c", "user.name=n"]


def _git(*args, cwd):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def _rev(cwd, ref="HEAD"):
    return subprocess.run(["git", "rev-parse", ref], cwd=cwd, check=True, capture_output=True,
                          text=True).stdout.strip()


def _upstream(root: Path) -> tuple[Path, str, str]:
    """A two-commit repository: the second commit changes main code, adds a
    test, deletes a file and adds a doc page."""
    up = root / "up"
    (up / "mod-a/src/main/java").mkdir(parents=True)
    (up / "mod-a/src/main/java/A.java").write_text("class A { int v() { return 1; } }\n")
    (up / "mod-a/src/main/java/Old.java").write_text("class Old {}\n")
    _git("init", "-q", cwd=up)
    _git("add", ".", cwd=up)
    _git(*GIT_ID, "commit", "-qm", "parent", cwd=up)
    (up / "mod-a/src/main/java/A.java").write_text("class A { int v() { return 2; } }\n")
    (up / "mod-a/src/main/java/Old.java").unlink()
    (up / "mod-a/src/test/java").mkdir(parents=True)
    (up / "mod-a/src/test/java/ATests.java").write_text("check 2\n")
    (up / "docs").mkdir()
    (up / "docs/a.adoc").write_text("= A\n")
    _git("add", "-A", ".", cwd=up)
    _git(*GIT_ID, "commit", "-qm", "gold", cwd=up)
    return up, _rev(up, "HEAD^"), _rev(up)


GOLD = [{"path": "docs/a.adoc", "status": "A"}, {"path": "mod-a/src/main/java/A.java", "status": "M"},
        {"path": "mod-a/src/main/java/Old.java", "status": "D"},
        {"path": "mod-a/src/test/java/ATests.java", "status": "A"}]
# Passes when A returns 2 and the test file is there.
TEST = "grep -q 'return 2' mod-a/src/main/java/A.java && test -f mod-a/src/test/java/ATests.java"


class CommitReplay(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.env = mock.patch.dict(os.environ, {"AEVAL_CACHE": str(self.root / "cache")})
        self.env.start()
        self.up, self.parent, self.gold = _upstream(self.root)
        self.repo = {"name": "r183", "url": f"file://{self.up}", "commit": self.parent, "gold_commit": self.gold}
        self.task = {"id": "t", "repo": "r183", "type": "edit", "answer_kind": "diff", "question": "q",
                     "rationale": "r", "gold_files": GOLD, "primary_files": ["mod-a/src/main/java/A.java"],
                     "test_command": TEST, "test_overlay": ["mod-a/src/test/java/ATests.java"]}

    def tearDown(self):
        self.env.stop()
        self.tmp.cleanup()

    def test_fetch_keeps_the_gold_commit_outside_the_checkout(self):
        prep.fetch({"r183": self.repo})
        checkout = config.cache_dir() / "repos" / "r183"
        self.assertEqual(_rev(checkout), self.parent)
        # The checkout holds the parent only: the gold commit is not reachable from it.
        res = subprocess.run(["git", "cat-file", "-e", self.gold], cwd=checkout, capture_output=True)
        self.assertNotEqual(res.returncode, 0)
        gold = config.gold_dir("r183")
        self.assertEqual(editgrade.gold_name_status(gold), {g["path"]: g["status"] for g in GOLD})
        self.assertEqual((gold / "files/mod-a/src/main/java/A.java").read_text(), "class A { int v() { return 2; } }\n")
        self.assertFalse((gold / "files/mod-a/src/main/java/Old.java").exists())
        # A pin that is not the gold commit's parent is refused.
        with self.assertRaises(SystemExit):
            prep.fetch_gold(dict(self.repo, name="r2", commit=self.gold, gold_commit=self.gold))

    def test_precheck_fails_with_the_overlay_and_passes_on_the_gold_commit(self):
        prep.fetch({"r183": self.repo})
        checkout = config.cache_dir() / "repos" / "r183"
        scratch = self.root / "scratch"
        scratch.mkdir()
        rec = editgrade.precheck(self.task, checkout, scratch, self.parent, gold=config.gold_dir("r183"),
                                 gold_commit=self.gold)
        self.assertTrue(rec["fails_on_parent"])
        self.assertTrue(rec["passes_on_gold"])
        self.assertEqual((rec["gold_commit"], rec["test_overlay"]), (self.gold, self.task["test_overlay"]))
        flaky = dict(self.task, test_command=TEST + " && test ! -f ran && touch ran")  # passes once only
        rec2 = editgrade.precheck(flaky, checkout, scratch, self.parent, gold=config.gold_dir("r183"),
                                  gold_commit=self.gold, gold_runs=2, log_dir=self.root / "logs")
        self.assertEqual((rec2["passes_on_gold"], rec2["gold_passes"], rec2["gold_runs"]), (False, 1, 2))
        self.assertEqual(len(list((self.root / "logs").iterdir())), 3)
        self.assertIn("seconds_parent", rec)
        self.assertIn("seconds_gold", rec)
        self.assertEqual(list(scratch.iterdir()), [])
        # A test command that already passes with the overlay alone is caught.
        rec = editgrade.precheck(dict(self.task, test_command="test -f mod-a/src/test/java/ATests.java"), checkout,
                                 scratch, self.parent, gold=config.gold_dir("r183"), gold_commit=self.gold)
        self.assertFalse(rec["fails_on_parent"])

    def test_check_tasks_reads_pins_from_the_task_file_and_wants_both_prechecks(self):
        prep.fetch({"r183": self.repo})
        repos = {"r183": self.repo}
        self.task["precheck"] = {"fails_on_parent": True, "commit": self.parent, "command": TEST,
                                 "passes_on_gold": True, "gold_commit": self.gold,
                                 "test_overlay": self.task["test_overlay"]}
        self.assertEqual(check_tasks([self.task], repos), [])
        for change, word in (({"passes_on_gold": False}, "pass on the gold commit"),
                             ({"test_overlay": None}, "another test_overlay")):
            t = dict(self.task, precheck=dict(self.task["precheck"], **change))
            self.assertTrue(any(word in p for p in check_tasks([t], repos)), word)
        # The gold files must be exactly the commit's changes; the overlay, gold test files.
        t = dict(self.task, gold_files=GOLD[1:])
        self.assertTrue(any("differ from the commit" in p for p in check_tasks([t], repos)))
        t = dict(self.task, test_overlay=["mod-a/src/main/java/A.java", "x"])
        problems = check_tasks([t], repos, check_evidence=False)
        self.assertTrue(any("not a test file" in p for p in problems))
        self.assertTrue(any("not a gold file" in p for p in problems))

    def test_load_repos_merges_the_task_files_pins(self):
        path = self.root / "tasks.json"
        path.write_text(json.dumps({"repos": [self.repo], "tasks": []}))
        with mock.patch.dict(os.environ, {"AEVAL_TASKS": str(path)}):
            repos = config.load_repos()
            self.assertIn("r183", repos)
            self.assertIn("hono", repos)
            path.write_text(json.dumps({"repos": [dict(self.repo, name="hono")], "tasks": []}))
            with self.assertRaises(SystemExit):
                config.load_repos()

    def test_repo_env(self):
        repo = {"name": "j", "session_env": {"GRADLE_OPTS": "-Dx=1"}, "env_from_caller": ["JAVA_HOME"]}
        with mock.patch.dict(os.environ, {"JAVA_HOME": "/jdk"}):
            env = config.repo_env(repo)
        self.assertEqual(env["GRADLE_OPTS"], "-Dx=1")
        self.assertEqual(env["PATH"], f"/jdk/bin:{config.SYSTEM_PATH}")
        with mock.patch.dict(os.environ, {"JAVA_HOME": ""}):
            with self.assertRaises(SystemExit):
                config.repo_env(repo)
        # The session keeps its myc wrapper first on PATH.
        s = config.session_env({"PATH": env["PATH"]}, legacy=False, bin_dir=Path("/b"))
        self.assertEqual(s["PATH"], f"/b:/jdk/bin:{config.SYSTEM_PATH}")
        self.assertEqual(config.session_env(None, bin_dir=Path("/b"))["PATH"], f"/b:{config.SYSTEM_PATH}")

    def test_baseline_leaves_out_what_the_harness_wrote(self):
        prep.fetch({"r183": self.repo})
        w = self.root / "w"
        config.copy_tree(config.cache_dir() / "repos" / "r183", w)
        (w / "CLAUDE.md").write_text("routing\n")  # setup-claude, before the agent
        (w / ".mcp.json").write_text("{}\n")
        base = editgrade.snapshot(w)
        (w / ".mcp.json").write_text('{"x": 1}\n')  # the agent edits one of them
        (w / "mod-a/src/main/java/A.java").write_text("class A { int v() { return 2; } }\n")
        self.assertEqual(editgrade.changed_files(w, base), [".mcp.json", "mod-a/src/main/java/A.java"])
        # Overlay and deletions from the gold commit.
        done = editgrade.apply_gold(w, config.gold_dir("r183"), self.task,
                                    ["mod-a/src/test/java/ATests.java", "mod-a/src/main/java/Old.java"])
        self.assertEqual(len(done), 2)
        self.assertTrue((w / "mod-a/src/test/java/ATests.java").exists())
        self.assertFalse((w / "mod-a/src/main/java/Old.java").exists())

    def test_dependents_in_other_modules(self):
        task = {"gold_files": ["spring-web/src/main/java/a/A.java", "spring-web/src/main/java/b/B.java",
                               "spring-webmvc/src/main/java/c/C.java", "spring-web/src/test/java/a/ATests.java",
                               "framework-docs/modules/ROOT/pages/web.adoc"],
                "primary_files": ["spring-web/src/main/java/a/A.java"]}
        dep = editgrade.forgotten_dependents(task, {"spring-web/src/main/java/b/B.java"})
        self.assertEqual(dep["by_kind"]["remote"]["n"], 2)  # another package counts as remote
        self.assertEqual(dep["other_module"], {"n": 1, "forgotten": ["spring-webmvc/src/main/java/c/C.java"]})
        self.assertEqual(editgrade.module_of("spring-web/src/main/java/a/A.java"), "spring-web")
        self.assertIsNone(editgrade.module_of("framework-docs/modules/ROOT/pages/web.adoc"))


class GradleCommands(unittest.TestCase):
    def test_failure_kinds_and_rerun(self):
        def kind(out, passed=False, timed_out=False):
            return editgrade.failure_kind({"passed": passed, "timed_out": timed_out, "output": out})
        self.assertIsNone(kind("", passed=True))
        self.assertEqual(kind("", timed_out=True), "timeout")
        self.assertEqual(kind("> Task :spring-core:compileTestJava FAILED\nExecution failed for task "
                              "':spring-core:compileTestJava'."), "compile")
        self.assertEqual(kind("12 tests completed, 1 failed\nThere were failing tests."), "tests")
        self.assertEqual(kind("Execution failed for task ':spring-core:compileJava'.\n> Could not resolve all files"),
                         "infrastructure")  # a download error is not evidence about the change
        self.assertEqual(kind("Downloading from https://x/gradle.zip failed: timeout"), "infrastructure")
        self.assertEqual(kind("> Could not resolve all files for configuration\n > No cached version of a:b:1 available for offline mode."), "infrastructure")
        # Gradle also prints that line as a notice of a build that goes on.
        self.assertEqual(kind("No cached version of a:b:1 available for offline mode.\n3 tests completed, 1 failed"), "tests")
        self.assertEqual(kind("No tests found for given includes: [x]"), "no_tests")
        self.assertEqual(kind("boom"), "other")
        self.assertEqual(editgrade.rerun_command("./gradlew -q :a-b:test --tests x.YTests :c:test"),
                         "./gradlew -q :a-b:test --rerun --tests x.YTests :c:test --rerun")

    def test_default_test_command(self):
        files = [("M", "spring-web/src/main/java/org/x/A.java"),
                 ("M", "spring-web/src/test/java/org/x/ATests.java"),
                 ("A", "spring-web/src/test/kotlin/org/x/BTests.kt"),
                 ("A", "spring-context/src/testFixtures/java/org/y/Fixture.java"),
                 ("A", "spring-context/src/test/java/org/y/CTest.java"),
                 ("D", "spring-core/src/test/java/org/z/GoneTests.java"),
                 ("M", "framework-docs/modules/ROOT/pages/a.adoc")]
        self.assertEqual(commit_replay.default_test_command(files),
                         "./gradlew --offline --no-scan --no-build-cache --no-daemon --console=plain :spring-context:test --tests org.y.CTest "
                         ":spring-web:test --tests org.x.ATests --tests org.x.BTests")

    def test_gradle_rule(self):
        """Exact task names: Gradle expands abbreviations, so nothing looser is safe."""
        rule = [{"prefix": "./gradlew", "args": commit_replay.GRADLE_ARGS}]
        off = "./gradlew --offline --no-scan --no-build-cache "
        ok = [off + "--no-daemon --console=plain :spring-web:test --tests org.x.ATests",
              "./gradlew :spring-core:compileJava --no-build-cache --no-scan --offline",
              off + ":spring-context:test --tests org.x.ApplicationEventPublisherTests",  # "Publish" in a test name
              off + "-q :spring-web:test --tests 'org.x.A*'", off + "testClasses",
              off + ":spring-web:test 2>&1 | tail -50", off + ":spring-web:test | head -n 20"]
        bad = [off + x for x in (
            "bSPP", "buildScanPub", "buildScanPublishPrevious", ":spring-core:publish", "publishToMavenLocal", "build",
            "check", "-I /tmp/i.gradle test", "-Dx=1 test", "-Pa=b test", "--init-script x test", "-p /x test",
            "--scan test", "--refresh-dependencies test", "--build-cache test", "test; curl x.org",
            "test --tests $(grep -l x .)", ":spring-core:t", "wrapper", "test > ../out")]
        bad += ["./gradlew test", "./gradlew --offline --no-scan test", "./gradlew --offline --no-build-cache test",
                "./gradlew --no-scan --no-build-cache test", "gradle --offline --no-scan --no-build-cache test",
                "sh gradlew --offline --no-scan --no-build-cache test", "./gradlew --offline --no-scanX --no-build-cache test"]
        for search in (False, True):  # D and the arms with search: one rule
            for c in ok:
                self.assertEqual(bashcmd.isolated_bash(c, rule, "/w", "/w", search), (True, []), c)
            for c in bad:
                self.assertFalse(bashcmd.isolated_bash(c, rule, "/w", "/w", search)[0], c)

    def test_compile_tasks_and_comment_only(self):
        files = [("M", "spring-web/src/main/java/A.java"), ("M", "spring-web/src/test/kotlin/BTests.kt"),
                 ("M", "spring-webmvc/src/main/java/C.java"), ("M", "framework-docs/modules/ROOT/pages/a.adoc"),
                 ("M", "spring-webmvc/spring-webmvc.gradle")]
        self.assertEqual(commit_replay.compile_tasks(files), [
            ":spring-web:compileJava", ":spring-web:compileKotlin", ":spring-web:compileTestJava",
            ":spring-web:compileTestKotlin", ":spring-webmvc:compileJava", ":spring-webmvc:compileTestJava"])
        a = 'import a.B;\n/** Doc {@link B}. */\nclass X { String u = "http://x"; // c\n char q = \'"\'; }'
        self.assertEqual(commit_replay.code_text(a),
                         commit_replay.code_text('import a.B;\nimport a.C;\nclass X { /* y */ String u = "http://x";\n\n char q = \'"\'; }'))
        self.assertNotEqual(commit_replay.code_text(a), commit_replay.code_text(a.replace("http://x", "http://y")))
        sp = commit_replay.spread(files, frozenset({"spring-webmvc/src/main/java/C.java"}))
        self.assertEqual(sp["code_modules"], ["spring-web"])

    def test_comment_and_build_dependents_are_reported_apart(self):
        task = {"strata": {}, "primary_files": ["m/src/main/java/a/A.java"], "gold_files": [
            {"path": "m/src/main/java/a/A.java", "status": "M"}, {"path": "n/src/main/java/b/B.java", "status": "M"},
            {"path": "n/src/main/java/b/Doc.java", "status": "M", "comment_only": True},
            {"path": "n/n.gradle", "status": "M"}]}
        g = editgrade.grade_edit(task, ["m/src/main/java/a/A.java", "n/src/main/java/b/B.java"], {"passed": True})
        self.assertEqual(g["files_by_kind"]["code_changed"]["recall"], 1.0)
        self.assertEqual(g["files_by_kind"]["code"]["recall"], 0.5)
        self.assertEqual(g["dependents"]["by_kind"]["comments"], {"n": 1, "forgotten": ["n/src/main/java/b/Doc.java"]})
        self.assertEqual(g["dependents"]["by_kind"]["build"], {"n": 1, "forgotten": ["n/n.gradle"]})
        self.assertEqual(g["dependents"]["other_module"], {"n": 1, "forgotten": []})


class Isolation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(os.path.realpath(self.tmp.name)) / "work" / "spring-framework"
        (self.root / "spring-core/src").mkdir(parents=True)
        (self.root / "a.java").write_text("x")
        self.secret = self.root.parent.parent / "gold"
        self.secret.mkdir()
        (self.secret / "f").write_text("secret")
        (self.root / "link").symlink_to(self.secret)

    def tearDown(self):
        self.tmp.cleanup()

    def why(self, tool, search=True, here=None, readable=(), **inp):
        return bashcmd.isolated_tool(tool, inp, str(self.root), here or str(self.root), [], search, readable)

    def test_nothing_leaves_the_working_copy_in_any_arm(self):
        s = str(self.secret)
        for search in (False, True):
            for cmd in (f"cat {s}/f", "cat ../../gold/f", "cat link/f", "cat ~/.cache/x", "cat $HOME/x", f"grep -r x {s}",
                        "grep -rn x ..", f"grep --file={s}/f x", f"rg x {s}", f"find {s} -name f", "find . -exec cat {} +",
                        "ls /", f"git -C {s} log", f"git show --no-index {s}/f", f"git diff --no-index a.java {s}/f",
                        f"cat < {s}/f", f"cp {s}/f b", f"head {s}/f", f"myc analyze {s}", f"myc callers x --repo={s}",
                        f"for f in {s}/*; do cat $f; done", "cd .. && ls", "sed -n '1r /etc/passwd' a.java",
                        "python3 -c 'print(open(1))'", "bash -c 'cat /etc/passwd'", "awk 'BEGIN{system(1)}'", "env",
                        "./evil.sh", "/bin/cat a.java", "xargs cat", "eval ls", "tee x", "perl -e 1", "node x",
                        "sort -o ../x a.java", "myc serve", "myc diff", "myc email sync", "myc setup-claude", "git diff --ext-diff", "xargs cat", "X=1", "rg --pre cat x", "git grep -Ocat x", "cat .git/config", "echo $(cat a)"):
                self.assertIsNotNone(self.why("Bash", search, command=cmd), (search, cmd))
            for cmd in ("curl https://x.org", "wget x", "git fetch origin", "git pull", "git clone x", "git remote -v",
                        "git ls-remote https://x", "nc x 1", "ssh x", "gh api x", "mvn x", "java -version"):
                self.assertEqual(self.why("Bash", search, command=cmd)[0], "network", cmd)
            for tool, inp in (("Read", {"file_path": s + "/f"}), ("Read", {"file_path": "../../gold/f"}),
                              ("Read", {"file_path": str(self.root / "link/f")}), ("Read", {"file_path": ".git/HEAD"}),
                              ("Write", {"file_path": s + "/g"}), ("Edit", {"file_path": "/etc/hosts"}),
                              ("Write", {"file_path": "gradle/wrapper/gradle-wrapper.properties"}),
                              ("Edit", {"file_path": "gradlew"}), ("Grep", {"pattern": "x", "path": s}),
                              ("Grep", {"pattern": "x", "glob": "../**"}), ("Glob", {"pattern": s + "/*"}),
                              ("Glob", {"pattern": "**/*.java", "path": ".."}), ("WebFetch", {"url": "https://x"}),
                              ("WebSearch", {"query": "x"}), ("Task", {"prompt": "x"}), ("NotARealTool", {}),
                              ("mcp__myceliums__analyze", {"path": s}), ("mcp__myceliums__list_repos", {"x": ["~/y"]}),
                              ("mcp__other__x", {})):
                self.assertIsNotNone(self.why(tool, search, **inp), (tool, inp))
            for tool, inp in (("Read", {"file_path": str(self.root / "a.java")}), ("Read", {"file_path": "a.java"}),
                              ("Edit", {"file_path": str(self.root / "a.java")}), ("Write", {"file_path": "spring-core/B.java"}),
                              ("mcp__myceliums__find_callers", {"symbol_name": "MimeType.valueOf"}),
                              ("mcp__myceliums__analyze", {"path": str(self.root)}), ("ToolSearch", {"query": "x"})):
                self.assertIsNone(self.why(tool, search, **inp), (tool, inp))
            for cmd in ("git status", "git diff", "git log --oneline -3", "myc callers MimeType.valueOf | head -20",
                        "git diff | tail -n 5", "mkdir x", "cd spring-core", "rm a.java", "pwd"):
                self.assertIsNone(self.why("Bash", search, command=cmd), (search, cmd))
        # The session's own saved results, and nothing beside them.
        own = str(self.root.parent.parent / "p" / "sid" / "tool-results")
        self.assertIsNone(self.why("Read", readable=[own], file_path=own + "/a.txt"))
        self.assertIsNotNone(self.why("Read", readable=[own], file_path=own + "/../../other/a.txt"))

    def test_arms_differ_only_in_search(self):
        only_search = ["grep -rn 'String foo(' spring-core", "grep -rn \"a/b\" . | head -5", "grep -n '/users' a.java", "rg -n x spring-core",
                       "find . -name '*.java'", "ls spring-core", "cat a.java", "head -20 a.java", "sed -n '1,9p' a.java",
                       "wc -l a.java", "git grep -n foo", "cat a.java | sort | uniq", "git log --oneline | grep x"]
        for c in only_search:
            self.assertIsNone(self.why("Bash", True, command=c), c)
            self.assertIsNotNone(self.why("Bash", False, command=c), c)
        # The settings the runner writes differ in that one argument and in the myceliums paths; tools in Grep and Glob.
        from aeval import isolation
        from aeval.runner import Runner, RunConfig, WorkItem
        task = {"id": "t", "repo": "r", "answer_kind": "diff", "allowed_commands": [{"prefix": "./gradlew", "args": "x"}]}
        top = self.root.parent.parent
        r = Runner(RunConfig(label="x", conditions=("none", "C", "D"), bash=True, claude="claude", myc=str(top / "bin/myc"),
                             models_dir=str(top / "models")), [task],
                   {"r": {"name": "r", "commit": "c", "isolated": True, "session_env": {"JAVA_HOME": str(top / "jdk")}}})
        hooks, tools, sets = {}, {}, {}
        for cond in ("none", "C", "D"):
            t = top / cond
            t.mkdir()
            it = WorkItem(task, cond, 1)
            with mock.patch.object(isolation, "available", lambda: True):
                st = r._isolated_settings(it, t, self.root)
            data = json.loads(st.read_text().replace(str(t), "T"))
            hooks[cond] = data["hooks"].pop("PreToolUse")[0]["hooks"][0]["command"]
            sets[cond] = data
            cmd = r._command(it, Path("/x/mcp.json"), st)
            tools[cond] = set(cmd[cmd.index("--tools") + 1].split(","))
            self.assertIn("WebFetch", cmd[cmd.index("--disallowedTools") + 1])
            self.assertEqual(cmd[0], "claude")  # the CLI itself runs as in daily use
        self.assertEqual(sets["C"], sets["D"])
        self.assertEqual(hooks["none"], hooks["C"])
        self.assertEqual(hooks["none"].replace("--search yes", "--search no"), hooks["D"])
        myc_paths = {"T/bin", "T/data", "T/home", str(top / "bin/myc"), str(top / "models")}
        fs = lambda c, k: set(sets[c]["sandbox"]["filesystem"][k])  # noqa: E731
        self.assertEqual(fs("C", "allowRead") - fs("none", "allowRead"), myc_paths)
        self.assertEqual(fs("C", "allowWrite") - fs("none", "allowWrite"), {"T/data", "T/home"})
        self.assertEqual(fs("none", "allowRead") - fs("C", "allowRead"), set())
        for c in sets:
            for k in ("allowRead", "allowWrite"):
                sets[c]["sandbox"]["filesystem"].pop(k)
        self.assertEqual(sets["none"], sets["C"])
        self.assertEqual(tools["none"], tools["C"])
        self.assertEqual(tools["none"] - tools["D"], {"Grep", "Glob"})
        self.assertEqual(tools["D"] - tools["none"], set())
        # The hook as a program: refuses, allows, and refuses when it cannot decide.
        script = Path(bashcmd.__file__)
        def run(event):
            return subprocess.run([sys.executable, "-I", str(script), "isolated-hook", "--root", str(self.root),
                                   "--search", "no", "--allow-json", "[]"], input=event, capture_output=True, text=True)
        ok = run(json.dumps({"tool_name": "Read", "tool_input": {"file_path": "a.java"}, "cwd": str(self.root)}))
        self.assertEqual((ok.returncode, ok.stdout), (0, ""))
        for event in (json.dumps({"tool_name": "Read", "tool_input": {"file_path": str(self.secret / "f")},
                                  "cwd": str(self.root)}), "not json"):
            out = json.loads(run(event).stdout)["hookSpecificOutput"]
            self.assertEqual(out["permissionDecision"], "deny")
            self.assertIn(bashcmd.ISOLATED_MARKER, out["permissionDecisionReason"])
            for hidden in (str(config.EVAL_DIR), str(config.cache_dir()), "--allow-json", "isolated-hook"):
                self.assertNotIn(hidden, out["permissionDecisionReason"])

    def test_session_settings_close_everything_else(self):
        from aeval import isolation
        top = self.root.parent.parent
        env = {"JAVA_HOME": str(top / "jdk")}
        st = isolation.session_settings(top, self.root, env, [], "hook")
        sb = st["sandbox"]
        # Fails closed, no way out, nothing a repository's files could have opened.
        self.assertEqual((sb["enabled"], sb["failIfUnavailable"], sb["allowUnsandboxedCommands"], sb["excludedCommands"]),
                         (True, True, False, []))
        self.assertFalse(sb["enableWeakerNestedSandbox"] or sb["enableWeakerNetworkIsolation"] or sb["allowAppleEvents"])
        net = sb["network"]
        self.assertEqual((net["allowedDomains"], net["deniedDomains"], net["strictAllowlist"]), ([], ["*"], True))
        self.assertEqual((net["allowUnixSockets"], net["allowAllUnixSockets"], net["allowMachLookup"]), ([], False, []))
        fs = sb["filesystem"]
        home = os.path.realpath(Path.home())
        for closed in (home, "/private/tmp", "/private/var/folders", "/Users/Shared", "/Volumes", f"{home}/Library/Keychains",
                       f"{home}/.claude/projects", f"{home}/.claude/history.jsonl", f"{home}/.claude/.credentials.json",
                       f"/private/tmp/claude-{os.getuid()}", f"{home}/.gradle/caches/build-cache-1",
                       f"{home}/.gradle/daemon/**/*.out.log"):
            self.assertIn(closed, fs["denyRead"])
        self.assertFalse([p for p in fs["denyRead"] if "gold" in p or "spring-history" in p or "myceliums" in p],
                         "the model is shown this list")
        for p in (".claude", ".mcp.json", "gradle/wrapper", "gradlew", ".git/hooks", ".git/config"):
            self.assertIn(os.path.realpath(self.root) + "/" + p, fs["denyWrite"])
        for p in ("init.d", "gradle.properties"):
            self.assertIn(f"{home}/.gradle/{p}", fs["denyWrite"])
        self.assertNotIn(f"{home}/.gradle/wrapper", fs["denyWrite"])  # the wrapper's lock file must stay writable
        g = top / "gh"
        (g / "build-scan-data/spool").mkdir(parents=True)
        (g / "build-scan-data/spool/old.scan").write_text("x")
        self.assertEqual(isolation.earlier_scans({"GRADLE_USER_HOME": str(g)}), [str(g / "build-scan-data/spool/old.scan")])
        self.assertIn(str(g / "build-scan-data/spool/old.scan"), isolation.never({"GRADLE_USER_HOME": str(g)}))
        dist = g / "wrapper/dists/gradle-9/abc/gradle-9.9.0"
        (dist / "bin").mkdir(parents=True)
        for f in ("x.zip", "x.zip.ok", "x.zip.lck"):
            (dist.parent / f).write_text("")
        denied_w = isolation.gradle_unwritable({"GRADLE_USER_HOME": str(g)})
        self.assertIn(os.path.realpath(dist), [os.path.realpath(x) for x in denied_w])
        self.assertIn("x.zip.ok", [Path(x).name for x in denied_w])
        self.assertNotIn("x.zip.lck", [Path(x).name for x in denied_w])
        for wide in (home, "/", "/private/tmp", f"{home}/.cache", f"{home}/.claude", f"{home}/Library"):
            self.assertNotIn(wide, fs["allowRead"] + fs["allowWrite"])
        self.assertTrue(st["permissions"]["blockReadsOutsideWorkingDirectories"] and st["permissions"]["blockWritesOutsideWorkingDirectories"])
        self.assertFalse([d for d in st["permissions"]["deny"] if "/.gradle" in d or "/.cache" in d])  # would close them to builds
        self.assertIn(f"Edit(/{os.path.realpath(self.root)}/.claude/**)", st["permissions"]["deny"])
        # No credential reaches a command's environment, and nothing touches the login.
        denied = {e["name"] for e in sb["credentials"]["envVars"] if e["mode"] == "deny"}
        self.assertLessEqual({"ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"}, denied)
        senv = isolation.session_environment(top)
        self.assertFalse([k for k in senv if "CONFIG_DIR" in k or "SECURESTORAGE" in k or "KEY" in k or "TOKEN" in k])
        for k in ("JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS"):  # every JVM of a build, worker JVMs included
            self.assertIn(f"-Djava.io.tmpdir={top}/t", senv[k])
            self.assertIn("-Djava.net.preferIPv4Stack=true", senv[k])  # loopback, with the sandbox's rule
        self.assertEqual(isolation.MARKER, bashcmd.ISOLATED_MARKER)
        for repo in ({"isolated": True}, {}):
            with mock.patch.dict(os.environ, {"AEVAL_WORK_ROOT": str(top / "wr/x"), "AEVAL_CACHE": str(top / "cache")}):
                self.assertEqual(config.session_root(repo), top / "wr/ag" if repo else top / "cache/tmp")

    def test_the_agent_cannot_write_settings(self):
        for search in (True, False):
            for tool, f in (("Write", ".claude/settings.json"), ("Edit", ".claude/settings.local.json"), ("Write", ".mcp.json"),
                            ("Write", "spring-core/.claude/x"), ("Write", ".CLAUDE/settings.json"), ("Write", ".gitattributes")):
                self.assertIsNotNone(self.why(tool, search, file_path=str(self.root / f)), f)
            for c in ("mkdir .claude", "cp a.java .mcp.json", "git diff > .claude/settings.json", "mv a.java spring-core/.claude/x", "touch .zshenv"):
                self.assertIsNotNone(self.why("Bash", search, command=c), c)

    @unittest.skipUnless(sys.platform == "darwin", "sandbox-exec is macOS")
    def test_the_grading_profile(self):
        """The test command runs the agent's code: an allowlist of files, no network, no keychain services."""
        from aeval import isolation
        top = self.root.parent.parent  # stands in for the home directory
        for d in ("tmp", "jdk", "gradle/caches/build-cache-1", "gradle/caches/modules-2", "gradle/daemon/9", "gradle/build-scan-data",
                  "projects", "other"):
            (top / d).mkdir(parents=True)
            (top / d / "f.txt").write_text("secret")
        (top / "gradle/daemon/9/d.out.log").write_text("secret")
        (top / "gradle/gradle.properties").write_text("a=b")
        (top / "history.jsonl").write_text("secret")
        (top / "tmp/ok").write_text("fine")
        prof = top / "p.sb"
        prof.write_text(isolation.profile([self.root, top / "tmp", top / "gradle"], [top / "jdk"], [top],
                                          [str(top / "gradle/caches/build-cache-1"), str(top / "gradle/build-scan-data"),
                                           str(top / "gradle/daemon/**/*.out.log")], [str(top / "gradle/gradle.properties")]))
        def run(*cmd):
            return subprocess.run(isolation.sandboxed(list(cmd), prof), capture_output=True, text=True, cwd=self.root)
        for f, text in ((self.root / "a.java", "x"), (top / "tmp/ok", "fine"), (top / "jdk/f.txt", "secret"),
                        (top / "gradle/caches/modules-2/f.txt", "secret"), (top / "gradle/daemon/9/f.txt", "secret")):
            self.assertEqual(run("/bin/cat", str(f)).stdout, text, f)
        for c in ("gold/f", "projects/f.txt", "other/f.txt", "history.jsonl", "gradle/caches/build-cache-1/f.txt",
                  "gradle/build-scan-data/f.txt", "gradle/daemon/9/d.out.log"):
            f = str(top / c)
            for cmd in (("/bin/cat", f), ("/bin/sh", "-c", f"cd {self.root}/spring-core; cat ../../../{c}"),
                        ("/bin/bash", "-c", "cat $'" + "".join(f"\\x{ord(ch):02x}" for ch in f) + "'"),
                        ("/bin/sh", "-c", f"cp {f} {self.root}/stolen; cat {self.root}/stolen"),
                        ("/usr/bin/grep", "-r", "secret", f), (sys.executable, "-I", "-c", f"print(open({f!r}).read())")):
                self.assertNotIn("secret", run(*cmd).stdout.replace(f, ""), cmd)
        self.assertNotIn("secret", run("/bin/cat", str(self.root / "link/f")).stdout)
        run("/bin/ln", str(self.secret / "f"), str(self.root / "hard"))
        self.assertNotIn("secret", run("/bin/cat", str(self.root / "hard")).stdout)
        self.assertNotEqual(run("/bin/sh", "-c", f"echo y > {top}/other/new").returncode, 0)
        self.assertNotEqual(run("/bin/sh", "-c", f"echo y > {top}/gradle/gradle.properties").returncode, 0)
        self.assertEqual(run("/bin/sh", "-c", f"echo y > {top}/gradle/new && echo y > {top}/gradle/daemon/9/e.out.log").returncode, 0)
        self.assertFalse(editgrade.run_test(f"cat {self.secret}/f", self.root, prefix=isolation.sandboxed([], prof))["passed"])
        # Keychain services cannot be looked up (a lookup asks nothing of them); loopback works.
        code = ("import ctypes,socket;L=ctypes.CDLL(None);p=ctypes.c_uint.in_dll(L,'bootstrap_port');r=ctypes.c_uint(0);"
                "print([L.bootstrap_look_up(p,n.encode(),ctypes.byref(r)) for n in %r]);s=socket.socket();s.bind(('127.0.0.1',0));"
                "s.listen();socket.create_connection(s.getsockname(),3);print('loop')" % (isolation.SECURITY_SERVICES,))
        out = run(sys.executable, "-I", "-c", code).stdout
        self.assertIn("loop", out)
        self.assertTrue(out.startswith("[") and all(int(x) for x in out[1:out.index("]")].split(",")), out)
        # The real one: whole regions, no keychain, no myceliums.
        with mock.patch.dict(os.environ, {"AEVAL_CACHE": str(top / "cache")}):
            text = isolation.grading_profile(top, self.root, {"JAVA_HOME": str(top / "jdk")})
        for needed in ("Library/Keychains", "(deny network-outbound)", 'remote ip "localhost:*"', "com.apple.SecurityServer", "build-cache-1"):
            self.assertIn(needed, text)
        self.assertNotIn("models", text.split("(deny network-outbound)")[0].split("(allow file-read-data file-read-xattr (")[-1])

    def test_c_imports_the_projects_agents_md(self):
        from aeval.runner import Runner, RunConfig, WorkItem
        r = Runner(RunConfig(label="x", conditions=("C",), claude="claude"), [], {})
        for agents in (True, False):
            w, out, b = (self.root.parent / f"{n}{agents}" for n in ("w", "o", "b"))
            for d in (w, out, b):
                d.mkdir()
            if agents:
                (w / "AGENTS.md").write_text("Spring's own rules\n")
            (b / "myc").write_text(f"#!/bin/sh\nmkdir -p {w}/.claude; echo routing > {w}/CLAUDE.md; echo '{{}}' > {w}/.claude/settings.json\n")
            (b / "myc").chmod(0o755)
            for _ in range(2):  # idempotent
                r._setup_claude(WorkItem({"id": "t", "repo": "r"}, "C", 1), w, b, {}, out)
            self.assertEqual((w / "CLAUDE.md").read_text(), "@AGENTS.md\n\nrouting\n" if agents else "routing\n")

    def test_one_claude_version_per_run(self):
        from aeval.runner import Runner, RunConfig, WorkItem
        base = self.root.parent
        with mock.patch.object(config, "RUNS_DIR", base / "runs"):
            (base / "runs/x").mkdir(parents=True)
            for v in ("2.1.330", "2.1.331"):
                (base / v).write_text(f"#!/bin/sh\necho '{v} (Claude Code)'\n")
                (base / v).chmod(0o755)
            link = base / "claude"
            link.symlink_to(base / "2.1.330")
            task = {"id": "t", "repo": "r", "type": "callers", "answer_kind": "symbols", "gold": []}
            r = Runner(RunConfig(label="x", conditions=("A",), claude=str(link)), [task], {"r": {"name": "r", "commit": "c"}})
            r._pin_claude()
            self.assertEqual((r.claude_version, r.cfg.claude), ("2.1.330", str(base / "2.1.330")))
            link.unlink()
            link.symlink_to(base / "2.1.331")  # the updater moves the link: this run keeps its binary
            self.assertEqual(config.tool_version([r.cfg.claude, "--version"]), "2.1.330 (Claude Code)")
            r2 = Runner(RunConfig(label="x", conditions=("A",), claude=str(link)), [], {})
            with self.assertRaises(SystemExit):  # a resume with another version
                r2._pin_claude()
            init = {"type": "system", "subtype": "init", "tools": ["Read", "Grep", "Glob", "mcp__myceliums__x"],
                    "mcp_servers": [{"name": "myceliums", "status": "connected"}], "slash_commands": [], "skills": [],
                    "claude_code_version": "2.1.331"}
            t = base / "t.jsonl"
            t.write_text(json.dumps(init) + "\n")
            row = r._summarise(WorkItem(task, "A", 1), 1, t, 1.0, False, 0)
            self.assertEqual(row["status"], "isolation_error")
            self.assertIn("pinned to 2.1.330", row["isolation_problems"][-1])
            self.assertIn("api_calls", row)
        self.assertEqual(config.ISOLATION_ENV["DISABLE_AUTOUPDATER"], "1")

    def test_audit_counts_per_arm(self):
        from aeval import isolation
        calls = [("1", "Read", {"file_path": str(self.secret / "f")}, bashcmd.ISOLATED_MARKER + " x"),
                 ("2", "Bash", {"command": "git log '#x' ; curl https://x.org"}, "<html>"),
                 ("3", "Bash", {"command": "./gradlew test"}, "BUILD"),
                 ("4", "Bash", {"command": "cat /etc/hosts"}, "cat: /etc/hosts: Operation not permitted"),
                 ("5", "Bash", {"command": "grep -rn 'GET /nosuchtop/{id}' spring-core | head -5"}, "none"),
                 ("6", "Read", {"file_path": str(self.root / "a.java")}, f"see {Path.home()}/SYNC/git/x"),
                 ("7", "Bash", {"command": "./gradlew --offline --no-scan --no-build-cache test 2>&1 | tail -5"}, "ok"),
                 ("8", "Bash", {"command": "cat spring-core/../../gold/f"}, bashcmd.ISOLATED_MARKER),
                 ("9", "Read", {"file_path": f"{Path.home()}/.claude/projects/p/s/tool-results/a.txt"}, "saved"),
                 ("10", "Bash", {"command": "grep -rn 'python3 ' spring-core"}, "none"),
                 ("11", "Glob", {"pattern": "{/etc,a}/*.conf"}, bashcmd.ISOLATED_MARKER),
                 ("12", "Glob", {"pattern": "**/*.{java,kt}"}, "a.java")]
        path = self.root.parent / "t.jsonl"
        with open(path, "w") as fh:
            for i, name, inp, res in calls:
                fh.write(json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": i, "name": name, "input": inp}]}}) + "\n")
                fh.write(json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": i, "content": res}]}}) + "\n")
        a = isolation.audit(str(path), str(self.root))
        self.assertEqual((a["outside_attempts"], a["outside_blocked"], a["outside_ran"]), (4, 3, 1))
        self.assertEqual((a["network_attempts"], a["network_ran"]), (2, 2))  # with a search pattern, for review
        self.assertEqual((a["gradle_attempts"], a["gradle_ran"]), (1, 1))
        self.assertEqual((a["disagreements"], len(a["to_review"])), (4, 4))
        self.assertTrue(any("git log '#x' ; curl" in x for x in a["to_review"]))
        self.assertEqual((a["kernel_refusals"], a["protected_in_results"], a["refused_calls"], a["tool_calls"], a["flagged"]),
                         (1, 1, 3, 12, True))
        s = isolation.summarise([{"condition": "none", "isolation_audit": a}, {"condition": "D", "isolation_audit": a}, {"condition": "D"}])
        self.assertEqual((s["none"]["sessions"], s["D"]["outside_ran"], s["D"]["flagged"]), (1, 1, 1))
        self.assertIn("| `D` | 1 | 3 | 3 / 1 | 0 / 2 | 0 / 1 | 4 | 1 | 1 | 0 | 1 |", "\n".join(isolation.report_lines(s)))
        self.assertEqual(isolation.report_lines({}), [])
        import ast  # the audit must not share the hook's parser
        names = {n.module for n in ast.walk(ast.parse(Path(isolation.__file__).read_text())) if isinstance(n, ast.ImportFrom)}
        self.assertNotIn("bashcmd", names)

    def test_parser_accounts_for_everything_or_refuses(self):
        rule = [{"prefix": "./gradlew", "args": commit_replay.GRADLE_ARGS}]
        w = str(self.root)
        for search in (True, False):
            for c in ("git log '#x' ; python3 x.py", 'git log "#" && curl x', "myc search '#x'; python3 x", "cat {../../gold/f,a}",
                      f"cat {{a,{self.secret}/f}}", f"grep -e{self.secret} a.java", f"grep -f{self.secret}/f a.java",
                      "cat $'\\x2fetc'", "cd missing/dirs; cat ../../x", "false && cd spring-core; cat ../a", "cat a\\\nb",
                      "awk 'BEGIN{system(\"curl x\")}'", "awk '{print > \"/tmp/x\"}' a.java", "awk -f x a.java",
                      "sed -n 'r /etc/passwd' a.java", "sed -n '1e date' a.java", "sed -i s/a/b/ a.java", "sed -n 's/a/b/w x' a.java",
                      "echo a # b\npython3 x", "cat 'a", "!!", "cat a.java\u2028python3 x", "cat ${X}", 'cat "$X"', "cat spring-core/.."):
                self.assertFalse(bashcmd.isolated_bash(c, rule, w, w, search)[0], (search, c))
        for c in ("grep -rn '#toString' .", 'grep -rn "Foo#bar" spring-core | head -5', "git log --oneline # recent", "grep -n 'x$' a.java",
                  "grep -rn 'a{2}' .", "sed -n '/class X/,/^}/p' a.java", "awk 'NR>=1 && NR<=5' a.java", "awk -F: '{print $1}' a.java"):
            self.assertTrue(bashcmd.isolated_bash(c, rule, w, w, True)[0], c)
        self.assertTrue(bashcmd.isolated_bash("myc search '#method'", rule, w, w, False)[0])
        self.assertFalse(bashcmd.isolated_bash("myc search '#x' | grep y", rule, w, w, False)[0])
        for tool, inp in (("Glob", {"pattern": "**/{/etc,a}/x"}), ("Grep", {"pattern": "x", "glob": "{../**,a}"}), ("Glob", {"pattern": "$HOME/*"})):
            self.assertIsNotNone(self.why(tool, **inp), inp)
        self.assertIsNone(self.why("Glob", pattern="**/*.{java,kt}"))


class Grading(unittest.TestCase):
    def test_committed_changes_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            up, parent, _ = _upstream(Path(tmp))
            _git("checkout", "-q", parent, cwd=up)
            (up / "CLAUDE.md").write_text("harness\n")
            base = editgrade.snapshot(up)
            (up / "mod-a/src/main/java/A.java").write_text("class A { int v() { return 2; } }\n")
            (up / "mod-a/src/main/java/Old.java").unlink()
            _git("add", "-A", "mod-a", cwd=up)
            _git(*GIT_ID, "commit", "-qm", "agent", cwd=up)
            (up / "new.java").write_text("x")
            self.assertEqual(editgrade.changed_files(up, base), ["new.java"])  # what the status alone saw
            self.assertEqual(editgrade.changed_files(up, base, parent),
                             ["mod-a/src/main/java/A.java", "mod-a/src/main/java/Old.java", "new.java"])

    def test_equivalent_places_and_weak_tests_in_the_report(self):
        task = {"strata": {}, "primary_files": ["m/src/main/java/a/A.java"], "equivalents": [["n/src/main/java/b/B.java", "n/src/main/java/b/C.java"]],
                "gold_files": [{"path": f, "status": "M"} for f in ("m/src/main/java/a/A.java", "n/src/main/java/b/B.java", "n/src/main/java/b/C.java")]}
        A, B, C, X = (f"{m}/src/main/java/{p}" for m, p in (("m", "a/A.java"), ("n", "b/B.java"), ("n", "b/C.java"), ("n", "b/X.java")))
        one = editgrade.grade_edit(task, [A, C], {"passed": True})
        self.assertEqual((one["dependents_forgotten"], one["files_by_kind"]["code_changed"]["recall"]), ([], 1.0))
        none = editgrade.grade_edit(task, [A], {"passed": True})
        self.assertEqual(len(none["dependents_forgotten"]), 2)
        self.assertEqual(none["files_by_kind"]["code_changed"]["recall"], 0.5)
        # One of a group is one hit: precision is not inflated by the place that was not used.
        noisy = editgrade.grade_edit(task, [A, C, X, X.replace("X", "Y")], {"passed": True})["files_by_kind"]["code_changed"]
        self.assertEqual((noisy["precision"], noisy["recall"], noisy["tp"], noisy["n_changed"]), (0.5, 1.0, 2, 4))
        self.assertEqual(editgrade.grade_edit(task, [A, B, C], {"passed": True})["files_by_kind"]["code_changed"]["precision"], 1.0)
        # A dependent whose part can go into a primary file is reported apart, and only there.
        t2 = {k: v for k, v in task.items() if k != "equivalents"} | {"either": [B]}
        g = editgrade.grade_edit(t2, [A], {"passed": True})["dependents"]
        self.assertEqual((g["n"], g["forgotten"], g["by_kind"]["either"]), (1, [C], {"n": 1, "forgotten": [B]}))
        repos = {"r": {"name": "r", "commit": "c"}}
        base = {"id": "t", "repo": "r", "type": "edit", "answer_kind": "diff", "question": "q", "rationale": "r", "primary_files": [A],
                "gold_files": task["gold_files"]}
        self.assertTrue(any("never forgotten" in p for p in check_tasks([base | {"equivalents": [[A, B]]}], repos, check_evidence=False)))
        self.assertTrue(any("either must name" in p for p in check_tasks([base | {"either": [A]}], repos, check_evidence=False)))
        self.assertEqual(check_tasks([base | {"equivalents": [[B, C]]}], repos, check_evidence=False), [])
        rows = [{"condition": "none", "edit": one}, {"condition": "none", "edit": none, "weak_test": True}, {"condition": "D", "edit": one}]
        text = "\n".join(editgrade.report_lines(rows))
        self.assertIn("| `none` | all | 2 | 1.00 | 0.75 | 1.00 | 0.50 | – | – |", text)
        self.assertIn("| `none` | without weakly tested | 1 | 1.00 | 1.00 |", text)
        self.assertEqual(editgrade.report_lines([{"condition": "none", "edit": one}]), [])


class SharedPathAndDerivedIndex(unittest.TestCase):
    def test_sessions_of_one_work_path_never_overlap(self):
        from aeval.runner import Scheduler, WorkItem
        items = [WorkItem({"id": i, "repo": i}, "A", 1) for i in ("a", "b", "c")]
        sched = Scheduler(list(items), work_names={"a": "s", "b": "s", "c": "c"})
        self.assertEqual([sched.next().repo, sched.next().repo], ["a", "c"])  # b waits for a
        sched.done(items[0])
        self.assertEqual(sched.next().repo, "b")
        self.assertEqual(config.work_name({"name": "x"}), "x")

    def test_indexes_are_derived_newest_first_and_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            env = {"AEVAL_CACHE": str(root / "cache"), "AEVAL_WORK_ROOT": str(root / "work"), "AEVAL_TASKS": ""}
            # A stand-in for myc: counts come from REUSED/RELS next to it.
            fake = root / "myc"
            fake.write_text("#!/bin/sh\n[ \"$1\" = --version ] && { echo fake; exit; }\n"
                            "d=$(dirname \"$0\"); K=; R=10\n"
                            "case \"$*\" in *--force*) K=\", $(cat $d/REUSED) kept from the previous index\"; R=$(cat $d/RELS);; esac\n"
                            "echo \"  Symbols: 5\n  Files: 2\n  Relationships: $R\n  Embeddings: 5 (5/5 symbols$K)\n  Mentions: 1\"\n"
                            "mkdir -p \"$MYCELIUMS_DATA_DIR\"; printf '{\"repos\":{\"i\":{\"id\":\"i\",\"path\":\"%s\",\"symbol_count\":5,"
                            "\"file_count\":2,\"analyzed_commit\":\"%s\",\"analyzed_at\":\"x\"}}}' \"$2\" \"$(cat $2/HEAD)\" "
                            "> \"$MYCELIUMS_DATA_DIR/repos.json\"\n")
            fake.chmod(0o755)
            (root / "REUSED").write_text("4")
            (root / "RELS").write_text("10")
            repos = {}
            for n, d in (("old", "2025"), ("new", "2026")):
                co = root / "cache/repos" / n
                co.mkdir(parents=True)
                (co / "HEAD").write_text(n)
                repos[n] = {"name": n, "commit": n, "work_name": "shared", "parent_date": d}
            base = root / "base"
            (base / ".myceliums").mkdir(parents=True)
            (root / "work").mkdir()
            reg = {"repos": {"i": {"path": str(root / "work/shared")}}}
            (base / ".myceliums/repos.json").write_text(json.dumps(reg))
            with mock.patch.dict(os.environ, env):
                for h in (base / ".myceliums/snapshots/r",):  # what `myc diff` keeps of earlier (newer) commits
                    h.mkdir(parents=True)
                    (h / "future.json").write_text("symbols of a later commit")
                prep.index(repos, str(fake), "m", base=base)
                for n in repos:
                    self.assertEqual(list((root / f"cache/index/{n}/home").rglob("future.json")), [])
                self.assertTrue((base / ".myceliums/snapshots/r/future.json").exists())  # the base is left alone
                metas = {n: config.load_json(root / f"cache/index/{n}/meta.json") for n in repos}
                self.assertEqual(metas["new"]["derived"]["base"], str(base))
                self.assertEqual(metas["old"]["derived"]["base"], str(root / "cache/index/new/home"))
                self.assertEqual(metas["old"]["derived"]["vectors_reused"], 4)
                self.assertTrue(metas["old"]["derived"]["graph_equal"])
                (root / "RELS").write_text("9")  # a graph that differs from a fresh build stops the run
                with self.assertRaises(SystemExit) as e:
                    prep.index(repos, str(fake), "m", only={"new"}, base=base)
                self.assertIn("differs from a fresh one", str(e.exception))
                (root / "RELS").write_text("10")
                (root / "REUSED").write_text("0")
                with self.assertRaises(SystemExit):  # nothing reused: not a derived index
                    prep.index(repos, str(fake), "m", only={"new"}, base=base)
                reg["repos"]["i"]["path"] = "/elsewhere/shared"
                (base / ".myceliums/repos.json").write_text(json.dumps(reg))
                with self.assertRaises(SystemExit):  # another path: myc would silently re-embed everything
                    prep.index(repos, str(fake), "m", only={"new"}, base=base)


class WrapperGuard(unittest.TestCase):
    URL = "https\\://services.gradle.org/distributions/gradle-9.9.0-bin.zip"

    def _setup(self, root):
        co = root / "co"
        (co / "gradle/wrapper").mkdir(parents=True)
        (co / "gradle/wrapper/gradle-wrapper.properties").write_text(f"distributionBase=GRADLE_USER_HOME\ndistributionUrl={self.URL}\n")
        # The wrapper's own directory for this URL (seen on disk).
        return co, root / "g/wrapper/dists/gradle-9.9.0-bin/k7t1gqg1ujh3kgq8oiu7hqi4", {"GRADLE_USER_HOME": str(root / "g")}

    def test_only_a_complete_distribution_passes(self):
        import zipfile
        with tempfile.TemporaryDirectory() as tmp:
            co, d, env = self._setup(Path(tmp))
            self.assertIn("the wrapper would download it", editgrade.wrapper_problem(co, env))
            d.mkdir(parents=True)
            (d / "gradle-9.9.0-bin.zip.part").write_bytes(b"half")  # interrupted download
            (d / "gradle-9.9.0-bin.zip").write_bytes(b"PK\x03\x04 truncated")
            self.assertIsNotNone(editgrade.wrapper_problem(co, env))
            with zipfile.ZipFile(d / "gradle-9.9.0-bin.zip", "w") as z:
                z.writestr("gradle-9.9.0/bin/gradle", "x")
            self.assertIsNone(editgrade.wrapper_problem(co, env))
            (d / "gradle-9.9.0-bin.zip").unlink()
            (d / "gradle-9.9.0-bin.zip.ok").write_text("")  # a marker without the unpacked distribution
            self.assertIsNotNone(editgrade.wrapper_problem(co, env))
            (d / "gradle-9.9.0/bin").mkdir(parents=True)
            (d / "gradle-9.9.0/bin/gradle").write_text("x")
            self.assertIsNone(editgrade.wrapper_problem(co, env))
            self.assertIsNone(editgrade.wrapper_problem(Path(tmp), env))  # no wrapper, nothing to check

    def test_precheck_starts_nothing_without_it_and_uses_one_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            co, d, env = self._setup(root)
            scratch = root / "scratch"
            scratch.mkdir()
            ran = root / "ran"
            task = {"id": "t", "test_command": f"pwd > {ran}; exit 1"}
            with self.assertRaises(SystemExit) as e:
                editgrade.precheck(task, co, scratch, "c", env=env)
            self.assertIn("no Gradle was started", str(e.exception))
            self.assertFalse(ran.exists())
            (d / "gradle-9.9.0/bin").mkdir(parents=True)
            (d / "gradle-9.9.0/bin/gradle").write_text("x")
            (d / "gradle-9.9.0-bin.zip.ok").write_text("")
            paths = set()
            for tid in ("t", "u"):
                editgrade.precheck(dict(task, id=tid), co, scratch, "c", env=env)
                paths.add(ran.read_text().strip())
            self.assertEqual(len(paths), 1)

    def test_a_run_refuses_before_its_first_session(self):
        from aeval.runner import Runner, RunConfig
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            co, d, env = self._setup(root)
            (root / "cache/repos").mkdir(parents=True)
            co.rename(root / "cache/repos/r")
            repo = {"name": "r", "commit": "c", "session_env": env}
            with mock.patch.dict(os.environ, {"AEVAL_CACHE": str(root / "cache")}), \
                    mock.patch.object(config, "RUNS_DIR", root / "runs"):
                r = Runner(RunConfig(label="x", conditions=("A",), reps=1), [{"id": "t", "repo": "r"}], {"r": repo})
                r._session = lambda *a: self.fail("a session was started")
                with self.assertRaises(SystemExit) as e:
                    r.run()
            self.assertIn("the wrapper would download it", str(e.exception))


# Dependents a question names all the same; each has to be justified here.
NAMED_ANYWAY: dict[str, tuple] = {
    "retryable-method-retry-event": ("Retryable",),  # the feature's public name; its file changes in Javadoc only
}


class TaskFile(unittest.TestCase):
    def test_commit_replay_task_file_schema(self):
        path = Path(__file__).resolve().parents[1] / "tasks" / "tasks-commit-replay.json"
        if not path.exists():
            self.skipTest("no task file yet")
        with mock.patch.dict(os.environ, {"AEVAL_TASKS": str(path)}):
            data = json.loads(path.read_text())
            repos = config.load_repos()
            self.assertEqual(len({r["name"] for r in data["repos"]}), len(data["tasks"]))
            for t in data["tasks"]:
                r = repos[t["repo"]]
                self.assertEqual(r["env_from_caller"], ["JAVA_HOME"])
                self.assertTrue(set(t["test_overlay"]) <= set(editgrade.gold_paths(t)))
                # Network-free: every command, its environment, arm D and the prompt of every arm.
                self.assertTrue(t["test_command"].startswith("./gradlew --offline --no-scan --no-build-cache "), t["id"])
                self.assertTrue(r["isolated"])
                for search in (False, True):
                    self.assertTrue(bashcmd.isolated_bash(t["test_command"], t["allowed_commands"], "/w", "/w", search)[0], t["id"])
                    self.assertFalse(bashcmd.isolated_bash(t["test_command"].replace("--offline ", ""), t["allowed_commands"],
                                                           "/w", "/w", search)[0])
                # Every module the commit changes sources in is compiled, main and test.
                for m in {editgrade.module_of(g["path"]) for g in t["gold_files"] if g["path"].endswith((".java", ".kt"))} - {None}:
                    for what in ("compileJava", "compileTestJava"):
                        self.assertIn(f":{m}:{what} ", t["test_command"] + " ", t["id"])
                # No gold class is named, except the primary ones.
                for g in t["gold_files"]:
                    cls = g["path"].rsplit("/", 1)[-1].rsplit(".", 1)[0]
                    if g["path"].endswith((".java", ".kt")) and g["path"] not in t["primary_files"] and g["status"] != "A" \
                            and cls not in NAMED_ANYWAY.get(t["id"], ()):
                        self.assertNotRegex(t["question"], rf"(?<![A-Za-z.]){cls}(?![A-Za-z])", t["id"])
                self.assertIn("-Dhttps.proxyHost=127.0.0.1", r["session_env"]["GRADLE_OPTS"])
                self.assertEqual(r["session_env"]["DEVELOCITY_CACHE_SERVER"], "http://127.0.0.1:9")
                self.assertIn("--offline --no-scan --no-build-cache", build_prompt(t, r))
                q = t["question"].lower()
                for word in ("framework-docs", ".adoc", ".java", "src/main", "src/test"):
                    self.assertNotIn(word, q, (t["id"], word))


if __name__ == "__main__":
    unittest.main()
