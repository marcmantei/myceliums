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
  delete + add, and each as a group of ``equivalents``, so one dependent:
  :func:`renames`), ``comment_only`` where only comments or documentation
  text change (:func:`comment_only`);
* ``forced_tests``: the commit's test files that had to change, from the
  measured record ``forced_tests.json`` (aeval/forced.py; the 20 tasks only);
  one that fails (and compiles) only with a verdict in ``affected_183.json``
  that the task text states what it contradicts;
* ``optional``: code and docs dependents that the stated change does not
  affect at all, from the same verdicts;
* ``test_overlay``: the commit's test files (fixtures included), written
  over the working copy (or deleted) before the test command runs;
* ``test_command``: ``./gradlew``, main and test compilation of every module
  the commit changes, then (unless the spec names the tests) each module's
  ``test`` task, filtered to the commit's changed test classes;
* ``allowed_commands``: ``./gradlew`` with task paths, ``--tests`` filters
  and a few harmless flags, for arm D's Bash allowlist;
* ``strata``: files, code modules, whether docs change.

A precheck already recorded for the same commit and test command is kept.
"""

from __future__ import annotations

import argparse
import json
import random
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aeval.editgrade import file_kind, module_of  # noqa: E402

URL = "https://github.com/spring-projects/spring-framework.git"
EXCLUDE = re.compile(r"^(Upgrade|Bump|Polish|Update copyright|Replace file headers|Revert|Release|Next development|"
                     r"Merge|Update to|Prepare|Format|Apply|Consistent|Fix typo|Remove unused)", re.I)
MODULE = re.compile(r"^(spring-[a-z-]+|framework-[a-z]+|integration-tests)/")
# Gradle without the network and without a build cache (#183): dependencies
# from the warm cache, no build scan, and no task output carried over from
# another arm's session or from a precheck of the gold commit.
FLAGS = ("--offline", "--no-scan", "--no-build-cache")
OFFLINE = " ".join(FLAGS)
# What the hook lets any arm pass to ./gradlew: all three flags, harmless
# options, tasks by exact name (Gradle expands abbreviations, so a pattern
# would admit `bSPP`, buildScanPublishPrevious), and --tests filters.
GRADLE_TASKS = ("compileJava", "compileTestJava", "compileKotlin", "compileTestKotlin", "compileTestFixturesJava",
                "classes", "testClasses", "testFixturesClasses", "test", "checkstyleMain", "checkstyleTest")
GRADLE_OPTIONS = ("--no-daemon", "--console=plain", *FLAGS, "-q", "--quiet", "-i", "--info", "--stacktrace",
                  "--continue", "--rerun-tasks", "--rerun")
GRADLE_ARGS = ("".join(rf"(?=(?:.* )?{f}(?: |$))" for f in FLAGS)
               + r"(?:(?:" + "|".join(re.escape(o) for o in GRADLE_OPTIONS)
               + r"|(?::(?:spring|framework|integration)-[a-z0-9-]+:)?(?:" + "|".join(GRADLE_TASKS) + ")"
               r"|--tests [A-Za-z0-9_.*$#]+)(?: |$))+")
BUILD_NOTE = ("The build has no network access. Run Gradle as `./gradlew " + OFFLINE + " --no-daemon ...`, with "
              "fully qualified task paths such as `:spring-core:test --tests ...`; the output may be piped to `tail` "
              "or `head`. In shell commands, name files by paths relative to the working copy and do not use `cd`.")
FORCED = Path(__file__).resolve().with_name("forced_tests.json")
AFFECTED = Path(__file__).resolve().with_name("affected_183.json")
# A port nobody listens on: a build started without the flags cannot leave
# the machine either (not verified yet with a real build).
NOWHERE = "127.0.0.1"
SESSION_ENV = {
    # One build per session and no Kotlin daemon left behind: daemons would
    # outlive the session and pile up across Gradle versions.
    "GRADLE_OPTS": "-Dorg.gradle.daemon=false -Dorg.gradle.workers.max=4 -Dorg.gradle.project.kotlin.compiler.execution.strategy=in-process "
                   f"-Dhttp.proxyHost={NOWHERE} -Dhttp.proxyPort=9 -Dhttps.proxyHost={NOWHERE} -Dhttps.proxyPort=9 "
                   "-Dhttp.nonProxyHosts=localhost|127.*|[::1]",
    "DEVELOCITY_CACHE_SERVER": f"http://{NOWHERE}:9",  # the Spring build's remote cache (default ge.spring.io)
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


_LITERAL_OR_COMMENT = re.compile(r'"""(?:[^\\]|\\.)*?"""|"(?:[^"\\\n]|\\.)*"|\'(?:[^\'\\\n]|\\.)*\'|//[^\n]*|/\*.*?\*/', re.S)


def code_text(source: str) -> str:
    """Java/Kotlin source without comments (Javadoc), import lines and white space."""
    s = _LITERAL_OR_COMMENT.sub(lambda m: " " if m.group(0).startswith("/") else m.group(0), source)
    return "".join("".join(ln for ln in s.splitlines() if not ln.strip().startswith("import ")).split())


def schema_text(source: str) -> str:
    """An XML schema without its documentation elements, XML comments and
    white space around text."""
    def walk(e):
        kids = [walk(c) for c in e if not str(e.tag).endswith("}annotation") or not str(c.tag).endswith("}documentation")]
        return [e.tag, sorted(e.attrib.items()), (e.text or "").strip(), kids]
    return json.dumps(walk(ET.fromstring(source)))


def comment_only(hist: Path, sha: str, status: str, path: str) -> bool:
    """A modified file whose code is the same before and after the commit:
    source in which only comments, Javadoc or imports (for Javadoc links)
    changed, or an XML schema in which only documentation text did."""
    text = code_text if path.endswith((".java", ".kt")) else schema_text if path.endswith(".xsd") else None
    if status != "M" or text is None:
        return False
    return text(git(hist, "show", f"{sha}^:{path}")) == text(git(hist, "show", f"{sha}:{path}"))


def renames(hist: Path, sha: str) -> list[list[str]]:
    """``[old, new]`` of every file git calls renamed (its default similarity,
    50 %). ``name_status`` lists both, but they are one change: editing the old
    file in place and renaming it are equivalent."""
    out = git(hist, "show", "--format=", "--name-status", "--find-renames=50%", "--diff-filter=R", sha)
    return [line.split("\t")[1:3] for line in out.splitlines() if line.startswith("R")]


def spread(files: list[tuple[str, str]], comments: frozenset[str] = frozenset()) -> dict:
    """The commit's spread; ``code_modules`` are modules whose main code
    changes (``comments``: files that change in comments only)."""
    kinds = {"code": 0, "tests": 0, "docs": 0}
    modules, code_modules = set(), set()
    for _, path in files:
        kinds[file_kind(path)] += 1
        m = MODULE.match(path)
        if m:
            modules.add(m.group(1))
        if file_kind(path) == "code" and "/src/main/" in path and path not in comments:
            code_modules.add(module_of(path))
    return {"files": len(files), "kinds": kinds, "modules": sorted(modules), "code_modules": sorted(code_modules),
            "adoc": any(p.startswith("framework-docs/") and p.endswith(".adoc") for _, p in files)}


def compile_tasks(files: list[tuple[str, str]]) -> list[str]:
    """Main and test compilation of every module the commit changes sources
    in, so that a change that breaks one of them fails even when no test of
    that module runs."""
    mods: dict[str, set[str]] = {}
    for _, path in files:
        mod = module_of(path)
        if mod and path.endswith((".java", ".kt")):
            mods.setdefault(mod, set()).update({"compileJava", "compileTestJava"})
            if path.endswith(".kt"):
                mods[mod].update({"compileKotlin", "compileTestKotlin"})
    return [f":{m.replace('/', ':')}:{t}" for m in sorted(mods) for t in sorted(mods[m])]


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
    return f"./gradlew {OFFLINE} --no-daemon --console=plain " + " ".join(parts)


def cmd_build(args) -> int:
    hist = Path(args.history)
    spec = json.loads(Path(args.spec).read_text())
    out_path = Path(args.out)
    old = {t["id"]: t for t in json.loads(out_path.read_text())["tasks"]} if out_path.exists() else {}
    measured = json.loads(FORCED.read_text())["tasks"] if FORCED.exists() else {}
    verdicts = json.loads(AFFECTED.read_text())["tasks"] if AFFECTED.exists() else {}
    repos, tasks = [], []
    for s in spec["tasks"]:
        if s.get("drop") or bool(s.get("reserve")) != args.reserve:
            continue
        sha = git(hist, "rev-parse", s["commit"]).strip()
        parent = git(hist, "rev-parse", f"{sha}^").strip()
        files = name_status(hist, sha)
        comments = frozenset(p for st, p in files if comment_only(hist, sha, st, p))
        gold = [{"path": p, "status": st, **({"comment_only": True} if p in comments else {})} for st, p in files]
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
            "build_note": BUILD_NOTE, "isolated": True, "session_env": SESSION_ENV, "env_from_caller": ["JAVA_HOME"],
        })
        cmd = s.get("test_command") or default_test_command(files)
        if OFFLINE not in cmd:  # the spec's own commands name modules and tests only
            cmd = cmd.replace("./gradlew ", f"./gradlew {OFFLINE} ", 1)
        first = re.search(r" :", cmd).start()
        cmd = f"{cmd[:first]} {' '.join(compile_tasks(files))}{cmd[first:]}"
        # Build scripts only the commit's own tests need (spec): harness side, like the tests.
        build_overlay = [p for p in s.get("build_overlay", []) if p in paths]
        overlay = [p for _, p in files if file_kind(p) == "tests"] + build_overlay
        equivalents = [*s.get("equivalents", []), *renames(hist, sha)]
        forced = measured.get(s["id"])
        judged = verdicts.get(s["id"], {})
        optional = sorted(p for p, v in judged.items() if not v["counts"] and v["kind"] != "forced_tests")
        if forced and (forced["commit"], forced["gold_commit"]) != (parent, sha):
            raise SystemExit(f"{s['id']}: the forced-tests record is of another commit")
        sp = spread(files, comments)
        task = {
            "id": s["id"], "repo": name, "type": "edit", "answer_kind": "diff",
            "question": s["question"], "rationale": s["rationale"],
            "source": {"commit": sha, "subject": git(hist, "show", "-s", "--format=%s", sha).strip(),
                       "date": git(hist, "show", "-s", "--format=%cs", sha).strip(),
                       **({"issue": s["issue"]} if s.get("issue") else {})},
            "gold_files": gold, "primary_files": s["primary_files"],
            "test_command": cmd, "test_overlay": overlay,
            **({"build_overlay": build_overlay} if build_overlay else {}),
            "allowed_commands": [{"prefix": "./gradlew", "args": GRADLE_ARGS}],
            **({"equivalents": equivalents} if equivalents else {}),
            **({"forced_tests": sorted(p for p, f in forced["files"].items() if f["forced"] and (
                f["outcome"] != "fails" or judged[p]["counts"]))} if forced else {}),
            **({"optional": optional} if optional else {}),
            **({"either": s["either"]} if s.get("either") else {}),
            **({"untested": s["untested"]} if s.get("untested") else {}),
            **({"weak_test": s["weak_test"]} if s.get("weak_test") else {}),
            "test_timeout_s": s.get("test_timeout_s", 1800),
            "strata": {"files": sp["files"], "files_by_kind": sp["kinds"], "code_modules": len(sp["code_modules"]), "comment_only": len(comments),
                       "modules": sp["modules"], "docs": sp["kinds"]["docs"] > 0,
                       "agents_md": bool(git(hist, "ls-tree", "--name-only", parent, "AGENTS.md").strip()),
                       "size": "S" if sp["files"] <= 5 else "M" if sp["files"] <= 12 else "L"},
        }
        prev = old.get(s["id"], {}).get("precheck")
        # A failed download says nothing about the task: prechecked again.
        if prev and prev.get("parent_failure") == "infrastructure":
            prev = None
        if prev and prev.get("command") == cmd and prev.get("commit") == parent:
            task["precheck"] = prev
        tasks.append(task)
    data = {"description": spec["description"], "repos": repos, "tasks": tasks}
    out_path.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(tasks)} tasks -> {out_path}", file=sys.stderr)
    return 0


PILOT_SEED = 183


def draw_pilot(candidates: list[dict], seed: int = PILOT_SEED) -> dict:
    """The pilot's tasks: one per stratum (size x docs) among the candidates,
    then one of them each for the pipeline checks in C and D. Candidates and
    strata are sorted, so the seed alone decides."""
    rng = random.Random(seed)
    strata: dict = {}
    for c in sorted(candidates, key=lambda c: c["id"]):
        strata.setdefault((c["size"], c["docs"]), []).append(c["id"])
    tasks = [rng.choice(strata[k]) for k in sorted(strata)]
    c, d = rng.sample(tasks, 2)
    return {"none": tasks, "C": [c], "D": [d]}


def cmd_pilot(args) -> int:
    """Draw the pilot once, from the tasks whose precheck is valid today, and record it."""
    out = Path(args.out)
    if out.exists():
        raise SystemExit(f"{out} exists: the pilot is drawn once")
    data = json.loads(Path(args.tasks).read_text())
    cands = [{"id": t["id"], "size": t["strata"]["size"], "docs": t["strata"]["docs"]} for t in data["tasks"]
             if (t.get("precheck") or {}).get("fails_on_parent") and t["precheck"].get("passes_on_gold")]
    draw = draw_pilot(cands)
    out.write_text(json.dumps({"seed": PILOT_SEED, "candidates": sorted(cands, key=lambda c: c["id"]), **draw}, indent=1) + "\n")
    print(json.dumps(draw))
    return 0


def cmd_dependents(args) -> int:
    """The registered number of dependents by type and task (a test compares the task file with it)."""
    from aeval import report_commit
    tasks = json.loads(Path(args.tasks).read_text())["tasks"]
    report_commit.measured(tasks)
    out = {t["id"]: report_commit.by_type(t) for t in tasks}
    Path(args.out).write_text(json.dumps(out, indent=1) + "\n")
    print({k: sum(v[k] for v in out.values()) for k in report_commit.KINDS})
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
    s = sub.add_parser("pilot", help="draw the pilot's tasks (once)")
    s.add_argument("tasks")
    s.add_argument("out")
    s.set_defaults(fn=cmd_pilot)
    s = sub.add_parser("dependents", help="write the registered number of dependents by type and task")
    s.add_argument("tasks")
    s.add_argument("out")
    s.set_defaults(fn=cmd_dependents)
    args = p.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
