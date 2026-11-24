"""Which of a commit's test changes a change forces (#183).

The replayed commit's test files are of two sorts: those that had to change so
that the build and the tests stay green, and those its author added or
extended. Only the first are dependents in the sense of "what the change
needs". :func:`measure` tells them apart by running them, not by reading: the
commit's **non-test** files are written over the parent, and a test file is
**forced** when its **parent version** does not compile there, or a test of it
fails there. A test file that is new in the commit is never forced.

States of one working copy, in this order (``FLAGS``, the grading sandbox, the
same working path as the prechecks):

* ``X``, a fresh copy of the parent with the commit's non-test files, every test
  file as at the parent: main and test code of every module the commit changes
  is compiled (``--continue``). A gold test file with a compile error is
  ``compile``. Those files are then replaced by the commit's version (the
  commit's new test files come with them) and the compilation repeated until it
  passes, because one error can hide another; a file that fails only then is
  ``compile_after_others``.
* ``XT``: the tests of the remaining files, in their parent version. Any failing
  test: ``fails``; else ``passes``.
* ``G`` (control): the whole commit, the tests of the changed files.
* ``P`` (control): the untouched parent, the same files in their parent version.

A file is *forced* (``compile``, ``compile_after_others``, ``fails``) or *not
forced* (``passes``, ``new``) only when its tests ran and passed in both
controls. A failing run is repeated once; what differs between the two, a file
whose tests did not run, or a control that fails, is ``unclear`` and listed,
never guessed.
"""

from __future__ import annotations

import hashlib
import json
import re
import time
import xml.etree.ElementTree as ET
from pathlib import Path

from .editgrade import apply_gold, failure_kind, file_kind, gold_paths, module_of, precheck_copy, rerun_command, run_test, wrapper_problem

FORCED = ("compile", "compile_after_others", "fails")
NOT_FORCED = ("passes", "new")
_JAVAC = re.compile(r"^(/\S+?\.java):(\d+): error: (.*)$", re.M)
_KOTLIN = re.compile(r"^e: (?:file://)?(/\S+?\.kt):(\d+)(?::\d+)? (.*)$", re.M)
_CLASS = re.compile(r"src/(?:test|testFixtures)/(?:java|kotlin)/(.+)\.(?:java|kt)$")


def sha256(path: Path) -> str | None:
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def test_files(task: dict) -> tuple[dict[str, str], list[str]]:
    """``(old, new)``: the commit's test files that exist at the parent, with
    their status, and those it adds."""
    status = {g["path"]: g["status"] for g in task["gold_files"]}
    tests = [p for p in gold_paths(task) if file_kind(p) == "tests"]
    return {p: status[p] for p in tests if status[p] != "A"}, [p for p in tests if status[p] == "A"]


def state_hash(task: dict, checkout: Path, gold: Path) -> str:
    """Of everything the outcome depends on in the sources: the parent commit's
    old test files and the commit's files."""
    old, _new = test_files(task)
    items = [["parent", p, sha256(checkout / p)] for p in sorted(old)]
    items += [[g["status"], g["path"], sha256(gold / "files" / g["path"])] for g in sorted(task["gold_files"], key=lambda g: g["path"])]
    return hashlib.sha256(json.dumps(items).encode()).hexdigest()


def compile_errors(output: str, work: Path) -> dict[str, list[str]]:
    """Compiler errors by file (relative path)."""
    out: dict[str, list[str]] = {}
    for rx in (_JAVAC, _KOTLIN):
        for m in rx.finditer(output):
            p = Path(m.group(1))
            rel = str(p.relative_to(work)) if work in p.parents else str(p)
            out.setdefault(rel, []).append(f"line {m.group(2)}: {m.group(3)}")
    return out


def test_classes(work: Path, path: str) -> list[str]:
    """The test classes that run a file's tests: its own, or for an abstract
    class its subclasses in the same test source set."""
    m = _CLASS.search(path)
    if not m:
        return []
    name = m.group(1).replace("/", ".")
    simple = name.rsplit(".", 1)[-1]
    source = (work / path).read_text(encoding="utf-8") if (work / path).is_file() else ""
    if not re.search(rf"\babstract class {simple}\b", source):
        return [name]
    root = work / path[:m.start(1)]
    subs = [p for p in sorted(root.rglob("*.java")) if re.search(rf"\bclass \w+ extends {simple}\b", p.read_text(encoding="utf-8"))]
    return [str(p.relative_to(root))[:-5].replace("/", ".") for p in subs]


def results(work: Path, modules: list[str]) -> dict[str, dict]:
    """JUnit's XML reports: per top-level class the tests run, and the failed
    ones by method name."""
    out: dict[str, dict] = {}
    for m in modules:
        for f in sorted((work / m / "build" / "test-results" / "test").glob("TEST-*.xml")):
            for case in ET.parse(f).getroot().iter("testcase"):
                top = case.get("classname", "").split("$")[0]
                r = out.setdefault(top, {"run": 0, "failed": []})
                if case.find("skipped") is not None:
                    continue
                r["run"] += 1
                bad = case.find("failure") if case.find("failure") is not None else case.find("error")
                if bad is not None:
                    said = (bad.get("message") or bad.get("type") or "").strip().splitlines()
                    r["failed"].append({"class": case.get("classname", "").rsplit(".", 1)[-1], "name": case.get("name") or "",
                                        "message": said[0][:160] if said else ""})
    return out


def of_file(work: Path, path: str, res: dict[str, dict], changed: list[str]) -> dict:
    """The results that are this file's. A failure in a subclass of an abstract
    test class belongs to the changed file that declares the method."""
    def source(p):
        return (work / p).read_text(encoding="utf-8") if (work / p).is_file() else ""

    def declares(p, test):
        name = re.match(r"\w+", test["name"])
        return bool(name and re.search(rf"\bvoid {name.group(0)}\(", source(p)))

    classes = test_classes(work, path)
    abstract = re.search(rf"\babstract class {Path(path).stem}\b", source(path))
    out: dict = {"run": 0, "failed": []}
    for c in classes:
        r = res.get(c, {"run": 0, "failed": []})
        out["run"] += r["run"]
        for t in r["failed"]:
            inherited = not declares(path, t) and any(declares(p, t) and c in test_classes(work, p) for p in changed if p != path)
            if declares(path, t) if abstract else not inherited:
                out["failed"].append(f"{t['class']}.{t['name']}: {t['message']}")
    return out


class Session:
    """One task's working copy, its commands and their record."""

    def __init__(self, task, work, env, prefix, log_dir, timeout_s):
        self.task, self.work, self.env, self.prefix, self.log_dir, self.timeout_s = task, work, env, prefix, log_dir, timeout_s
        self.steps: list[dict] = []
        self.gradle = task["test_command"].split(" :", 1)[0]
        self.compile = [x for x in task["test_command"].split() if re.fullmatch(r":[\w:-]+:compile\w+", x)]

    def run(self, step: str, args: str) -> dict:
        command = f"{self.gradle} --continue {args}"
        res = run_test(command, self.work, self.timeout_s, self.env, self.prefix)
        rec = {"step": step, "command": command, "returncode": res["returncode"], "timed_out": res["timed_out"], "seconds": res["seconds"],
               "failure": failure_kind(res), "log_sha256": hashlib.sha256(res["output"].encode()).hexdigest()}
        if self.log_dir:
            self.log_dir.mkdir(parents=True, exist_ok=True)
            (self.log_dir / f"{self.task['id']}.{len(self.steps):02d}-{step}.log").write_text(res["output"])
        self.steps.append(rec)
        return {**rec, "output": res["output"], "passed": res["passed"]}

    def compile_all(self, step: str) -> dict:
        res = self.run(step, " ".join(self.compile))
        if not res["passed"] and not compile_errors(res["output"], self.work):  # not the sources' fault, as far as the log says
            res = self.run(step + "-again", " ".join(self.compile))
        return res

    def tests(self, step: str, paths: list[str]) -> tuple[dict, dict[str, dict], dict]:
        """Run the files' tests; a run with a failure is repeated once, and
        ``(result, results per class, results of the repetition)`` returned."""
        by_module: dict[str, set[str]] = {}
        for p in paths:
            for c in test_classes(self.work, p):
                by_module.setdefault(module_of(p), set()).add(c)
        args = rerun_command(" ".join(f":{m}:test" + "".join(f" --tests {c}" for c in sorted(cs)) for m, cs in sorted(by_module.items())))
        def once(name):
            for m in by_module:
                for f in (self.work / m / "build" / "test-results" / "test").glob("TEST-*.xml"):
                    f.unlink()
            return self.run(name, args), results(self.work, sorted(by_module))
        res, got = once(step)
        again: dict = {}
        if not res["passed"]:
            _res2, again = once(step + "-again")
        return res, got, again


def measure(task: dict, checkout: Path, scratch: Path, gold: Path, commits: tuple[str, str], timeout_s: int = 1800, env: dict | None = None,
            sandbox=None, log_dir: Path | None = None) -> dict:
    """The record of one task (see the module's text); ``commits`` is the pinned
    parent and the gold commit."""
    old, new = test_files(task)
    nontest = [p for p in gold_paths(task) if file_kind(p) != "tests"]
    status = {g["path"]: g["status"] for g in task["gold_files"]}
    files = {p: {"status": status[p], "outcome": "new" if p in new else None, "parent_sha256": sha256(checkout / p),
                 "gold_sha256": sha256(gold / "files" / p), "evidence": [], "controls": {}} for p in [*old, *new]}
    rec = {"commit": commits[0], "gold_commit": commits[1], "state_sha256": state_hash(task, checkout, gold),
           "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "sandboxed": sandbox is not None, "steps": [], "files": files, "notes": []}
    if problem := wrapper_problem(checkout, env):
        raise SystemExit(f"{task['id']}: {problem}")
    with precheck_copy(checkout, scratch) as work:
        work = Path(work).resolve()
        prefix: tuple = ()
        if sandbox is not None:
            prefix, extra = sandbox(work)
            env = {**(env or {}), **extra}
        s = Session(task, work, env, prefix, log_dir, timeout_s)
        rec["steps"] = s.steps
        if old:
            _states(task, s, work, checkout, gold, files, old, new, nontest, rec["notes"])
    for p, f in files.items():
        f["forced"] = True if f["outcome"] in FORCED else False if f["outcome"] in NOT_FORCED else None
    return rec


def _states(task, s: Session, work: Path, checkout: Path, gold: Path, files, old, new, nontest, notes) -> None:
    # X: the commit without its tests, on the parent.
    apply_gold(work, gold, task, nontest)
    res, n, stuck = s.compile_all("X"), 0, None
    while not res["passed"]:
        errors = compile_errors(res["output"], work)
        hit = [p for p in old if p in errors and files[p]["outcome"] is None]
        other = sorted(set(errors) - set(files))
        if other:
            notes.append(f"{s.steps[-1]['step']}: errors outside the commit's test files: {', '.join(other[:5])}")
        if not hit:
            stuck = f"{s.steps[-1]['step']} fails ({s.steps[-1]['failure']}) without an error in a test file of the commit"
            break
        for p in hit:
            files[p]["outcome"] = "compile" if n == 0 else "compile_after_others"
            files[p]["evidence"] = errors[p][:3] + ([f"... {len(errors[p])} errors"] if len(errors[p]) > 3 else [])
            files[p]["command"] = s.steps[-1]["command"]
        n += 1
        apply_gold(work, gold, task, hit + new)
        res = s.compile_all(f"X{n}")
    rest = [p for p in old if files[p]["outcome"] is None]
    if stuck:
        notes.append(stuck)
        for p in rest:
            files[p].update(outcome="unclear", evidence=[stuck])
        rest = []
    # XT: the tests of the files that compile as they were.
    if rest:
        res, got, again = s.tests("XT", rest)
        for p in rest:
            a, b = of_file(work, p, got, rest), of_file(work, p, again, rest) if again else None
            files[p]["command"] = s.steps[-1]["command"] if not again else s.steps[-2]["command"]
            files[p]["tests_run"] = a["run"]
            if not a["run"]:
                files[p].update(outcome="unclear", evidence=[f"no test ran ({res['failure']})"])
            elif not a["failed"]:
                files[p]["outcome"] = "passes"
            elif b is not None and set(b["failed"]) and {x.split(": ")[0] for x in a["failed"]} == {x.split(": ")[0] for x in b["failed"]}:
                files[p].update(outcome="fails", evidence=a["failed"][:5] + ([f"... {len(a['failed'])} failed"] if len(a["failed"]) > 5 else []))
            else:
                files[p].update(outcome="unclear", evidence=["not the same failures twice", *a["failed"][:3]])
    # Controls: the whole commit, then the untouched parent.
    apply_gold(work, gold, task)
    _control("G", s, work, files, [p for p in old if (work / p).is_file()], notes)
    for p in gold_paths(task):
        src, dst = checkout / p, work / p
        if src.is_file():
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(src.read_bytes())
        else:
            dst.unlink(missing_ok=True)
    _control("P", s, work, files, list(old), notes)


def _control(step: str, s: Session, work: Path, files, paths, notes) -> None:
    res, got, again = s.tests(step, paths) if paths else (None, {}, {})
    for p in paths:
        r = of_file(work, p, got, paths)
        if r["failed"] or not r["run"]:
            r = of_file(work, p, again, paths) if again else r
            notes.append(f"control {step} failed at first for {p}")
        ok = bool(r["run"]) and not r["failed"]
        files[p]["controls"][step] = {"run": r["run"], "failed": r["failed"][:5]}
        if not ok:
            files[p].update(outcome="unclear", evidence=[f"control {step}: {r['run']} tests run, {len(r['failed'])} failed", *files[p]["evidence"]][:5])


def problems(record: dict, task: dict, checkout: Path | None = None, gold: Path | None = None) -> list[str]:
    """Why a stored record does not belong to this task (and, with the checkout
    and the gold files, to these sources)."""
    old, new = test_files(task)
    out = []
    if set(record["files"]) != {*old, *new}:
        out.append(f"{task['id']}: the record's test files are not the task's")
    for p, f in record["files"].items():
        if p in new and f["forced"] is not False:
            out.append(f"{task['id']}: {p} is new, so not forced")
        if f["forced"] is None:
            out.append(f"{task['id']}: {p} is unclear")
    if checkout and gold and state_hash(task, checkout, gold) != record["state_sha256"]:
        out.append(f"{task['id']}: the sources are not those of the record")
    return out
