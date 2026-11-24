"""Isolated sessions (#183): what a session may reach, and what it tried.

A commit-replay session must not find the replayed commit: the gold files,
later parents, the history clone, the task file and other sessions'
transcripts lie on the same disk, and the commit is public. The ``claude``
process itself runs as in daily use (its login, token refresh and API calls
are not touched); **what the agent executes** is confined, the same in every
arm (:func:`session_settings`, passed with ``--settings``, which a repository's
files cannot loosen):

1. **Bash**: Claude Code's own sandbox (Seatbelt). Under the home directory,
   ``/private/tmp``, the temporary directories, ``/Volumes`` and
   ``/Users/Shared`` only what a session needs is readable (:func:`readable`),
   the network is closed except loopback, nothing runs unsandboxed
   (``allowUnsandboxedCommands: false``, no excluded commands), and the
   session does not start without it (``failIfUnavailable``). It holds for
   every child process, also for a test the agent wrote and Gradle runs.
2. **Read, Grep, Glob, Edit, Write** run inside ``claude``: they are confined by
   ``blockReads/WritesOutsideWorkingDirectories``, deny rules and
3. the **hook** of :mod:`aeval.bashcmd` (``isolated-hook``) on every tool call
   (myceliums calls included), on canonical paths; a command its parser cannot
   account for is refused.
4. The **test command that grades** a session runs under :func:`grading_profile`
   (``sandbox-exec``): the same files, no network but loopback, no keychain and
   no security services.
5. :func:`audit` scans the transcript afterwards with raw-text tests that share
   no code with the hook, and reports where the two disagree.

Threat model: a cooperative agent that has been told to work in its checkout,
audited per session. Not covered: Claude Code's sandbox cannot close macOS's
security services (lookups of ``com.apple.SecurityServer`` and
``com.apple.securityd.xpc`` succeed in 2.1.331; its settings only add services),
the keychain files are closed;  ``claude``, the myceliums server and the
hooks run with the user's access; the names of files outside may stay visible;
Gradle's home is shared and writable, so a session could leave something for a
later one (not init scripts, settings or the wrapper: those are closed); Spring's
commits are public and may be in the model's training data.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

from . import config

SANDBOX = "/usr/bin/sandbox-exec"
KINDS = ("outside", "network", "gradle")
# The refusal text of the hook (aeval/bashcmd.py ISOLATED_MARKER, a test keeps them equal).
MARKER = "Not allowed in this session:"


# Closed to sandboxed commands and to grading unless readable() names it.
CLOSED = ("/private/tmp", "/tmp", "/private/var/folders", "/var/folders", "/Users/Shared", "/Volumes", "/.vol",
          "/System/Volumes/Data/Users", "/System/Volumes/Data/private/tmp", "/private/var/db/mds")
# Closed even inside what is readable or re-opened by Claude Code.
NEVER = ("Library/Keychains", ".claude/projects", ".claude/history.jsonl", ".claude/file-history",
         ".claude/.credentials.json", ".claude/sessions", ".claude/paste-cache", ".claude/plans", ".claude.json",
         ".gitconfig", ".config", ".ssh", ".zsh_history")
SECRET_ENV = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "CLAUDE_CONFIG_DIR",
              "GH_TOKEN", "GITHUB_TOKEN")
SECURITY_SERVICES = ("com.apple.SecurityServer", "com.apple.securityd", "com.apple.securityd.xpc", "com.apple.secd",
                     "com.apple.security.agent", "com.apple.security.authhost", "com.apple.securityd.general",
                     "com.apple.ctkd.token-client")


def protected_roots() -> list[Path]:
    """Where nothing may be read unless :func:`readable` names it; ``AEVAL_PROTECT``
    (paths separated by ``:``) adds some."""
    roots = [Path.home(), *map(Path, CLOSED), config.cache_dir(), config.EVAL_DIR.parent.parent]
    roots += [Path(p) for p in os.environ.get("AEVAL_PROTECT", "").split(":") if p]
    real = list(dict.fromkeys(Path(os.path.realpath(r)) for r in roots))
    # Claude Code shows the model this list: whole regions, not the names of what lies in them.
    return [r for r in real if not any(o != r and o in r.parents for o in real)]


def gradle_home(env: dict) -> Path:
    return Path(os.path.realpath(env.get("GRADLE_USER_HOME") or Path.home() / ".gradle"))


def never(env: dict) -> list[str]:
    """Paths and patterns that stay closed inside readable places: secrets of
    the user's configuration, Gradle's build cache, scans stored by earlier
    builds and daemon logs (other builds' output)."""
    g = gradle_home(env)
    return ([str(Path.home() / n) for n in NEVER]
            + [f"/private/tmp/claude-{os.getuid()}", f"/tmp/claude-{os.getuid()}"]  # every session's command output
            + [str(g / "caches" / "build-cache-1"), str(g / "daemon" / "**" / "*.out.log")] + earlier_scans(env))


def unwritable(workdir: Path, env: dict) -> list[str]:
    """What a command may not change although it lies in a writable place:
    Claude Code's settings in the working copy, the Gradle wrapper, and what
    would carry code into a later session through Gradle's home."""
    w = Path(os.path.realpath(workdir))
    return ([str(w / n) for n in (".claude", ".mcp.json", "gradle/wrapper", "gradlew", "gradlew.bat", ".git/hooks", ".git/config")]
            + gradle_unwritable(env))


def earlier_scans(env: dict) -> list[str]:
    """Files of build scans stored by earlier builds (the plugin fails when it
    cannot create files in their directories, so only the payloads are closed)."""
    d = gradle_home(env) / "build-scan-data"
    return sorted(str(p) for p in d.rglob("*") if p.is_file()) if d.is_dir() else []


def gradle_unwritable(env: dict) -> list[str]:
    """What would carry code into a later session through Gradle's home: its
    init scripts and properties, and the unpacked distributions. The
    distribution's lock file (``.zip.lck``) must stay writable: the wrapper
    opens it even when the distribution is complete."""
    g = gradle_home(env)
    dists = g / "wrapper" / "dists"
    installed = sorted(p for d in dists.glob("*/*") for p in d.iterdir() if p.is_dir() or p.name.endswith((".zip", ".ok"))) \
        if dists.is_dir() else []
    return [str(g / n) for n in ("init.d", "init.gradle", "init.gradle.kts", "gradle.properties")] + [str(p) for p in installed]


def readable(tmp: Path, workdir: Path, env: dict, myc: list[Path]) -> tuple[list[Path], list[Path]]:
    """``(read and write, read only)`` for what a session executes: its working
    copy, its temporary files, Gradle's home; the JDK; in the arms with
    myceliums also the binary, the models and this session's index copy."""
    rw = [workdir, tmp / "t", gradle_home(env)] + ([tmp / "data", tmp / "home"] if myc else [])
    ro = [tmp / "g"] + [Path(env[k]) for k in ("JAVA_HOME",) if env.get(k)] + ([tmp / "bin"] if myc else []) + list(myc)
    real = lambda ps: [Path(os.path.realpath(p)) for p in ps]  # noqa: E731
    return real(rw), real(ro)


def session_settings(tmp: Path, workdir: Path, env: dict, myc: list[Path], hook: str | None) -> dict:
    """Settings of one isolated session; the arms differ in ``--search`` of the
    hook's command and in the myceliums paths."""
    rw, ro = readable(tmp, workdir, env, myc)
    closed = [str(r) for r in protected_roots()] + never(env)
    work = os.path.realpath(workdir)
    # Deny rules also close their paths to sandboxed commands, so not Gradle's home or the JDK's parent; the two
    # blocks below keep the file tools inside the working copy anyway.
    deny = [f"{t}(/{p}/**)" for t in ("Read", "Edit") for p in
            ("/Volumes", "/Users/Shared", str(Path.home() / "Library"), str(Path.home() / "SYNC"), str(Path.home() / ".ssh"))]
    deny += [f"Edit(/{work}/{n})" for n in (".claude/**", ".mcp.json", "gradle/wrapper/**", "gradlew", "gradlew.bat", ".git/**")]
    return {
        "sandbox": {
            "enabled": True, "failIfUnavailable": True, "allowUnsandboxedCommands": False, "autoAllowBashIfSandboxed": False,
            "excludedCommands": [], "enableWeakerNestedSandbox": False, "enableWeakerNetworkIsolation": False,
            "allowAppleEvents": False,
            "filesystem": {"denyRead": closed, "allowRead": [str(p) for p in rw + ro],
                           "allowWrite": [str(p) for p in rw if p != Path(work)], "denyWrite": unwritable(workdir, env)},
            # (CLAUDE_CODE_SUBPROCESS_ENV_SCRUB would also drop the pinned git configuration.)
            "credentials": {"envVars": [{"name": n, "mode": "deny"} for n in SECRET_ENV]},
            "network": {"allowedDomains": [], "deniedDomains": ["*"], "strictAllowlist": True, "allowLocalBinding": True,
                        "allowUnixSockets": [], "allowAllUnixSockets": False, "allowMachLookup": []},
        },
        "permissions": {"deny": deny, "blockReadsOutsideWorkingDirectories": True,
                        "blockWritesOutsideWorkingDirectories": True},
        **({"hooks": {"PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": hook}]}]}} if hook else {}),
    }


def _q(path) -> str:
    return '"' + os.path.realpath(path).replace("\\", "\\\\").replace('"', '\\"') + '"'


def profile(rw: list[Path], ro: list[Path] = (), roots: list[Path] | None = None, closed: list[str] = (),
            unwritable: list[str] = ()) -> str:
    """Seatbelt profile, last matching rule wins: nothing of the protected
    roots can be read or written except ``rw`` and ``ro``; ``closed`` (paths, or
    patterns with ``*``) stays closed inside them and ``unwritable`` read-only;
    no network but loopback, no security services."""
    sub = lambda paths: " ".join(f"(subpath {_q(p)})" for p in paths)  # noqa: E731
    out = ["(version 1)", "(allow default)",
           f"(deny file-read-data file-read-xattr file-write* {sub(protected_roots() if roots is None else roots)})"]
    if ro:
        out.append(f"(allow file-read-data file-read-xattr {sub(ro)})")
    if rw:
        out.append(f"(allow file-read-data file-read-xattr file-write* {sub(rw)})")
    for c in closed:
        if "*" in c:
            rx = re.escape(c).replace(r"\*\*", ".*").replace(r"\*", "[^/]*").replace('"', "")
            out.append(f'(deny file-read-data (regex #"^{rx}$"))')
        else:
            out.append(f"(deny file-read-data file-read-xattr file-write* (subpath {_q(c)}))")
    if unwritable:
        out.append(f"(deny file-write* {sub(unwritable)})")
    out += ["(deny network-outbound)", '(allow network-outbound (remote ip "localhost:*"))',
            "(deny mach-lookup " + " ".join(f'(global-name "{n}")' for n in SECURITY_SERVICES) + ")"]
    return "\n".join(out) + "\n"


def grading_profile(tmp: Path, workdir: Path, env: dict) -> str:
    """For the test command that grades a session (it runs code the agent
    wrote); no myceliums paths, the same in every arm."""
    rw, ro = readable(tmp, workdir, env, [])
    return profile(rw, ro, closed=never(env), unwritable=gradle_unwritable(env))


def sandboxed(cmd: list[str], profile_path: Path) -> list[str]:
    return [SANDBOX, "-f", str(profile_path), *cmd]


def available() -> bool:
    return sys.platform == "darwin" and os.access(SANDBOX, os.X_OK)


def session_environment(tmp: Path) -> dict[str, str]:
    """Temporary files of this session alone, also the JVM's (macOS's Java takes
    the user's directory whatever ``TMPDIR`` says), and no shell start files.
    Nothing about the login: ``claude`` finds it as always."""
    for d in ("t", "zdotdir", "g"):
        (tmp / d).mkdir(exist_ok=True)
    git = subprocess.run(["/usr/bin/xcrun", "-f", "git"], capture_output=True, text=True).stdout.strip() \
        if Path("/usr/bin/xcrun").exists() else ""
    if git and not (tmp / "g" / "git").exists():  # macOS's /usr/bin/git is a shim that looks around outside first
        (tmp / "g" / "git").symlink_to(git)
    return {"TMPDIR": str(tmp / "t"), "CLAUDE_CODE_TMPDIR": str(tmp / "t"),
            "ZDOTDIR": str(tmp / "zdotdir"),
            # IPv4 loopback: a JVM that opens IPv6-mapped sockets (its default) is refused by the sandboxes' loopback
            # rule. JAVA_TOOL_OPTIONS reaches a JVM that Gradle starts; the launcher variable also its worker JVMs.
            "JAVA_TOOL_OPTIONS": f"-Djava.io.tmpdir={tmp / 't'} -XX:-UsePerfData -Djava.net.preferIPv4Stack=true",
            "JDK_JAVA_OPTIONS": f"-Djava.io.tmpdir={tmp / 't'} -XX:-UsePerfData -Djava.net.preferIPv4Stack=true"}


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
    if name in ("Grep", "Glob"):  # `{/etc,a}/x`: every alternative is a path
        texts += [part for t in texts for part in re.split(r"[{},]", t) if part.startswith(("/", "~", "..")) and "{" in t]
    for t in texts:
        if "/.claude/projects/" in t and t.rstrip("/").split("/")[-2:-1] == ["tool-results"] and "/../" not in t:
            continue  # a saved result; the hook admits this session's own only
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
