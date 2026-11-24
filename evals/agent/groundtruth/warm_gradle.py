"""One online warm-up of the Gradle cache for the commit-replay tasks (#183).

    JAVA_HOME=<original JDK 25> python3 -I warm_gradle.py --dry-run   # every command and host; starts nothing
    JAVA_HOME=<original JDK 25> python3 -I warm_gradle.py --go        # run it, once, in one go

The only step that needs the network, and with the network it only **resolves
and compiles**. For every task without a valid offline precheck it puts the
replayed commit's tree at ONE fixed path and runs the task's own compile tasks
and test tasks there without ``--offline``, with an init script
(:data:`INIT`) that takes the actions of every test task away, also in
``buildSrc``: its inputs are compiled and its runtime class path is resolved
(downloaded), and **no test runs**. A report of a run test stops the warm-up.
The build logic of the commit (buildSrc, plugins, code generators, annotation
processors) **does run, unsandboxed and with the network**, as in any online build.
Tests run afterwards: offline, sandboxed, with the JDK copy (below).

It stops at the first error and says what to do; a build that takes longer than
20 minutes is killed with everything it started (a blocked daemon too). No ``--continue``, no build
scan, no build cache (local or remote; the remote one's address is a closed
port), no JDK download. A connection or a read that does not answer fails
within about a minute (:data:`NETWORK`: 45 s each, one attempt); so does one a
firewall holds back longer than that: start again when it is answered. The
wrapper never downloads: without the distribution on disk nothing starts.

Java connects itself, not through another program: a firewall sees it as what
it is. :data:`HOSTS` is **informative**, what the builds are known to ask for;
this script enforces nothing, **the firewall on the machine does**. Nothing
else on the machine is changed besides ``~/.gradle`` and the fixed path.

With the original JDK only: the IPv4 copy (make_jdk_ipv4.py) is for offline
builds and is refused here. Afterwards, offline and with the copy: move the old
frozen dependencies away, ``agent_eval.py gradle-cache``, ``precheck-edit
--gold-runs 2 --sandbox``.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import time
from pathlib import Path

EVAL = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(EVAL))
TASKS = EVAL / "tasks" / "tasks-commit-replay.json"
os.environ.setdefault("AEVAL_TASKS", str(TASKS))

from aeval import config, editgrade, isolation  # noqa: E402

ROOT = Path("/private/tmp/aeval-warm")
WORK = ROOT / "spring-framework"
INIT_PATH = ROOT / "resolve-only.gradle"
HOSTS = {
    "repo.maven.apache.org": "Maven Central: libraries",
    "repo.spring.io": "Spring milestones and snapshots (only where a build uses them)",
    "plugins.gradle.org": "Gradle plugin portal (redirects to plugins-artifacts.gradle.org)",
    "plugins-artifacts.gradle.org": "the portal's files",
    "nodejs.org": "release list read by the docs module's node plugin (seen as a notice in the offline logs)",
}
MINUTES = (2, 6)  # per build; first builds online took 2-9 minutes on 23 November beside a busy CPU
TIMEOUT_S = 1200  # one build
NETWORK = ("-Dorg.gradle.internal.http.connectionTimeout=45000 -Dorg.gradle.internal.http.socketTimeout=45000 "
           "-Dorg.gradle.internal.repository.max.tentatives=1 -Dorg.gradle.internal.network.retry.max.attempts=1 "
           "-Porg.gradle.java.installations.auto-download=false")
# Last thing before the build runs, in every build (buildSrc too): a test task resolves and does nothing else.
INIT = """\
gradle.taskGraph.whenReady { graph ->
    graph.allTasks.findAll { it instanceof Test }.each { test ->
        def classpath = test.classpath
        def launcher = test.jvmArgumentProviders
        test.actions = []
        test.doLast {
            launcher.each { it.asArguments() }
            println "aeval: resolved ${classpath.files.size()} files for ${it.path}, no test run"
        }
    }
}
"""


def valid(task: dict, repo: dict) -> bool:
    p = task.get("precheck") or {}
    return bool(p.get("fails_on_parent") and p.get("passes_on_gold") and p.get("command") == task["test_command"]
                and p.get("commit") == repo["commit"])


def online_command(test_command: str) -> str:
    """The task's command with the network: its compile and test tasks, no
    ``--tests`` filter (nothing runs), the init script and short timeouts."""
    cmd = re.sub(r" --tests \S+", "", test_command)
    if "--offline " not in cmd:
        raise SystemExit(f"not an offline command: {test_command}")
    return cmd.replace("--offline ", f"-I {INIT_PATH} {NETWORK} ", 1)


def ran_tests(work: Path) -> list[str]:
    """Reports of tests that ran in the working copy (there must be none)."""
    return sorted(str(p.relative_to(work)) for p in work.rglob("build/test-results/**/TEST-*.xml"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--go", action="store_true", help="run it (needs the network and somebody to answer a firewall)")
    ap.add_argument("--dry-run", action="store_true", help="print every command and host, start nothing (the default)")
    ap.add_argument("--all", action="store_true", help="every task, also those with a valid precheck")
    args = ap.parse_args()
    if args.go and args.dry_run:
        ap.error("--go or --dry-run, not both")
    repos = config.load_repos()
    todo = [t for t in config.load_tasks() if args.all or not valid(t, repos[t["repo"]])]
    todo.sort(key=lambda t: repos[t["repo"]]["gradle"])
    if not todo:
        print("every task has a valid precheck; nothing to do")
        return 0
    env = dict(config.repo_env(repos[todo[0]["repo"]]))
    java = Path(env["JAVA_HOME"]) / "bin" / "java"
    if not java.is_file() or isolation.IPV4.encode() in java.read_bytes()[:4096]:
        raise SystemExit(f"{java} is missing or is the IPv4 copy, which is for offline builds only: use the original JDK")
    # The sessions' GRADLE_OPTS without the closed proxy; one build at a time, four workers, no daemon.
    env["GRADLE_OPTS"] = " ".join(o for o in env.get("GRADLE_OPTS", "").split() if "proxy" not in o.lower())
    print(f"{len(todo)} builds, one after the other, about {len(todo) * MINUTES[0]}-{len(todo) * MINUTES[1]} minutes in all; "
          "resolve and compile only, stops at the first error")
    print(f"program that connects: {java} (working directory {WORK}, one path for all builds)")
    print("hosts (informative: nothing here enforces them, the machine's firewall does):")
    for h, why in HOSTS.items():
        print(f"  {h:<32} {why}")
    print(f"environment: GRADLE_OPTS={env['GRADLE_OPTS']}")
    print(f"init script {INIT_PATH}:\n" + "".join(f"  | {x}\n" for x in INIT.splitlines()), end="")
    problems = []
    for t in todo:
        problem = editgrade.wrapper_problem(config.cache_dir() / "repos" / t["repo"], env)
        problems += [problem] if problem else []
        print(f"  {repos[t['repo']]['gradle']:<6} {t['id']}\n         {online_command(t['test_command'])}" + (f"\n         PROBLEM: {problem}" if problem else ""))
    if not args.go:
        print("nothing started; run with --go")
        return 0
    if problems:
        print("STOPPED before the first build: " + "; ".join(problems))
        return 1
    logs = ROOT / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    INIT_PATH.write_text(INIT)
    for n, t in enumerate(todo, 1):
        if WORK.exists():
            config.remove_tree(WORK, ROOT)
        config.copy_tree(config.cache_dir() / "repos" / t["repo"], WORK)
        editgrade.apply_gold(WORK, config.gold_dir(t["repo"]), t)  # the commit's tree: its tests' class path resolves
        if problem := editgrade.wrapper_problem(WORK, env):  # the replayed commit itself may name another distribution
            print(f"STOPPED at [{n}/{len(todo)}] {t['id']}: {problem}")
            return 1
        started = time.time()
        res = editgrade.run_test(online_command(t["test_command"]), WORK, TIMEOUT_S, env)
        log = logs / f"{t['id']}.log"
        log.write_text(res["output"])
        ran = ran_tests(WORK)
        if ran or not res["passed"]:
            why = (f"tests ran although none may: {ran[:3]}" if ran else f"timed out after {TIMEOUT_S} s" if res["timed_out"]
                   else next((x.strip() for x in res["output"].split("What went wrong:", 1)[-1].splitlines() if x.strip()), "no output"))
            print(f"STOPPED at [{n}/{len(todo)}] {t['id']} ({time.time() - started:.0f} s): {why}\n  log: {log}\n"
                  "  Nothing else was started. If a connection was refused or not answered in time, allow it and run the same "
                  "command again: what is downloaded stays, builds that finished are quick.")
            return 1
        print(f"[{n}/{len(todo)}] {t['id']}: ok ({time.time() - started:.0f} s)", flush=True)
    config.remove_tree(WORK, ROOT)
    size = subprocess.run(["du", "-sh", str(Path.home() / ".gradle")], capture_output=True, text=True).stdout.split("\t")[0]
    print(f"~/.gradle is {size}. Next, offline and with the JDK copy: agent_eval.py gradle-cache, precheck-edit --gold-runs 2 --sandbox")
    return 0


if __name__ == "__main__":
    sys.exit(main())
