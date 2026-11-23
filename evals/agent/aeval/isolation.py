"""Isolated sessions (#183): what a session may reach, and what it tried.

A commit-replay session must not find the replayed commit: the gold files,
later parents, the history clone, the task file and other sessions'
transcripts lie on the same disk, and the commit is public. Three layers, the
same in every arm:

1. the PreToolUse hook of :mod:`aeval.bashcmd` (``isolated-hook``): every tool
   call stays in the working copy, no network program, no interpreter, one
   Gradle rule;
2. on macOS the whole session (and the test command that grades it) runs under
   ``sandbox-exec`` with :func:`profile`: the kernel refuses the data of the
   protected places to every child process, also to a test the agent wrote and
   Gradle runs, which no hook can see;
3. :func:`audit` reads the transcript afterwards with its own, simpler tests
   and counts attempts, refusals and what got through.

Limits: the network is refused by the hook only (the session needs the model's
API, so the kernel cannot deny it), so code the agent writes and runs through
Gradle could open a connection; the harness points the build's HTTP(S) proxy
at a closed port. The model may remember the public commit. This guards a
cooperative agent.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

from . import config
from .bashcmd import ISOLATED_MARKER, NETWORK_PROGRAMS, programs

SANDBOX = "/usr/bin/sandbox-exec"
URL = re.compile(r"https?://(?!(?:localhost|127\.0\.0\.1)\b)[\w.-]+")
KINDS = ("outside", "network", "gradle")


def protected_roots() -> list[Path]:
    """Places no session may read: the harness's cache, this repository and
    its siblings, the history, other sessions; ``AEVAL_PROTECT`` (paths
    separated by ``:``) adds some."""
    home = Path.home()
    repo = config.EVAL_DIR.parent.parent
    roots = [config.cache_dir(), repo, repo.parent, home / ".cache" / "myc-java" / "spring-history",
             home / ".cache" / "myc-java" / "spring-framework", home / ".cache" / "myceliums-eval-repos",
             home / ".claude" / "projects", home / ".local" / "share" / "myc-orch"]
    roots += [Path(p) for p in os.environ.get("AEVAL_PROTECT", "").split(":") if p]
    return list(dict.fromkeys(Path(os.path.realpath(r)) for r in roots))


def project_dir(workdir: Path) -> Path:
    """Where Claude Code keeps this working directory's own session files."""
    return Path.home() / ".claude" / "projects" / re.sub(r"[^A-Za-z0-9]", "-", os.path.realpath(workdir))


def _q(path) -> str:
    return '"' + os.path.realpath(path).replace("\\", "\\\\").replace('"', '\\"') + '"'


def profile(allow: list[Path], roots: list[Path] | None = None) -> str:
    """Seatbelt profile: everything, except the data of the protected places
    (names and times stay visible); ``allow`` is re-allowed (the last rule
    that matches wins)."""
    deny = " ".join(f"(subpath {_q(r)})" for r in (protected_roots() if roots is None else roots))
    ok = " ".join(f"(subpath {_q(a)})" for a in allow)
    return ("(version 1)\n(allow default)\n"
            f"(deny file-read-data file-read-xattr file-write* {deny})\n"
            + (f"(allow file-read-data file-read-xattr file-write* {ok})\n" if ok else ""))


def sandboxed(cmd: list[str], profile_path: Path) -> list[str]:
    return [SANDBOX, "-f", str(profile_path), *cmd]


def available() -> bool:
    return sys.platform == "darwin" and os.access(SANDBOX, os.X_OK)


def audit(path: str, root: str) -> dict:
    """Attempts to leave the working copy or reach the network, per kind:
    ``attempts``, ``blocked`` (the hook's refusal is the result) and ``ran``;
    plus protected places named in results or edits. Independent of the hook's
    parser where possible: plain text tests over every tool input."""
    root = os.path.realpath(root)
    own = str(project_dir(Path(root)))
    places = [str(p) for p in protected_roots()]
    out = {f"{k}_{w}": 0 for k in KINDS for w in ("attempts", "blocked", "ran")}
    out.update(refused_calls=0, protected_in_results=0, edits_naming_protected=0, tool_calls=0)
    waiting: dict[str, set[str]] = {}
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            content = (ev.get("message") or {}).get("content") if ev.get("type") in ("assistant", "user") else None
            for b in content if isinstance(content, list) else []:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "tool_use":
                    name, inp = b.get("name") or "", b.get("input") or {}
                    out["tool_calls"] += 1
                    kinds = _kinds(name, inp, root, own)
                    for k in kinds:
                        out[k + "_attempts"] += 1
                    waiting[b.get("id")] = kinds
                    if name in ("Edit", "Write"):
                        body = json.dumps({k: v for k, v in inp.items() if k != "file_path"})
                        out["edits_naming_protected"] += any(p in body for p in places)
                elif b.get("type") == "tool_result":
                    text = b.get("content")
                    text = text if isinstance(text, str) else json.dumps(text)
                    refused = ISOLATED_MARKER in text
                    out["refused_calls"] += refused
                    out["protected_in_results"] += not refused and any(p in text.replace(own, "") for p in places)
                    for k in waiting.pop(b.get("tool_use_id"), ()):
                        out[k + ("_blocked" if refused else "_ran")] += 1
    out["flagged"] = bool(any(out[k + "_ran"] for k in KINDS) or out["protected_in_results"]
                          or out["edits_naming_protected"])
    return out


def _kinds(name: str, inp: dict, root: str, own: str) -> set[str]:
    kinds: set[str] = set()
    texts: list[str] = []
    if name in ("WebFetch", "WebSearch"):
        kinds.add("network")
    elif name == "Bash":
        cmd = str(inp.get("command") or "")
        progs = programs(cmd)
        if any(p in NETWORK_PROGRAMS and p != "gradle" for p, _ in progs):
            kinds.add("network")
        for p, a in progs:
            if p in ("gradlew", "gradle") and not {"--offline", "--no-scan", "--no-build-cache"} <= set(a):
                kinds.add("gradle")
        patterns = [a for p, args in progs if p in ("grep", "egrep", "fgrep", "rg", "git grep") for a in args
                    if not a.startswith("-")][:1]
        rest = cmd
        for p in patterns:  # a search pattern is not a path or an address
            rest = rest.replace(p, " ")
        if URL.search(rest):
            kinds.add("network")
        texts = re.findall(r"(?:^|[\s=:'\"<>(])((?:/|~/?|\.\./)[^\s'\";|&<>)]*)", URL.sub(" ", rest))
    elif name in ("Read", "Edit", "Write"):
        texts = [str(inp.get("file_path") or "")]
    elif name in ("Grep", "Glob"):
        texts = [str(inp.get("path") or ""), str(inp.get("glob" if name == "Grep" else "pattern") or "")]
    elif name.startswith("mcp__"):
        texts = [v for v in inp.values() if isinstance(v, str) and v.startswith(("/", "~", ".."))]
    for t in texts:
        if t in ("/dev/null", "/dev/stdout", "/dev/stderr"):
            continue
        t = os.path.expanduser(t)
        if t.startswith("/") or ".." in t.split("/"):
            full = os.path.realpath(os.path.join(root, t))
            if not (full == root or full.startswith(root + "/") or full.startswith(own + "/")):
                kinds.add("outside")
    return kinds


def summarise(rows: list[dict]) -> dict:
    """Totals per condition over rows that carry ``isolation_audit``."""
    out: dict[str, dict] = {}
    for r in rows:
        a = r.get("isolation_audit")
        if not a:
            continue
        s = out.setdefault(r["condition"], {"sessions": 0, "flagged": 0})
        s["sessions"] += 1
        for k, v in a.items():
            s[k] = s.get(k, 0) + (bool(v) if k == "flagged" else v)
    return out


def report_lines(summary: dict) -> list[str]:
    if not summary:
        return []
    lines = ["## Isolation audit", "",
             "Per condition, over its sessions (#183): tool calls that named a place outside the working copy, the "
             "network, or Gradle without the required flags (refused by the hook / ran), protected places named in "
             "results or edits, and flagged sessions (anything that ran or appeared). A flag is a hole in the "
             "isolation.", "",
             "| Condition | Sessions | Outside: refused / ran | Network: refused / ran | Gradle: refused / ran | "
             "Protected in results | In edits | Flagged |", "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for c, a in sorted(summary.items()):
        lines.append(f"| `{c}` | {a['sessions']} | {a['outside_blocked']} / {a['outside_ran']} | "
                     f"{a['network_blocked']} / {a['network_ran']} | {a['gradle_blocked']} / {a['gradle_ran']} | "
                     f"{a['protected_in_results']} | {a['edits_naming_protected']} | {a['flagged']} |")
    return lines + [""]
