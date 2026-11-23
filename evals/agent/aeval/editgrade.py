"""Edit tasks: the answer is a change to the working copy, not a list (#178).

An edit task (``"type": "edit"``, ``"answer_kind": "diff"``) asks the agent to
make a change; the gold is the set of files a reference commit changed (for
the commit replays of #183: the commit itself, replayed on its parent). The
task carries

* ``gold_files``: ``[{"path": "...", "status": "M" | "A" | "D"}, ...]`` (a
  bare string is a modified file);
* ``dependents`` (optional): the gold files that depend on the core change
  (call sites, re-exports, tests of the changed code), the ones an agent
  forgets; default: every gold file;
* ``test_command`` (optional): a shell command, run in the working copy after
  the session, that must exit 0.

Grading, after the session, from ``git status`` of the working copy:

* (a) the test command's exit status (``tests_passed``; ``None`` without one);
* (b) file-level recall and precision against ``gold_files``, overall and
  separately for code, tests and docs (:func:`file_kind`);
* (c) forgotten dependents: the ``dependents`` the agent did not touch.

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
_DOC_EXT = (".md", ".rst", ".txt", ".adoc")
_TEST_NAME = re.compile(r"(^test_.*\.py$|_test\.(py|go)$|\.(test|spec)\.[cm]?[jt]sx?$|Test\.java$|_spec\.rb$)")
_TEST_DIR = re.compile(r"(^|/)(tests?|__tests__|spec|testing)/")


def file_kind(path: str) -> str:
    """``tests``, ``docs`` or ``code`` by path: test directories and test
    file names first, then documentation (``docs/``, ``.md``, ``.rst``,
    ``.txt``, ``.adoc``, ``CHANGELOG``/``README``), everything else code."""
    name = path.rsplit("/", 1)[-1]
    if _TEST_DIR.search(path) or _TEST_NAME.search(name):
        return "tests"
    if (path.startswith("docs/") or "/docs/" in path or name.lower().endswith(_DOC_EXT)
            or name.upper().startswith(("CHANGELOG", "README"))):
        return "docs"
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
    dependents = task.get("dependents")
    dependents = gold_paths({"gold_files": dependents}) if dependents is not None else sorted(gold)
    forgotten = sorted(set(dependents) - got)
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
        "dependents_forgotten": forgotten,
        "n_dependents": len(dependents),
        "files_extra": sorted(got - gold),
        "tests_passed": passed,
        "test": tests,
        "success": success,
    }


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
