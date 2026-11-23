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
                         "./gradlew --offline --no-scan --no-daemon --console=plain :spring-context:test --tests org.y.CTest "
                         ":spring-web:test --tests org.x.ATests --tests org.x.BTests")

    def test_arm_d_admits_gradle_test_runs_but_nothing_that_loads_code(self):
        rule = [{"prefix": "./gradlew", "args": commit_replay.GRADLE_ARGS}]
        off = "./gradlew --offline --no-scan "
        ok = [off + "--no-daemon --console=plain :spring-web:test --tests org.x.ATests",
              "./gradlew :spring-core:compileJava --no-scan --offline", off + "-q :spring-web:test --tests 'org.x.A*'",
              off + "build"]
        bad = [off + "-I /tmp/init.gradle :spring-web:test", off + "-Dx=1 test", off + "-Pa=b test",
               off + "--init-script x test", off + "-p /elsewhere test", off + "test; grep -r x .",
               off + "test --tests $(grep -l x .)", "gradle --offline --no-scan test",
               # network: a missing flag, a scan, a refresh, an upload of a stored scan, artifacts
               "./gradlew test", "./gradlew --offline test", "./gradlew --no-scan test", off + "--scan test",
               off + "--refresh-dependencies test", off + "buildScanPublishPrevious", off + ":spring-core:publish",
               off + "publishAllPublicationsToCentral", "./gradlew --offline --no-scanX test"]
        for c in ok:
            self.assertTrue(bashcmd.allowed(c, rule)[0], c)
        for c in bad:
            self.assertFalse(bashcmd.allowed(c, rule)[0], c)


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
                prep.index(repos, str(fake), "m", base=base)
                metas = {n: config.load_json(root / f"cache/index/{n}/meta.json") for n in repos}
                self.assertEqual(metas["new"]["derived"]["base"], str(base))
                self.assertEqual(metas["old"]["derived"]["base"], str(root / "cache/index/new/home"))
                self.assertEqual(metas["old"]["derived"]["vectors_reused"], 4)
                self.assertTrue(metas["old"]["derived"]["graph_equal"])
                (root / "RELS").write_text("9")  # a graph that differs from a fresh build is recorded
                prep.index(repos, str(fake), "m", only={"new"}, base=base)
                self.assertFalse(config.load_json(root / "cache/index/new/meta.json")["derived"]["graph_equal"])
                (root / "REUSED").write_text("0")
                with self.assertRaises(SystemExit):  # nothing reused: not a derived index
                    prep.index(repos, str(fake), "m", only={"new"}, base=base)
                reg["repos"]["i"]["path"] = "/elsewhere/shared"
                (base / ".myceliums/repos.json").write_text(json.dumps(reg))
                with self.assertRaises(SystemExit):  # another path: myc would silently re-embed everything
                    prep.index(repos, str(fake), "m", only={"new"}, base=base)


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
                self.assertTrue(t["test_command"].startswith("./gradlew --offline --no-scan "), t["id"])
                self.assertTrue(bashcmd.allowed(t["test_command"], t["allowed_commands"])[0], t["id"])
                self.assertFalse(bashcmd.allowed(t["test_command"].replace("--offline ", ""), t["allowed_commands"])[0])
                self.assertIn("-Dhttps.proxyHost=127.0.0.1", r["session_env"]["GRADLE_OPTS"])
                self.assertEqual(r["session_env"]["DEVELOCITY_CACHE_SERVER"], "http://127.0.0.1:9")
                self.assertIn("--offline --no-scan", build_prompt(t, r))
                q = t["question"].lower()
                for word in ("framework-docs", ".adoc", ".java", "src/main", "src/test"):
                    self.assertNotIn(word, q, (t["id"], word))


if __name__ == "__main__":
    unittest.main()
