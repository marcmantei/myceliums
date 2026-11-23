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
* ``allowed_commands`` (optional): the build/test commands arm D's Bash
  allowlist admits, exactly (strings) or as ``{"prefix", "args": REGEX}``
  (default: the test command);
* ``test_overlay`` (optional, #183): gold test files written from the
  reference commit into the working copy before the test command runs (the
  commit's own tests, which the agent never sees); they come from the
  repository's ``gold_commit``, which ``fetch`` keeps outside every working
  copy (:func:`apply_gold`). With a ``gold_commit`` the precheck also applies
  the whole commit and requires the test command to pass there.

Grading, after the session, from ``git status`` of the working copy:

* (a) the test command's exit status (``tests_passed``; ``None`` without one);
* (b) file-level recall and precision against ``gold_files``, overall and
  separately for code, tests and docs (:func:`file_kind`);
* (c) forgotten dependents, with ``primary_files``: the dependents the agent
  did not touch, split into ``docs``, ``tests``, ``remote`` (code in another
  directory than every primary file: another module) and ``local`` (code next
  to a primary file). Without ``primary_files`` every gold file would be a
  dependent and the metric would equal 1 − recall, so it is not reported
  (``None``). Commit-replay tasks (#183) report two kinds of dependent apart:
  ``comments`` (gold files that change in comments or imports only) and
  ``build`` (Gradle scripts). ``other_module`` (#183) counts the code dependents in another
  Gradle/Maven module than every primary file (the path before ``/src/``).

A session succeeds when the test command passes; without one, when the
file-level F1 over code files (all files if the gold has no code file)
reaches the run's success threshold.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import shutil
import zipfile
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


def changed_files(workdir: Path, baseline: dict[str, str] | None = None, base: str | None = None) -> list[str]:
    """Files that differ from ``HEAD`` in the working copy: modified, added,
    deleted, renamed (both paths) and untracked (``git status``).

    With ``base`` (a commit) they are the files that differ from it, so that
    changes the agent committed count like uncommitted ones.

    ``baseline`` (from :func:`snapshot` before the session) leaves out the
    files the harness changed and the agent did not touch afterwards: what
    ``myc setup-claude`` writes in arm C (``CLAUDE.md``, ``.mcp.json``)."""
    files = _status_files(workdir)
    if base:
        out = subprocess.run(["git", "-C", str(workdir), "diff", "--name-only", "--no-renames", "-z", base],
                             capture_output=True, text=True, timeout=120)
        if out.returncode != 0:
            raise RuntimeError(f"git diff {base} failed in {workdir}: {out.stderr.strip()[:200]}")
        files = sorted(set(files) | {f for f in out.stdout.split("\0") if f})
    if baseline:
        files = [f for f in files if f not in baseline or _digest(workdir / f) != baseline[f]]
    return files


def snapshot(workdir: Path) -> dict[str, str]:
    """The files that already differ from ``HEAD`` and their content digests."""
    return {f: _digest(workdir / f) for f in _status_files(workdir)}


def _digest(path: Path) -> str:
    if path.is_file():
        return hashlib.sha256(path.read_bytes()).hexdigest()
    return "dir" if path.is_dir() else "missing"


def _status_files(workdir: Path) -> list[str]:
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


def report_lines(rows: list[dict]) -> list[str]:
    """Edit tasks per condition, over all tasks and without those whose test
    command does not enforce every gold dependent (``weak_test``)."""
    rows = [r for r in rows if r.get("edit")]
    if not any("weak_test" in r for r in rows):
        return []

    def mean(vals):
        vals = [v for v in vals if v is not None]
        return f"{sum(vals) / len(vals):.2f}" if vals else "–"

    def part(r, key, field="recall"):
        return ((r["edit"].get("files_by_kind") or {}).get(key) or {}).get(field)

    def kept(r, key):
        k = ((r["edit"].get("dependents") or {}).get("by_kind") or {}).get(key) or {}
        return 1 - len(k["forgotten"]) / k["n"] if k.get("n") else None

    out = ["## Edit tasks", "",
           "Means over sessions. *Success* is the test command. A task is weakly tested when its test command does not "
           "enforce every gold dependent (`weak_test` in the task file); its success says less, so the table gives both. "
           "*Code*: recall of gold files whose code changes; *remote*, *docs*, *tests*: share of those dependents touched.",
           "", "| Condition | Tasks | Sessions | Success | Code | Code precision | Remote | Docs | Tests |",
           "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for name, keep in (("all", lambda r: True), ("without weakly tested", lambda r: not r.get("weak_test"))):
        for c in sorted({r["condition"] for r in rows}):
            own = [r for r in rows if r["condition"] == c and keep(r)]
            if own:
                out.append(f"| `{c}` | {name} | {len(own)} | {mean([float(r['edit']['success']) for r in own])} | "
                           f"{mean([part(r, 'code_changed') for r in own])} | "
                           f"{mean([part(r, 'code_changed', 'precision') for r in own])} | "
                           f"{mean([kept(r, 'remote') for r in own])} | {mean([kept(r, 'docs') for r in own])} | "
                           f"{mean([kept(r, 'tests') for r in own])} |")
    return out + [""]


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
    # #183: code whose code changes: without comment-only files and build scripts (on both sides).
    if "strata" in task:
        skip = {x["path"] for x in task["gold_files"] if x.get("comment_only")}
        keep = lambda fs: {f for f in fs if file_kind(f) == "code" and f not in skip and ".gradle" not in f.rsplit("/", 1)[-1]}  # noqa: E731
        by_kind["code_changed"] = _prf(keep(gold), keep(with_equivalents(task, got)))
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
    primary_modules = {module_of(p) for p in primary}
    comments = {g["path"] for g in task.get("gold_files") or [] if isinstance(g, dict) and g.get("comment_only")}
    # Reported apart only where a task marks them (#183).
    kinds = ("docs", "tests", "remote", "local") + (("comments", "build") if any(
        isinstance(g, dict) and "status" in g for g in task.get("gold_files") or []) and "strata" in task else ())
    out = {"n": 0, "forgotten": [], "by_kind": {k: {"n": 0, "forgotten": []} for k in kinds},
           "other_module": {"n": 0, "forgotten": []}}
    changed = with_equivalents(task, changed)
    for f in sorted(set(gold_paths(task)) - primary):
        kind = file_kind(f)
        if kind == "code":
            kind = "local" if (f.rsplit("/", 1)[0] if "/" in f else "") in primary_dirs else "remote"
            if "comments" in kinds and f in comments:
                kind = "comments"
            elif "build" in kinds and f.endswith((".gradle", ".gradle.kts")):
                kind = "build"
        out["n"] += 1
        out["by_kind"][kind]["n"] += 1
        far = kind in ("remote", "local") and module_of(f) is not None and module_of(f) not in primary_modules
        out["other_module"]["n"] += far
        if f not in changed:
            out["forgotten"].append(f)
            out["by_kind"][kind]["forgotten"].append(f)
            if far:
                out["other_module"]["forgotten"].append(f)
    return out


def with_equivalents(task: dict, changed: set[str]) -> set[str]:
    """``changed`` plus the gold files of every group of ``equivalents`` one
    member of which was changed: places where the same part of the change can
    go, so choosing one never counts as forgetting the other (#183)."""
    out = set(changed)
    for group in task.get("equivalents") or []:
        if out & set(group):
            out |= set(group)
    return out


def module_of(path: str) -> str | None:
    """The build module of a source path: what comes before ``/src/``
    (``spring-web`` for ``spring-web/src/main/java/...``); None outside one."""
    i = path.find("/src/")
    return path[:i] if i > 0 else None


def apply_gold(work: Path, gold: Path, task: dict, paths: list[str] | None = None) -> list[str]:
    """Write gold files from ``gold`` (``config.gold_dir``) into ``work``:
    ``paths`` (default: every gold file); a deleted gold file is removed.
    Returns the paths written or removed."""
    status = {g if isinstance(g, str) else g["path"]: "M" if isinstance(g, str) else g.get("status", "M")
              for g in task.get("gold_files") or []}
    done = []
    for rel in paths if paths is not None else list(status):
        dst = work / rel
        if status.get(rel) == "D":
            if dst.exists():
                dst.unlink()
        else:
            src = gold / "files" / rel
            if not src.is_file():
                raise RuntimeError(f"gold file {rel} missing in {gold}; run `agent_eval.py fetch`")
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dst)
        done.append(rel)
    return done


def gold_name_status(gold: Path) -> dict[str, str]:
    """The reference commit's changed files and their status, as ``fetch``
    recorded them (``gold.json``)."""
    return json.loads((gold / "gold.json").read_text())["files"]


def wrapper_problem(checkout: Path, env: dict | None = None) -> str | None:
    """Why running ``checkout``'s Gradle wrapper would open a connection: its
    distribution is not complete on disk. None when it is (unpacked with its
    ``.ok`` marker, or a whole zip the wrapper unpacks itself), or when there
    is no wrapper (#183).

    The wrapper ignores ``--offline`` for its own download, and a firewall sees
    that download as coming from ``gradle-wrapper.jar`` at the path of this
    working copy, so it must never get that far."""
    props = checkout / "gradle" / "wrapper" / "gradle-wrapper.properties"
    if not props.is_file():
        return None
    m = re.search(r"^distributionUrl\s*=\s*(\S+)", props.read_text(), re.M)
    if not m:
        return f"{props}: no distributionUrl"
    url = m.group(1).replace("\\:", ":")
    name = url.rsplit("/", 1)[-1]
    n, digest = int.from_bytes(hashlib.md5(url.encode()).digest(), "big"), ""
    while n:
        n, r = divmod(n, 36)
        digest = "0123456789abcdefghijklmnopqrstuvwxyz"[r] + digest
    env = env or {}
    home = Path(env.get("GRADLE_USER_HOME") or Path(env.get("HOME") or os.environ.get("HOME", "/")) / ".gradle")
    where = home / "wrapper" / "dists" / name.removesuffix(".zip") / digest
    zip_path = where / name
    if (where / (name + ".ok")).exists() and any(where.glob("*/bin/gradle")):
        return None
    if zip_path.is_file() and zipfile.is_zipfile(zip_path):
        return None
    return (f"Gradle distribution {name} is not complete in {where}: the wrapper would download it. "
            f"Put the zip there first; no Gradle was started")


def precheck(task: dict, checkout: Path, scratch: Path, commit: str, timeout_s: int = 900,
             gold: Path | None = None, env: dict | None = None, gold_commit: str | None = None,
             gold_runs: int = 1, log_dir: Path | None = None) -> dict:
    """Run the task's test command on a fresh copy of the pinned checkout
    (the gold commit's parent) before any agent runs, with the task's
    ``test_overlay`` applied (the commit's own tests). The test must fail
    there, or passing it after a session would prove nothing. With ``gold``
    (#183) the whole reference commit is then applied to the same copy and
    the test must pass. Returns the record ``check-tasks`` expects in the
    task's ``precheck``."""
    from .config import copy_tree, remove_tree
    if problem := wrapper_problem(checkout, env):
        raise SystemExit(f"{task['id']}: {problem}")
    # One fixed path for every task: a firewall knows a wrapper by its path.
    work = scratch / "precheck-work"
    lock = open(scratch.with_name(scratch.name + ".precheck.lock"), "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        raise SystemExit("another precheck is running; prechecks share one working path") from None
    if work.exists():  # left by an interrupted precheck
        remove_tree(work, scratch)
    copy_tree(checkout, work)
    after = None
    try:
        if task.get("test_overlay"):
            apply_gold(work, gold, task, task["test_overlay"])
        res = run_test(task["test_command"], work, timeout_s, env)
        if gold is not None:
            apply_gold(work, gold, task)
            # Executed for real: Gradle's (remote) build cache could otherwise
            # answer, and a repetition would be up to date.
            runs = [run_test(rerun_command(task["test_command"]), work, timeout_s, env)]
            while len(runs) < gold_runs and runs[-1]["passed"]:
                runs.append(run_test(rerun_command(task["test_command"]), work, timeout_s, env))
            after = runs[-1]
    finally:
        remove_tree(work, scratch)
        lock.close()
    kind = failure_kind(res)
    rec = {"fails_on_parent": not res["passed"] and kind not in ("timeout", "infrastructure", "no_tests"),
           "parent_failure": kind, "returncode": res["returncode"], "timed_out": res["timed_out"],
           "commit": commit, "command": task["test_command"], "seconds_parent": res["seconds"],
           "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    if task.get("test_overlay"):
        rec["test_overlay"] = list(task["test_overlay"])
    if after is not None:
        rec.update({"passes_on_gold": after["passed"], "gold_commit": gold_commit, "gold_returncode": after["returncode"],
                    "gold_timed_out": after["timed_out"], "seconds_gold": runs[0]["seconds"],
                    "gold_runs": len(runs), "gold_passes": sum(r["passed"] for r in runs),
                    "gold_failure": failure_kind(after), "seconds_gold_runs": [r["seconds"] for r in runs]})
    if log_dir is not None:
        log_dir.mkdir(parents=True, exist_ok=True)
        for n, r in [("parent", res)] + [(f"gold{i}", r) for i, r in enumerate(runs if gold is not None else [], 1)]:
            (log_dir / f"{task['id']}.{n}.log").write_text(r["output"])
    return rec


def failure_kind(res: dict) -> str | None:
    """Why a test run failed: ``tests`` (they ran and failed), ``compile``,
    ``infrastructure`` (a download, the wrapper or the toolchain), ``no_tests``
    (a filter matched nothing), ``timeout`` or ``other``; None when it passed.
    Only ``tests`` and ``compile`` say something about the change (#183)."""
    if res["passed"]:
        return None
    if res["timed_out"]:
        return "timeout"
    out = res.get("output") or res.get("output_tail") or ""
    for kind, pattern in (("infrastructure", r"is not complete in .*: the wrapper would download|Could not (?:GET|HEAD|download|resolve all)|Downloading from .* failed|"
                                             r"No matching toolchain|Cannot find a Java installation|UnknownHostException"),
                          ("no_tests", r"No tests found for given includes"),
                          ("compile", r"Execution failed for task ':[\w:-]*compile\w*'"),
                          ("tests", r"There were failing tests|\d+ tests? completed, \d+ failed")):
        if re.search(pattern, out):
            return kind
    return "other"


def rerun_command(command: str) -> str:
    """The command with Gradle's ``--rerun`` on each ``test`` task, so that a
    repetition runs the tests instead of taking them from the build cache."""
    return re.sub(r"(:[\w-]+:test)(?=\s|$)", r"\1 --rerun", command)


def run_test(command: str, workdir: Path, timeout_s: int = 900, env: dict | None = None,
             prefix=()) -> dict:
    """Run a task's test command in the working copy (``/bin/sh -c``), keeping
    the whole output in ``output``;
    ``env`` is added to a minimal environment (``PATH``, ``HOME``);
    ``prefix`` runs it inside a sandbox (aeval/isolation.py)."""
    started = time.time()
    base = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("HOME", "/tmp")}
    try:
        res = subprocess.run([*prefix, "/bin/sh", "-c", command], cwd=workdir, capture_output=True, text=True,
                             timeout=timeout_s, env={**base, **(env or {})})
        rc, out, timed_out = res.returncode, (res.stdout or "") + (res.stderr or ""), False
    except subprocess.TimeoutExpired as exc:
        rc, out, timed_out = None, str(exc.stdout or "") + str(exc.stderr or ""), True
    return {"command": command, "passed": rc == 0, "returncode": rc, "timed_out": timed_out,
            "seconds": round(time.time() - started, 2), "output_tail": out[-2000:], "output": out}
