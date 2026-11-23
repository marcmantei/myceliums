"""One online warm-up of the Gradle cache for the commit-replay tasks (#183).

    JAVA_HOME=<JDK 25> python3 -I warm_gradle.py            # list what would run, the hosts and the estimate; starts nothing
    JAVA_HOME=<JDK 25> python3 -I warm_gradle.py --go       # run it, once, in one go

The only step that needs the network. For every task without a valid offline
precheck it puts the replayed commit's tree at ONE fixed path and runs the
task's own command there without ``--offline``: the same wrapper jar path and
the same working directory for every build, so that a firewall asks once per
host. No build scan (``--no-scan``) and no build cache, local or remote
(``--no-build-cache``; the remote cache's address is a closed port). The
wrapper itself never downloads: a build whose distribution is not complete on
disk is skipped and named. Nothing else on the machine is changed besides
``~/.gradle`` and the fixed path.

Afterwards: ``agent_eval.py precheck-edit --gold-runs 2`` (offline).
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path

EVAL = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(EVAL))
TASKS = EVAL / "tasks" / "tasks-commit-replay.json"
os.environ["AEVAL_TASKS"] = str(TASKS)

from aeval import config, editgrade  # noqa: E402

ROOT = Path("/private/tmp/aeval-warm")
WORK = ROOT / "spring-framework"
HOSTS = {
    "repo.maven.apache.org": "Maven Central: libraries",
    "repo.spring.io": "Spring milestones and snapshots (only where a build uses them)",
    "plugins.gradle.org": "Gradle plugin portal (redirects to plugins-artifacts.gradle.org)",
    "plugins-artifacts.gradle.org": "the portal's files",
    "nodejs.org": "release list read by the docs module's node plugin (seen as a notice in the offline logs)",
}
MINUTES = (2, 6)  # per build; first builds online took 2-9 minutes on 23 November beside a busy CPU


def valid(task: dict, repo: dict) -> bool:
    p = task.get("precheck") or {}
    return bool(p.get("fails_on_parent") and p.get("passes_on_gold") and p.get("command") == task["test_command"]
                and p.get("commit") == repo["commit"])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--go", action="store_true", help="run it (needs the network and somebody to answer a firewall)")
    ap.add_argument("--all", action="store_true", help="every task, also those with a valid precheck")
    args = ap.parse_args()
    repos = config.load_repos()
    todo = [t for t in config.load_tasks() if args.all or not valid(t, repos[t["repo"]])]
    todo.sort(key=lambda t: repos[t["repo"]]["gradle"])
    env = {k: v for k, v in config.repo_env(repos[todo[0]["repo"]]).items()} if todo else {}
    # The sessions' GRADLE_OPTS without the closed proxy; one build at a time, four workers, no daemon.
    env["GRADLE_OPTS"] = " ".join(o for o in env.get("GRADLE_OPTS", "").split() if "proxy" not in o.lower())
    print(f"{len(todo)} builds, one after the other, about {len(todo) * MINUTES[0]}-{len(todo) * MINUTES[1]} minutes in all")
    print(f"java: {env.get('JAVA_HOME')}/bin/java; wrapper: {WORK}/gradle/wrapper/gradle-wrapper.jar (one path for all)")
    print("hosts:")
    for h, why in HOSTS.items():
        print(f"  {h:<32} {why}")
    for t in todo:
        problem = editgrade.wrapper_problem(config.cache_dir() / "repos" / t["repo"], env)
        print(f"  {repos[t['repo']]['gradle']:<6} {t['id']}" + ("   SKIPPED: " + problem if problem else ""))
    if not args.go:
        print("nothing started; run with --go")
        return 0
    logs = ROOT / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    failed = []
    for n, t in enumerate(todo, 1):
        checkout = config.cache_dir() / "repos" / t["repo"]
        if editgrade.wrapper_problem(checkout, env):
            failed.append(t["id"] + " (distribution missing)")
            continue
        if WORK.exists():
            config.remove_tree(WORK, ROOT)
        config.copy_tree(checkout, WORK)
        editgrade.apply_gold(WORK, config.gold_dir(t["repo"]), t)  # the commit: everything compiles and the tests resolve
        command = t["test_command"].replace("--offline ", "", 1) + " --continue"
        started = time.time()
        res = editgrade.run_test(command, WORK, 1800, env)
        (logs / f"{t['id']}.log").write_text(res["output"])
        print(f"[{n}/{len(todo)}] {t['id']}: {'ok' if res['passed'] else 'FAILED, see ' + str(logs / (t['id'] + '.log'))} "
              f"({time.time() - started:.0f} s)", flush=True)
        if not res["passed"]:
            failed.append(t["id"])
    if WORK.exists():
        config.remove_tree(WORK, ROOT)
    size = subprocess.run(["du", "-sh", str(Path.home() / ".gradle")], capture_output=True, text=True).stdout.split("\t")[0]
    print(f"~/.gradle is {size}; failed: {failed or 'none'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
