"""Isolated sessions (#183): what a session may reach, and what it tried.

A commit-replay session must not find the replayed commit: the gold files,
later parents, the history clone, the task file and other sessions'
transcripts lie on the same disk, and the commit is public. Three layers, the
same in every arm:

1. on macOS the whole session (and the test command that grades it) runs under
   ``sandbox-exec`` with :func:`profile`: under the home directory and
   ``/private/tmp`` the kernel lets it read only what it needs
   (:func:`readable`), whatever the shell command looks like, also from a test
   the agent wrote and Gradle runs;
2. the PreToolUse hook of :mod:`aeval.bashcmd` (``isolated-hook``): tool calls
   stay in the working copy, no network program, no interpreter, one Gradle
   rule; a command its parser cannot fully account for is refused;
3. :func:`audit` scans the transcript afterwards with raw-text tests that share
   no code with the hook, and reports where the two disagree.

Threat model: a cooperative agent that has been told to work in its checkout,
audited per session. Not covered: the network is refused by the hook only (the
session needs the model's API, so the kernel cannot deny it), so code the
agent writes and runs through Gradle could open a connection (the build's
HTTP(S) proxy points at a closed port); ``/private/var/folders`` (the user's
temporary directory, which Java uses) and system directories stay readable,
and names and times of every file. The CLI's login is in the keychain, whose files
stay readable (encrypted; the hook refuses the `security` program), unless the
caller passes `CLAUDE_CODE_OAUTH_TOKEN`, which every process of the session
then inherits. Spring's commits are public and may be in
the model's training data.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

from . import config

SANDBOX = "/usr/bin/sandbox-exec"
KINDS = ("outside", "network", "gradle")
# The refusal text of the hook (aeval/bashcmd.py ISOLATED_MARKER, a test keeps them equal).
MARKER = "Not allowed in this session:"


def protected_roots() -> list[Path]:
    """Where nothing may be read unless :func:`readable` names it; ``AEVAL_PROTECT``
    (paths separated by ``:``) adds some."""
    roots = [Path.home(), Path("/private/tmp"), config.cache_dir(), config.EVAL_DIR.parent.parent]
    roots += [Path(p) for p in os.environ.get("AEVAL_PROTECT", "").split(":") if p]
    return list(dict.fromkeys(Path(os.path.realpath(r)) for r in roots))


def gradle_home(env: dict) -> Path:
    return Path(env.get("GRADLE_USER_HOME") or Path.home() / ".gradle")


def readable(tmp: Path, workdir: Path, claude: str, env: dict, myc: list[Path]) -> tuple[list[Path], list[Path]]:
    """``(read and write, read only)`` of one session: its working copy and its
    own temporary directory (index copy, hook, configuration and temporary
    files of this session's Claude Code), Gradle's home; the CLI's binary, the
    JDK, and in the arms with myceliums its binary and embedding models."""
    rw = [workdir, tmp, gradle_home(env)]
    ro = [Path(claude)]
    if not (os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("CLAUDE_CODE_OAUTH_TOKEN")):
        ro.append(Path.home() / "Library" / "Keychains")  # the CLI's login; its credentials file is not needed
    ro += [Path(env[k]) for k in ("JAVA_HOME",) if env.get(k)] + list(myc)
    return rw, ro


def _q(path) -> str:
    return '"' + os.path.realpath(path).replace("\\", "\\\\").replace('"', '\\"') + '"'


def profile(rw: list[Path], ro: list[Path] = (), roots: list[Path] | None = None,
            never: list[Path] = (), never_read: list[str] = ()) -> str:
    """Seatbelt profile, last matching rule wins: nothing of the protected
    roots can be read or written (names and times stay visible), except
    ``rw`` and ``ro``; ``never`` stays closed inside them (Gradle's build
    cache and stored scans), ``never_read`` are regexes of files that may be
    written but not read (Gradle daemon logs, which hold other builds'
    output)."""
    sub = lambda paths: " ".join(f"(subpath {_q(p)})" for p in paths)  # noqa: E731
    out = ["(version 1)", "(allow default)",
           f"(deny file-read-data file-read-xattr file-write* {sub(protected_roots() if roots is None else roots)})"]
    if ro:
        out.append(f"(allow file-read-data file-read-xattr {sub(ro)})")
    if rw:
        out.append(f"(allow file-read-data file-read-xattr file-write* {sub(rw)})")
    if never:
        out.append(f"(deny file-read-data file-read-xattr file-write* {sub(never)})")
    out += [f'(deny file-read-data (regex #"{rx}"))' for rx in never_read]
    return "\n".join(out) + "\n"


def session_profile(tmp: Path, workdir: Path, hooks: Path, claude: str, env: dict, myc: list[Path]) -> str:
    rw, ro = readable(tmp, workdir, claude, env, myc)
    g = Path(os.path.realpath(gradle_home(env)))
    return profile(rw + [hooks], ro, never=[g / "caches" / "build-cache-1", g / "build-scan-data"],
                   never_read=["^" + re.escape(str(g)) + r"/daemon/.*[.]out[.]log$"])


def session_environment(tmp: Path) -> dict[str, str]:
    """A Claude Code configuration and temporary directory of this session
    alone; the login stays the user's (read from its secure storage)."""
    for d in ("claude-config", "claude-tmp", "zdotdir", "t"):
        (tmp / d).mkdir(exist_ok=True)
    return {"CLAUDE_CONFIG_DIR": str(tmp / "claude-config"), "CLAUDE_CODE_TMPDIR": str(tmp / "claude-tmp"),
            "TMPDIR": str(tmp / "t"), "ZDOTDIR": str(tmp / "zdotdir"),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR": os.environ.get("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            or os.environ.get("CLAUDE_CONFIG_DIR") or str(Path.home() / ".claude")}


def sandboxed(cmd: list[str], profile_path: Path) -> list[str]:
    return [SANDBOX, "-f", str(profile_path), *cmd]


def available() -> bool:
    return sys.platform == "darwin" and os.access(SANDBOX, os.X_OK)


# Raw text, on purpose without the hook's parser: program names, addresses, paths.
_NET = re.compile(r"(?<![\w./-])(?:/usr/bin/|/bin/|/usr/local/bin/|/opt/homebrew/bin/)?"
                  r"(curl|wget|nc|ncat|netcat|telnet|ssh|scp|sftp|rsync|ftp|openssl|dig|nslookup|ping|gh|npm|npx|pip3?|"
                  r"brew|mvn|docker|jshell|python[\d.]*|node|ruby|perl|php|osascript|git\s+(?:fetch|pull|clone|push|"
                  r"remote|ls-remote|submodule|archive))(?![\w.-])")
_KEYCHAIN = re.compile(r"find-(?:generic|internet)-password|dump-keychain|(?<![\w.-])/usr/bin/security\b")
_ADDRESS = re.compile(r"(?:https?|ftp|ssh|git)://(?!(?:localhost|127\.0\.0\.1)\b)[\w.-]+|\b(?:[\w-]+\.)*(?:github|"
                      r"githubusercontent|gitlab|gradle|maven|sonatype|spring)\.(?:com|org|io)\b(?!\.)")
_PATH = re.compile(r"(?<![\w.*)\]}$])(/[A-Za-z][\w.@+-]*(?:/[^\s'\";|&<>(){}*?]*)?|~(?:/[^\s'\";|&<>()]*)?)")
_UP = re.compile(r"(?<![\w.])\.\.(?=/|$|[\s'\";|&)}\],])")
_GRADLE = re.compile(r"(?<![\w-])(?:\./)?gradlew?(?![\w.-])([^|;&\n]*)")


def scan(text: str, root: str) -> set[str]:
    """The kinds of reach a piece of raw text mentions."""
    kinds = set()
    if _NET.search(text) or _ADDRESS.search(text):
        kinds.add("network")
    if _UP.search(text) or _KEYCHAIN.search(text):
        kinds.add("outside")
    for m in _PATH.finditer(text):
        p = os.path.realpath(os.path.expanduser(m.group(1)))
        if m.group(1).startswith("/dev/") or p == root or p.startswith(root + "/"):
            continue
        # `/users/{id}` in a search pattern is no path of this machine: it counts when its first part exists.
        if m.group(1).startswith("~") or os.path.lexists("/" + m.group(1).split("/")[1]):
            kinds.add("outside")
    for m in _GRADLE.finditer(text):
        if not all(f in m.group(1) for f in ("--offline", "--no-scan", "--no-build-cache")):
            kinds.add("gradle")
    return kinds


def audit(path: str, root: str) -> dict:
    """Attempts to leave the working copy or reach the network, per kind:
    ``attempts``, ``blocked`` (the hook refused) and ``ran``. A call that
    this scan suspects and the hook let run is a *disagreement*: kept, for
    review, because the scan also meets search patterns. Also: calls the hook
    refused, refusals by the kernel, and protected places that show up in
    results or edits."""
    root = os.path.realpath(root)
    places = [str(p) for p in (config.cache_dir(), config.EVAL_DIR.parent.parent, Path.home() / "SYNC",
                               Path.home() / ".claude", Path.home() / ".cache" / "myc-java" / "spring-history")]
    own = re.compile(r"[^\s\"']*/aeval-run-[^/\s\"']*")  # this session's own directory, which its tools may name
    out: dict = {f"{k}_{w}": 0 for k in KINDS for w in ("attempts", "blocked", "ran")}
    out.update(refused_calls=0, kernel_refusals=0, protected_in_results=0, edits_naming_protected=0, tool_calls=0,
               disagreements=0, to_review=[])
    waiting: dict[str, tuple[set[str], str]] = {}
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
                    kinds = _kinds(name, inp, root)
                    for k in kinds:
                        out[k + "_attempts"] += 1
                    waiting[b.get("id")] = (kinds, f"{name}: {json.dumps(inp, ensure_ascii=False)[:200]}")
                    if name in ("Edit", "Write"):
                        body = json.dumps({k: v for k, v in inp.items() if k != "file_path"})
                        out["edits_naming_protected"] += any(p in body for p in places)
                elif b.get("type") == "tool_result":
                    text = b.get("content")
                    text = text if isinstance(text, str) else json.dumps(text)
                    refused = MARKER in text
                    out["refused_calls"] += refused
                    out["kernel_refusals"] += not refused and "Operation not permitted" in text
                    out["protected_in_results"] += not refused and any(p in own.sub("", text) for p in places)
                    kinds, shown = waiting.pop(b.get("tool_use_id"), (set(), ""))
                    for k in kinds:
                        out[k + ("_blocked" if refused else "_ran")] += 1
                    if kinds and not refused:
                        out["disagreements"] += 1
                        if len(out["to_review"]) < 20:
                            out["to_review"].append(f"{'+'.join(sorted(kinds))} {shown}")
    out["flagged"] = bool(out["disagreements"] or out["kernel_refusals"] or out["protected_in_results"]
                          or out["edits_naming_protected"])
    return out


def _kinds(name: str, inp: dict, root: str) -> set[str]:
    if name in ("WebFetch", "WebSearch"):
        return {"network"}
    if name == "Bash":
        return scan(str(inp.get("command") or ""), root)
    if name in ("Read", "Edit", "Write"):
        texts = [str(inp.get("file_path") or "")]
    elif name in ("Grep", "Glob"):
        texts = [str(inp.get("path") or ""), str(inp.get("glob" if name == "Grep" else "pattern") or "")]
    elif name.startswith("mcp__"):
        texts = [v for v in inp.values() if isinstance(v, str) and v.startswith(("/", "~", ".."))]
    else:
        return set()
    kinds = set()
    for t in texts:
        if "/claude-config/projects/" in t and t.rstrip("/").split("/")[-2:-1] == ["tool-results"] and "/../" not in t:
            continue  # a saved result of this session
        full = os.path.realpath(os.path.join(root, os.path.expanduser(t)))
        if t and not (full == root or full.startswith(root + "/")) or _UP.search(t):
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
            if k != "to_review":
                s[k] = s.get(k, 0) + (bool(v) if k == "flagged" else v)
    return out


def report_lines(summary: dict) -> list[str]:
    if not summary:
        return []
    lines = ["## Isolation audit", "",
             "Per condition, over its sessions (#183): tool calls the hook refused; calls that a raw-text scan of the "
             "transcript found naming a place outside the working copy, the network or Gradle without the required "
             "flags (refused / ran); refusals by the kernel; protected places in results or edits. A call the scan "
             "suspects and the hook let run is a disagreement: the scan also meets search patterns, so each is listed "
             "in the session's row (`to_review`) and has to be read. Flagged: a session with a disagreement, a kernel "
             "refusal, or a protected place in a result or an edit.", "",
             "| Condition | Sessions | Refused calls | Outside: refused / ran | Network: refused / ran | "
             "Gradle: refused / ran | Disagreements | Kernel refusals | Protected in results | In edits | Flagged |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for c, a in sorted(summary.items()):
        lines.append(f"| `{c}` | {a['sessions']} | {a['refused_calls']} | {a['outside_blocked']} / {a['outside_ran']} | "
                     f"{a['network_blocked']} / {a['network_ran']} | {a['gradle_blocked']} / {a['gradle_ran']} | "
                     f"{a['disagreements']} | {a['kernel_refusals']} | {a['protected_in_results']} | "
                     f"{a['edits_naming_protected']} | {a['flagged']} |")
    return lines + [""]
