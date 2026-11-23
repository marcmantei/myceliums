"""Commit-replay edit tasks on spring-framework (#183): mine candidate commits
from the history, then build the task file from a hand-written spec.

    # a blobless clone with history (the session checkouts are shallow, at the parent only)
    git clone --filter=blob:none --no-checkout https://github.com/spring-projects/spring-framework.git HIST
    python3 -I commit_replay.py mine HIST --since 2025-10-01 > candidates.json
    python3 -I commit_replay.py build HIST commit_replay_spec.json ../tasks/tasks-commit-replay.json
    python3 -I commit_replay.py build HIST commit_replay_spec.json ../tasks/tasks-commit-replay-reserve.json --reserve

``mine`` lists every non-merge commit on ``origin/main`` since the date that

* changes at most 25 files, at most two of them renamed or copied;
* is not a dependency bump, polish, copyright, revert or release commit (by
  subject);
* changes main code (``src/main/**``) and tests, and touches at least two
  build modules (top-level ``spring-*``, ``framework-*`` or
  ``integration-tests`` directories; ``framework-docs`` counts);
* has a parent whose build uses the Java 25 toolchain (one JDK for every task).

It prints each with its spread: files by kind (aeval/editgrade.py), code
modules, whether it changes an AsciiDoc page, and the Gradle version of the
parent. Which candidates become tasks, and their descriptions, are decided by
hand (the spec); ``build`` derives everything else from the commit:

* one pinned repository per task (``repos`` in the task file): the parent,
  with ``gold_commit`` the replayed commit;
* ``gold_files``: the commit's name-status against its parent (renames as
  delete + add);
* ``test_overlay``: the commit's test files (fixtures included), written
  over the working copy (or deleted) before the test command runs;
* ``test_command`` (unless the spec gives one): ``./gradlew`` with each
  module's ``test`` task, filtered to the commit's changed test classes;
* ``allowed_commands``: ``./gradlew`` with task paths, ``--tests`` filters
  and a few harmless flags, for arm D's Bash allowlist;
* ``strata``: files, code modules, whether docs change.

A precheck already recorded for the same commit and test command is kept.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aeval.editgrade import file_kind, module_of  # noqa: E402

URL = "https://github.com/spring-projects/spring-framework.git"
EXCLUDE = re.compile(r"^(Upgrade|Bump|Polish|Update copyright|Replace file headers|Revert|Release|Next development|"
                     r"Merge|Update to|Prepare|Format|Apply|Consistent|Fix typo|Remove unused)", re.I)
MODULE = re.compile(r"^(spring-[a-z-]+|framework-[a-z]+|integration-tests)/")
# Gradle flags that cannot load code or configuration (no -I/-D/-P/--init-script/-c/-p).
GRADLE_ARGS = (r"(?:(?:--no-daemon|--console=plain|--offline|-q|--quiet|-i|--info|--stacktrace|--continue|"
               r"--rerun-tasks|--rerun|:[a-z][a-z-]*:[A-Za-z]+|[a-z][A-Za-z]*|--tests [A-Za-z0-9_.*$#]+)(?: |$))+")
SESSION_ENV = {
    # One build per session and no Kotlin daemon left behind: daemons would
    # outlive the session and pile up across Gradle versions.
    "GRADLE_OPTS": "-Dorg.gradle.daemon=false -Dorg.gradle.project.kotlin.compiler.execution.strategy=in-process",
}


def git(hist: Path, *args: str) -> str:
    res = subprocess.run(["git", "-C", str(hist), *args], capture_output=True, text=True)
    if res.returncode != 0:
        raise SystemExit(f"git {' '.join(args)}: {res.stderr.strip()}")
    return res.stdout


def name_status(hist: Path, sha: str) -> list[tuple[str, str]]:
    out = []
    for line in git(hist, "show", "--format=", "--name-status", "--no-renames", sha).splitlines():
        parts = line.split("\t")
        out.append((parts[0][0], parts[-1]))
    return out


def spread(files: list[tuple[str, str]]) -> dict:
    kinds = {"code": 0, "tests": 0, "docs": 0}
    modules, code_modules = set(), set()
    for _, path in files:
        kinds[file_kind(path)] += 1
        m = MODULE.match(path)
        if m:
            modules.add(m.group(1))
        if file_kind(path) == "code" and "/src/main/" in path:
            code_modules.add(module_of(path))
    return {"files": len(files), "kinds": kinds, "modules": sorted(modules), "code_modules": sorted(code_modules),
            "adoc": any(p.startswith("framework-docs/") and p.endswith(".adoc") for _, p in files)}


def parent_build(hist: Path, sha: str) -> tuple[str, str]:
    """(Gradle version, Java toolchain) of the commit's parent."""
    props = subprocess.run(["git", "-C", str(hist), "show", f"{sha}^:gradle/wrapper/gradle-wrapper.properties"],
                           capture_output=True, text=True).stdout
    conv = subprocess.run(["git", "-C", str(hist), "show",
                           f"{sha}^:buildSrc/src/main/java/org/springframework/build/JavaConventions.java"],
                          capture_output=True, text=True).stdout
    g = re.search(r"gradle-([\d.]+)-bin", props)
    j = re.search(r"DEFAULT_LANGUAGE_VERSION = JavaLanguageVersion\.of\((\d+)\)", conv)
    return (g.group(1) if g else "?", j.group(1) if j else "?")


def cmd_mine(args) -> int:
    hist = Path(args.history)
    log = git(hist, "log", "origin/main", "--no-merges", f"--since={args.since}", "--format=@@%H%x09%cs%x09%s",
              "--name-status")
    commits, cur = [], None
    for line in log.splitlines():
        if line.startswith("@@"):
            sha, date, subject = line[2:].split("\t", 2)
            cur = {"sha": sha, "date": date, "subject": subject, "raw": []}
            commits.append(cur)
        elif line.strip():
            cur["raw"].append(line.split("\t")[0][0])
    out = []
    for c in commits:
        if not c["raw"] or len(c["raw"]) > 25 or EXCLUDE.search(c["subject"]):
            continue
        if sum(s in "RC" for s in c["raw"]) > 2:
            continue
        files = name_status(hist, c["sha"])
        s = spread(files)
        if not s["code_modules"] or not s["kinds"]["tests"] or len(s["modules"]) < 2:
            continue
        gradle, java = parent_build(hist, c["sha"])
        if java != "25":
            continue
        out.append({"sha": c["sha"], "date": c["date"], "subject": c["subject"], **s, "gradle": gradle})
    json.dump({"since": args.since, "n": len(out), "candidates": out}, sys.stdout, indent=1)
    print()
    print(f"{len(out)} candidates", file=sys.stderr)
    return 0


def default_test_command(files: list[tuple[str, str]]) -> str:
    by_module: dict[str, list[str]] = {}
    for status, path in files:
        if status == "D" or file_kind(path) != "tests":
            continue
        mod = module_of(path)
        if mod is None:
            continue
        by_module.setdefault(mod, [])
        m = re.search(r"src/test/(?:java|kotlin)/(.+)\.(?:java|kt)$", path)
        if m and re.search(r"Tests?$", m.group(1)):
            by_module[mod].append(m.group(1).replace("/", "."))
    parts = [f":{mod}:test" + "".join(f" --tests {c}" for c in sorted(classes))
             for mod, classes in sorted(by_module.items())]
    return "./gradlew --no-daemon --console=plain " + " ".join(parts)


def cmd_build(args) -> int:
    hist = Path(args.history)
    spec = json.loads(Path(args.spec).read_text())
    out_path = Path(args.out)
    old = {t["id"]: t for t in json.loads(out_path.read_text())["tasks"]} if out_path.exists() else {}
    repos, tasks = [], []
    for s in spec["tasks"]:
        if s.get("drop") or bool(s.get("reserve")) != args.reserve:
            continue
        sha = git(hist, "rev-parse", s["commit"]).strip()
        parent = git(hist, "rev-parse", f"{sha}^").strip()
        files = name_status(hist, sha)
        gold = [{"path": p, "status": st} for st, p in files]
        paths = [p for _, p in files]
        for p in s["primary_files"]:
            if p not in paths:
                raise SystemExit(f"{s['id']}: primary file {p} not in the commit")
        name = f"spring-{s['id']}"
        gradle, java = parent_build(hist, sha)
        repos.append({
            "name": name, "display_name": "Spring Framework", "language": "java", "language_name": "Java",
            "url": URL, "ref": f"parent of {sha[:12]}", "commit": parent, "gold_commit": sha,
            "gradle": gradle, "java_toolchain": java, "work_name": "spring-framework",
            "parent_date": git(hist, "show", "-s", "--format=%cI", parent).strip(),
            "session_env": SESSION_ENV, "env_from_caller": ["JAVA_HOME"],
        })
        cmd = s.get("test_command") or default_test_command(files)
        overlay = [p for _, p in files if file_kind(p) == "tests"]
        sp = spread(files)
        task = {
            "id": s["id"], "repo": name, "type": "edit", "answer_kind": "diff",
            "question": s["question"], "rationale": s["rationale"],
            "source": {"commit": sha, "subject": git(hist, "show", "-s", "--format=%s", sha).strip(),
                       "date": git(hist, "show", "-s", "--format=%cs", sha).strip(),
                       **({"issue": s["issue"]} if s.get("issue") else {})},
            "gold_files": gold, "primary_files": s["primary_files"],
            "test_command": cmd, "test_overlay": overlay,
            "allowed_commands": [{"prefix": "./gradlew", "args": GRADLE_ARGS}],
            "test_timeout_s": s.get("test_timeout_s", 900),
            "strata": {"files": sp["files"], "files_by_kind": sp["kinds"], "code_modules": len(sp["code_modules"]),
                       "modules": sp["modules"], "docs": sp["kinds"]["docs"] > 0,
                       "size": "S" if sp["files"] <= 5 else "M" if sp["files"] <= 12 else "L"},
        }
        prev = old.get(s["id"], {}).get("precheck")
        if prev and prev.get("command") == cmd and prev.get("commit") == parent:
            task["precheck"] = prev
        tasks.append(task)
    data = {"description": spec["description"], "repos": repos, "tasks": tasks}
    out_path.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(tasks)} tasks -> {out_path}", file=sys.stderr)
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("mine")
    s.add_argument("history")
    s.add_argument("--since", default="2025-10-01")
    s.set_defaults(fn=cmd_mine)
    s = sub.add_parser("build")
    s.add_argument("history")
    s.add_argument("spec")
    s.add_argument("out")
    s.add_argument("--reserve", action="store_true",
                   help="build the spec's reserve tasks (prechecked as replacements) instead of the selected ones")
    s.set_defaults(fn=cmd_build)
    args = p.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
