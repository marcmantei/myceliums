"""Watch a run of the commit replay from outside (#183): load, disk and the sockets of the JDK copy.

    python3 -I watch_run.py PID LOG RUN_DIR [--jdk ~/.cache/myc-java/jdk-ipv4]

Passive: it kills nothing and ends with process PID. Every 10 s a line with the
load and the free disk, every second the sockets of the processes started from
the JDK copy (``ps``, ``lsof``; no environment is read). Loopback connections and
listeners are logged once per kind; a listener on any other address and a
remote connection are logged **each, with process id and port**. Below 31 GB
free, and on a remote connection, it creates ``RUN_DIR/STOP``: the running
session ends and is recorded, no new one starts.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

LOOPBACK = ("127.", "[::1]", "localhost", "[::ffff:127.")
MIN_FREE_GB = 31


def classify(line: str) -> tuple[str, str, str, str] | None:
    """``(kind, protocol, pid, address)`` of a line of ``lsof -nP -i``: ``LOOP``
    and ``LISTEN`` (loopback; pid and address left empty, one entry per
    protocol), ``REMOTE`` and ``LISTEN-ALL`` (an address other than loopback). A
    TCP socket that is neither connected nor listening (``*:*``) is no listener:
    ``UNBOUND``."""
    f = line.split()
    if len(f) < 9 or not f[1].isdigit():
        return None
    pid, proto, name = f[1], f[7], f[8]
    if "->" in name:
        remote = name.split("->")[1]
        return ("LOOP", proto, "", "") if remote.startswith(LOOPBACK) else ("REMOTE", proto, pid, remote)
    if proto == "TCP" and not line.rstrip().endswith("(LISTEN)"):
        return "UNBOUND", proto, pid, name
    if proto == "UDP" and name in ("*:*",):
        return "LISTEN", proto, "", ""
    return ("LISTEN", proto, "", "") if name.startswith(LOOPBACK) else ("LISTEN-ALL", proto, pid, name)


def sockets(jdk: str) -> list[tuple[str, str, str, str]]:
    ps = subprocess.run(["/bin/ps", "-Ao", "pid=,comm="], capture_output=True, text=True).stdout
    pids = [line.split()[0] for line in ps.splitlines() if jdk in line]
    if not pids:
        return []
    out = subprocess.run(["/usr/sbin/lsof", "-nP", "-i", "-a", "-p", ",".join(pids)], capture_output=True, text=True).stdout
    return [c for c in map(classify, out.splitlines()) if c]


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("pid", type=int)
    p.add_argument("log")
    p.add_argument("run_dir", type=Path)
    p.add_argument("--jdk", default=str(Path.home() / ".cache/myc-java/jdk-ipv4"))
    a = p.parse_args()
    jdk, seen, last, n, stop = os.path.expanduser(a.jdk).rstrip("/") + "/", set(), 0.0, 0, False
    with open(a.log, "a", buffering=1) as log:
        def say(*words):
            print(time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), *words, file=log)
        while True:
            try:
                os.kill(a.pid, 0)
            except OSError:
                break
            n, now = n + 1, time.time()
            if now - last >= 10:
                last, free = now, shutil.disk_usage(Path.home()).free / 2**30
                say("load", *(f"{x:.1f}" for x in os.getloadavg()), "free", f"{free:.0f}")
                if free < MIN_FREE_GB and not stop:
                    stop = True
                    say("STOP disk")
            for s in sockets(jdk):
                if s not in seen:
                    seen.add(s)
                    say(*s)
                if s[0] == "REMOTE" and not stop:
                    stop = True
                    say("STOP remote connection")
            if stop and a.run_dir.is_dir():
                (a.run_dir / "STOP").touch()
            time.sleep(1)
        say("ended after", n, "samples")
    return 0


if __name__ == "__main__":
    sys.exit(main())
