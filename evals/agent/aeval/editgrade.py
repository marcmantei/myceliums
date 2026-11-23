"""Edit tasks: the answer is a change to the working copy, not a list (#178).

An edit task (``"type": "edit"``, ``"answer_kind": "diff"``) asks the agent to
make a change; the gold is the set of files a reference commit changed (for
the commit replays of #183: the commit itself, replayed on its parent). The
task carries

* ``gold_files``: ``[{"path": "...", "status": "M" | "A" | "D"}, ...]`` (a
  bare string is a modified file);
* ``primary_files`` (optional): the gold file(s) holding the core change;
  every other gold file is a **dependent** (a call site, a re-export, a test,
  a doc page that has to follow);
* ``test_command`` (optional): a shell command, run in the working copy after
  the session, that must exit 0; ``precheck`` records that it **fails** on
  the pinned (parent) commit before any agent runs (:func:`precheck`,
  ``agent_eval.py precheck-edit``), so passing it means the change was made;
* ``allowed_commands`` (optional): build/test command prefixes arm D's Bash
  allowlist admits (default: the test command).

Grading, after the session, from ``git status`` of the working copy:

* (a) the test command's exit status (``tests_passed``; ``None`` without one);
* (b) file-level recall and precision against ``gold_files``, overall and
  separately for code, tests and docs (:func:`file_kind`);
* (c) forgotten dependents, with ``primary_files``: the dependents the agent
  did not touch, split into ``docs``, ``tests``, ``remote`` (code in another
  directory than every primary file: another module) and ``local`` (code next
  to a primary file). Without ``primary_files`` every gold file would be a
  dependent and the metric would equal 1 − recall, so it is not reported
  (``None``).

A session succeeds when the test command passes; without one, when the
file-level F1 over code files (all files if the gold has no code file)
reaches the run's success threshold.
"""

from __future__ import annotations

import os
import re
import subprocess
import time
from pathlib import Path

KINDS = ("code", "tests", "docs")
_DOC_EXT = (".md", ".adoc", ".rst")
_DOC_DIR = re.compile(r"(^|/)(docs|framework-docs)/")
_MAIN_SET = re.compile(r"(^|/)src/main/")
_TEST_SET = re.compile(r"(^|/)src/(test|testFixtures|integrationTest)/")
_TEST_DIR = re.compile(r"(^|/)(tests|__tests__)/")
_TEST_NAME = re.compile(r"(^test_.*\.py$|_test\.(py|go)$|\.(test|spec)\.[cm]?[jt]sx?$|Tests?\.(java|kt)$|_spec\.rb$)")


def file_kind(path: str) -> str:
    """``code``, ``tests`` or ``docs`` by path, first rule that matches:

    1. a Java/Gradle source set decides: ``src/main/**`` is code (also inside
       a module named ``spring-test/``), ``src/test/**``, ``src/testFixtures/**``
       and ``src/integrationTest/**`` are tests (fixtures count as tests);
    2. docs: ``*.md``, ``*.adoc``, ``*.rst``, and anything under a ``docs/``
       or ``framework-docs/`` tree (Django's ``docs/*.txt``); other ``.txt``
       files (``CMakeLists.txt``, ``requirements.txt``) are not docs;
    3. tests: anything under ``tests/`` or ``__tests__/``, and test file names
       (``test_*.py``, ``*_test.py``, ``*_test.go``, ``*.test.ts``,
       ``*.spec.js``, ``*Test.java``, ``*Tests.kt``, ``*_spec.rb``); a library
       package named ``test`` (``django/test/``) is code;
    4. everything else is code.
    """
    name = path.rsplit("/", 1)[-1]
    if _MAIN_SET.search(path):
        return "code"
    if _TEST_SET.search(path):
        return "tests"
    if name.lower().endswith(_DOC_EXT) or _DOC_DIR.search(path):
        return "docs"
    if _TEST_DIR.search(path) or _TEST_NAME.search(name):
        return "tests"
    return "code"


def gold_paths(task: dict) -> list[str]:
    return [g if isinstance(g, str) else g["path"] for g in task.get("gold_files") or []]


def changed_files(workdir: Path) -> list[str]:
    """Files that differ from ``HEAD`` in the working copy: modified, added,
    deleted, renamed (both paths) and untracked (``git status``)."""
    out = subprocess.run(["git", "-C", str(workdir), "status", "--porcelain=v1", "-z", "-uall"],
                         capture_output=True, text=True, timeout=120)
    if out.returncode != 0:
        raise RuntimeError(f"git status failed in {workdir}: {out.stderr.strip()[:200]}")
    files: list[str] = []
    entries = out.stdout.split("\0")
    i = 0
    while i < len(entries):
        e = entries[i]
        i += 1
        if len(e) < 4:
            continue
        status, path = e[:2], e[3:]
        files.append(path)
        if "R" in status or "C" in status:  # the next entry is the source path
            if i < len(entries) and entries[i]:
                files.append(entries[i])
            i += 1
    return sorted(set(files))


def _prf(gold: set[str], got: set[str]) -> dict:
    tp = len(gold & got)
    p = tp / len(got) if got else (1.0 if not gold else 0.0)
    r = tp / len(gold) if gold else (1.0 if not got else 0.0)
    f1 = 2 * p * r / (p + r) if p + r else 0.0
    return {"precision": round(p, 4), "recall": round(r, 4), "f1": round(f1, 4), "tp": tp,
            "n_gold": len(gold), "n_changed": len(got)}


def grade_edit(task: dict, changed: list[str], tests: dict | None, threshold: float = 0.8) -> dict:
    """Grade the files a session changed against the task's gold files."""
    gold = set(gold_paths(task))
    got = set(changed)
    by_kind = {}
    for kind in KINDS:
        g = {f for f in gold if file_kind(f) == kind}
        c = {f for f in got if file_kind(f) == kind}
        if g or c:
            by_kind[kind] = _prf(g, c)
    forgotten = forgotten_dependents(task, got)
    overall = _prf(gold, got)
    passed = None if tests is None else bool(tests.get("passed"))
    if passed is not None:
        success = passed
    else:
        basis = by_kind.get("code") if any(file_kind(f) == "code" for f in gold) else overall
        success = (basis or {}).get("f1", 0.0) >= threshold - 1e-9
    return {
        "files_changed": sorted(got),
        "file_precision": overall["precision"], "file_recall": overall["recall"], "file_f1": overall["f1"],
        "files_by_kind": by_kind,
        "dependents_forgotten": forgotten["forgotten"] if forgotten else None,
        "dependents": forgotten,
        "files_extra": sorted(got - gold),
        "tests_passed": passed,
        "test": tests,
        "success": success,
    }


def forgotten_dependents(task: dict, changed: set[str]) -> dict | None:
    """Dependents (gold files other than ``primary_files``) the agent did not
    touch, by kind; None without ``primary_files``."""
    primary = task.get("primary_files")
    if not primary:
        return None
    primary = set(primary)
    primary_dirs = {p.rsplit("/", 1)[0] if "/" in p else "" for p in primary}
    out = {"n": 0, "forgotten": [], "by_kind": {k: {"n": 0, "forgotten": []} for k in ("docs", "tests", "remote", "local")}}
    for f in sorted(set(gold_paths(task)) - primary):
        kind = file_kind(f)
        if kind == "code":
            kind = "local" if (f.rsplit("/", 1)[0] if "/" in f else "") in primary_dirs else "remote"
        out["n"] += 1
        out["by_kind"][kind]["n"] += 1
        if f not in changed:
            out["forgotten"].append(f)
            out["by_kind"][kind]["forgotten"].append(f)
    return out


def precheck(task: dict, checkout: Path, scratch: Path, commit: str, timeout_s: int = 900) -> dict:
    """Run the task's test command on a fresh copy of the pinned checkout
    (the gold commit's parent) before any agent runs. The test must fail
    there, or passing it after a session would prove nothing. Returns the
    record ``check-tasks`` expects in the task's ``precheck``."""
    from .config import copy_tree, remove_tree
    work = scratch / f"precheck-{task['id']}"
    copy_tree(checkout, work)
    try:
        res = run_test(task["test_command"], work, timeout_s)
    finally:
        remove_tree(work, scratch)
    return {"fails_on_parent": not res["passed"] and not res["timed_out"], "returncode": res["returncode"], "timed_out": res["timed_out"],
            "commit": commit, "command": task["test_command"],
            "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}


def run_test(command: str, workdir: Path, timeout_s: int = 900, env: dict | None = None) -> dict:
    """Run a task's test command in the working copy (``/bin/sh -c``)."""
    started = time.time()
    try:
        res = subprocess.run(["/bin/sh", "-c", command], cwd=workdir, capture_output=True, text=True,
                             timeout=timeout_s, env=env or {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                                                            "HOME": os.environ.get("HOME", "/tmp")})
        rc, out, timed_out = res.returncode, (res.stdout or "") + (res.stderr or ""), False
    except subprocess.TimeoutExpired as exc:
        rc, out, timed_out = None, str(exc.stdout or "") + str(exc.stderr or ""), True
    return {"command": command, "passed": rc == 0, "returncode": rc, "timed_out": timed_out,
            "seconds": round(time.time() - started, 2), "output_tail": out[-2000:]}
