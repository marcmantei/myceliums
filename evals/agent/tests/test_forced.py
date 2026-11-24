"""#183: which test changes a commit forces, measured with a stand-in for Gradle, and what follows from it in the grading."""

import json
import os
import sys
import tempfile
import unittest
import unittest.mock
from pathlib import Path

EVAL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL))

from aeval import forced  # noqa: E402

# Test code "compiles" when its API= equals the main code's, and a test passes when its V= does.
GRADLE = r'''
import pathlib, re, sys
args, root, ok = sys.argv[1:], pathlib.Path.cwd(), True
with open(root.parent / "commands", "a") as fh: fh.write(" ".join(args) + "\n")
def val(p, k):
    m = re.search(k + r"=(\d)", p.read_text()); return m and m.group(1)
main = {m.name: next(iter(sorted((m / "src/main/java").rglob("*.java")))) for m in root.iterdir() if (m / "src/main/java").is_dir()}
def compile(mod):
    good = True
    files = sorted((root / mod / "src").glob("test*/java/**/*.java"))
    hides = lambda f: "HIDES" in f.read_text() and val(main[mod], "API") == "2"  # an error after which no other is reported
    if any(map(hides, files)): files = [f for f in files if hides(f)]
    for f in files:
        if hides(f) or (val(f, "API") or val(main[mod], "API")) != val(main[mod], "API"):
            print(f"{f}:3: error: cannot find symbol"); good = False
    if not good: print(f"Execution failed for task ':{mod}:compileTestJava'.")
    return good
i = 0
while i < len(args):
    a = args[i]; i += 1
    m = re.fullmatch(r":([\w-]+):(compileTestJava|test)", a)
    if not m: continue
    mod = m.group(1)
    if not compile(mod): ok = False; continue
    if m.group(2) == "compileTestJava": continue
    classes = []
    while i < len(args) and (args[i] in ("--rerun", "--tests") or args[i - 1] == "--tests"):
        if args[i - 1] == "--tests": classes.append(args[i])
        i += 1
    out = root / mod / "build/test-results/test"; out.mkdir(parents=True, exist_ok=True)
    for c in classes:
        f = root / mod / "src/test/java" / (c.replace(".", "/") + ".java"); text = f.read_text()
        if "NORUN" in text: continue
        cases = []
        bases = [f] + [f.with_name(b + ".java") for b in re.findall(r"extends (\w+)", text)]
        for b in bases:
            for name, v in re.findall(r"void (\w+)\(\) \{ V=([\d*])", b.read_text()):
                bad = v not in ("*", val(main[mod], "V"))
                if "FLAKY" in b.read_text() and val(main[mod], "V") == "2":
                    seen = root.parent / "flaky"; bad = not seen.exists(); seen.touch()
                cases.append(f'<testcase classname="{c}$Nested" name="{name}()">' + ('<failure message="expected &lt;2&gt;"/>' if bad else "") + "</testcase>")
                ok = ok and not bad
        (out / f"TEST-{c}.xml").write_text("<testsuite>" + "".join(cases) + '<testcase classname="x" name="s"><skipped/></testcase></testsuite>')
if not ok: print("There were failing tests")
sys.exit(0 if ok else 1)
'''
A, OTHER = "m/src/main/java/p/A.java", "m/src/test/java/p/"


class Measure(unittest.TestCase):
    def measure(self, tests: dict, **kw):
        """``tests``: name -> (parent text or None, gold text or None). Main code: API=1 V=1 -> API=2 V=2."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name).resolve()
        checkout, gold, scratch = root / "co", root / "gold", root / "cache" / "tmp"
        scratch.mkdir(parents=True)
        for base, main in ((checkout, "API=1 V=1"), (gold / "files", "API=2 V=2")):
            (base / A).parent.mkdir(parents=True)
            (base / A).write_text(main)
        (checkout / "gradlew").write_text(f"#!/bin/sh\nexec {sys.executable} -I {root}/gradle.py \"$@\"\n")
        (checkout / "gradlew").chmod(0o755)
        (root / "gradle.py").write_text(GRADLE)
        files = [{"path": A, "status": "M"}]
        for name, (old, new) in tests.items():
            path = OTHER + name + ".java"
            for base, text in ((checkout, old), (gold / "files", new)):
                if text is not None:
                    (base / path).parent.mkdir(parents=True, exist_ok=True)
                    (base / path).write_text(text)
            files.append({"path": path, "status": "A" if old is None else "D" if new is None else "M"})
        task = {"id": "t", "gold_files": files, "test_command": "./gradlew --offline :m:compileJava :m:compileTestJava :m:test --tests p.X"}
        rec = forced.measure(task, checkout, scratch, gold, ("p", "g"), 60, {"PATH": os.environ["PATH"]}, log_dir=root / "logs")
        self.assertFalse((scratch / "precheck-work").exists())
        self.commands = (scratch / "commands").read_text().splitlines()
        self.assertEqual(forced.problems(rec, task, checkout, gold), [f"t: {p} is unclear" for p, f in rec["files"].items() if f["forced"] is None])
        self.assertEqual(json.loads(json.dumps(rec)), rec)
        (checkout / A).write_text("other")
        self.rec, self.task, self.sources = rec, task, (checkout, gold)
        return {p.removeprefix(OTHER).removesuffix(".java"): f for p, f in rec["files"].items()}

    def test_compile_fail_pass_and_new(self):
        f = self.measure({"CTests": ("API=1 void c() { V=1 }", "API=2 void c() { V=2 }"), "FTests": ("void f() { V=1 } void ok() { V=* }", "void f() { V=2 }"),
                          "NTests": (None, "API=2 void n() { V=2 }"), "GoneTests": ("API=1 void g() { V=1 }", None)})
        self.assertEqual({k: (v["outcome"], v["forced"]) for k, v in f.items()},
                         {"CTests": ("compile", True), "FTests": ("fails", True), "NTests": ("new", False), "GoneTests": ("compile", True)})
        self.assertEqual(f["CTests"]["evidence"], ["line 3: cannot find symbol"])
        self.assertIn("f(): expected <2>", f["FTests"]["evidence"][0])
        self.assertEqual([s["step"] for s in self.rec["steps"]], ["X", "X1", "XT", "XT-again", "G", "P"])
        self.assertEqual((f["GoneTests"]["controls"], f["FTests"]["controls"]["P"]), ({"P": {"run": 1, "failed": []}}, {"run": 2, "failed": []}))
        self.assertIn("--continue", self.commands[0])
        self.assertTrue(all(s["log_sha256"] and s["command"] for s in self.rec["steps"]))
        # The record is of these sources: another gold or parent test file is another state.
        checkout, gold = self.sources
        (checkout / (OTHER + "FTests.java")).write_text("other")
        self.assertIn("t: the sources are not those of the record", forced.problems(self.rec, self.task, checkout, gold))

    def test_a_test_that_passes_as_it_was_is_not_forced(self):
        """The commit added tests to it; nobody had to. And a parent version that fails at the parent says nothing."""
        f = self.measure({"PTests": ("void p() { V=* }", "void p() { V=* } void q() { V=2 }"), "BTests": ("void b() { V=2 }", "void b() { V=2 }")})
        self.assertEqual((f["PTests"]["outcome"], f["PTests"]["forced"], f["PTests"]["tests_run"]), ("passes", False, 1))
        self.assertEqual((f["BTests"]["outcome"], f["BTests"]["forced"]), ("unclear", None))
        self.assertEqual([s["step"] for s in self.rec["steps"]], ["X", "XT", "G", "P", "P-again"])

    def test_one_error_can_hide_another(self):
        f = self.measure({"STests": ("HIDES void s() { V=1 }", "API=2 void s() { V=2 }"), "CTests": ("API=1 void c() { V=1 }", "API=2 void c() { V=2 }"),
                          "PTests": ("void p() { V=* }", "void p() { V=* } void q() { V=2 }")})
        self.assertEqual({k: (v["outcome"], v["forced"]) for k, v in f.items()},
                         {"STests": ("compile", True), "CTests": ("compile_after_others", True), "PTests": ("passes", False)})
        self.assertEqual([s["step"] for s in self.rec["steps"]], ["X", "X1", "X2", "XT", "G", "P"])

    def test_a_failure_elsewhere_is_not_the_tests_fault(self):
        with unittest.mock.patch.object(forced, "compile_errors", lambda out, work: {}):
            f = self.measure({"CTests": ("API=1 void c() { V=1 }", "API=2 void c() { V=2 }")})
        self.assertEqual((f["CTests"]["outcome"], f["CTests"]["forced"]), ("unclear", None))
        self.assertEqual([s["step"] for s in self.rec["steps"]], ["X", "X-again", "G", "P"])  # repeated once

    def test_flaky_and_silent_files_are_listed_not_guessed(self):
        f = self.measure({"KTests": ("FLAKY void k() { V=1 }", "void k() { V=2 }"), "ZTests": ("NORUN", "void z() { V=2 }")})
        self.assertEqual((f["KTests"]["outcome"], f["ZTests"]["outcome"], f["KTests"]["forced"]), ("unclear", "unclear", None))
        self.assertIn("not the same failures twice", " ".join(f["KTests"]["evidence"]))

    def test_an_abstract_class_runs_through_its_subclasses(self):
        f = self.measure({"AbstractTests": ("abstract class AbstractTests { void a() { V=1 } }", "abstract class AbstractTests { void a() { V=2 } }"),
                          "SubTests": ("class SubTests extends AbstractTests { void s() { V=1 } }", "class SubTests extends AbstractTests { void s() { V=2 } }")})
        self.assertEqual((f["AbstractTests"]["outcome"], f["SubTests"]["outcome"]), ("fails", "fails"))
        self.assertEqual([x.split(".")[1][:2] for x in f["AbstractTests"]["evidence"]], ["a("])  # each failure belongs to the file that declares it
        self.assertEqual([x.split(".")[1][:2] for x in f["SubTests"]["evidence"]], ["s("])


class Record(unittest.TestCase):
    """The measured record of the 20 tasks (no Java here: structure, and that the task file is built from it)."""

    def setUp(self):
        self.record = json.loads((EVAL / "groundtruth/forced_tests.json").read_text())["tasks"]
        self.data = json.loads((EVAL / "tasks/tasks-commit-replay.json").read_text())

    def test_every_task_has_a_decided_record_of_its_commit(self):
        repos = {r["name"]: r for r in self.data["repos"]}
        self.assertEqual(set(self.record), {t["id"] for t in self.data["tasks"]})
        for t in self.data["tasks"]:
            rec = self.record[t["id"]]
            self.assertEqual(forced.problems(rec, t), [])
            self.assertEqual((rec["commit"], rec["gold_commit"]), (repos[t["repo"]]["commit"], repos[t["repo"]]["gold_commit"]))
            self.assertTrue(rec["sandboxed"])
            self.assertEqual(t["forced_tests"], sorted(p for p, f in rec["files"].items() if f["forced"]))
            for s in rec["steps"]:
                self.assertIn(" --offline --no-scan --no-build-cache ", s["command"])
            for p, f in rec["files"].items():
                self.assertEqual(f["forced"], f["outcome"] in forced.FORCED, p)
                if f["status"] != "A":  # decided only with both controls passed (a deleted file has no version in the commit)
                    self.assertEqual(sorted(f["controls"]), ["P"] if f["status"] == "D" else ["G", "P"], p)
                    for c in f["controls"].values():
                        self.assertTrue(c["run"] and not c["failed"], p)

    def test_the_sources_are_still_those_of_the_record(self):
        from aeval import config
        seen = 0
        for t in self.data["tasks"]:
            checkout, gold = config.cache_dir() / "repos" / t["repo"], config.gold_dir(t["repo"])
            if checkout.is_dir() and gold.is_dir():
                seen += 1
                self.assertEqual(forced.problems(self.record[t["id"]], t, checkout, gold), [])
        if not seen:
            self.skipTest("no checkouts here")


if __name__ == "__main__":
    unittest.main()
