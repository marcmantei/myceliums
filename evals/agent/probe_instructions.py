#!/usr/bin/env python3
"""Which project instructions reach Claude Code before and after ``myc setup-claude`` (#200).

Claude Code reads ``AGENTS.md`` only while no ``CLAUDE.md``, ``.claude/CLAUDE.md``
or ``CLAUDE.local.md`` is in the working directory or above it
(https://code.claude.com/docs/en/memory#agents-md), so a ``CLAUDE.md`` written
by a tool can hide a project's own instructions. This script starts one Claude
Code session per instruction-file layout and setup state against the scripted
mock API (:mod:`aeval.mockapi`; no model call, no usage), in a temporary HOME,
and reports which texts the recorded requests contain.

Setup states:

* ``none``    the layout as the project has it;
* ``legacy``  plus the routing block in ``CLAUDE.md``, as ``myc setup-claude``
  wrote it before #200 (emulated, with the text of that version);
* ``setup``   ``myc setup-claude --scope project`` run in the layout;
* ``repair``  the same command run over ``legacy``;
* ``user``    ``myc setup-claude --scope user``, writing to the temporary HOME.

Usage::

    python3 evals/agent/probe_instructions.py --myc target/debug/myc
    python3 evals/agent/probe_instructions.py --states none,legacy   # no binary needed

Exits 1 when a state other than ``legacy`` loses a text that ``none`` had, or
lacks the routing block. The sessions read ``packages/app/code.txt`` with the
Read tool, so instruction files that load on demand in that subdirectory show
up too.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from aeval import mockapi

SUBDIR_FILE = "packages/app/code.txt"
BLOCK = (Path(__file__).resolve().parents[2] / "myc/tests/data/routing_block_v3.md").read_text()
BLOCK_TEXT = "Structural questions go to the myceliums code graph first"


def _text(path: str) -> str:
    """The instruction text a layout puts into ``path``, unique per path."""
    return f"# Instructions\n\nPROBE[{path}] is loaded.\n"


# Layout -> {path relative to the project: content}; a value "-> x" is a symlink.
# "../x" is above the project (still below the temporary HOME).
LAYOUTS: dict[str, dict[str, str]] = {
    "neither": {},
    "AGENTS.md": {"AGENTS.md": ""},
    "CLAUDE.md": {"CLAUDE.md": ""},
    "both": {"AGENTS.md": "", "CLAUDE.md": ""},
    "both, CLAUDE.md imports AGENTS.md": {"AGENTS.md": "", "CLAUDE.md": "@AGENTS.md\n\n" + _text("CLAUDE.md")},
    "CLAUDE.md -> AGENTS.md": {"AGENTS.md": "", "CLAUDE.md": "-> AGENTS.md"},
    "AGENTS.md -> CLAUDE.md": {"CLAUDE.md": "", "AGENTS.md": "-> CLAUDE.md"},
    ".claude/CLAUDE.md": {".claude/CLAUDE.md": ""},
    "AGENTS.md + .claude/CLAUDE.md": {"AGENTS.md": "", ".claude/CLAUDE.md": ""},
    "AGENTS.md + CLAUDE.local.md": {"AGENTS.md": "", "CLAUDE.local.md": ""},
    ".claude/AGENTS.md": {".claude/AGENTS.md": ""},
    "AGENTS.md in a subdirectory": {"packages/app/AGENTS.md": ""},
    "AGENTS.md in root and subdirectory": {"AGENTS.md": "", "packages/app/AGENTS.md": ""},
    "AGENTS.md above the project": {"../AGENTS.md": ""},
}


class ReadOnceAPI(mockapi.MockAPI):
    """Answers the first request with one Read call for the subdirectory file."""

    def answer(self, body: dict) -> bytes:
        with self.lock:
            self.n += 1
            n = self.n
        (self.log_dir / f"request{n:02d}.json").write_text(json.dumps(body, indent=1))
        model = body.get("model") or "probe"
        tools = {t.get("name") for t in body.get("tools") or [] if isinstance(t, dict)}
        if "Read" in tools and mockapi._tool_results(body) == 0:
            return mockapi._message([{"type": "tool_use", "id": "toolu_read", "name": "Read",
                                      "input": {"file_path": self.read_path}}], "tool_use", model)
        return mockapi._message([{"type": "text", "text": "done"}], "end_turn", model)


def _strings(value) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, dict):
        return [s for v in value.values() for s in _strings(v)]
    if isinstance(value, list):
        return [s for v in value for s in _strings(v)]
    return []


def _write_layout(project: Path, layout: dict[str, str]) -> None:
    for rel, content in layout.items():
        path = project / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        if content.startswith("-> "):
            path.symlink_to(content[3:])
        else:
            path.write_text(content or _text(rel))


def _legacy_setup(project: Path) -> None:
    """The routing block where setup-claude put it before #200."""
    memory = project / "CLAUDE.md"
    old = memory.read_text() if memory.exists() else ""
    memory.write_text(old + ("\n" if old else "") + BLOCK)


def _setup(myc: str, scope: str, project: Path, home: Path) -> None:
    cmd = [myc, "setup-claude", "--scope", scope]
    if scope == "project":
        cmd += ["--project", str(project)]
    # A bare PATH: `--scope user` must edit the temporary ~/.claude.json, not call the real `claude`.
    env = {"HOME": str(home), "MYCELIUMS_DATA_DIR": str(home / "myc-data"), "PATH": "/usr/bin:/bin"}
    res = subprocess.run(cmd, env=env, cwd=project, capture_output=True, text=True, timeout=300)
    if res.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed:\n{res.stdout}{res.stderr}")


def probe(claude: str, myc: str | None, layout: dict[str, str], state: str, root: Path,
          keep: Path | None = None) -> dict:
    """One session; what its first and later requests contain."""
    home = root / "home"
    project = home / "work" / "project"
    project.mkdir(parents=True)
    log_dir = root / "requests"
    log_dir.mkdir()
    (project / SUBDIR_FILE).parent.mkdir(parents=True)
    (project / SUBDIR_FILE).write_text("text\n")
    subprocess.run(["git", "init", "-q", str(project)], check=True)
    _write_layout(project, layout)
    if state in ("legacy", "repair"):
        _legacy_setup(project)
    if state in ("setup", "repair"):
        _setup(myc, "project", project, home)
    if state == "user":
        _setup(myc, "user", project, home)
    api = ReadOnceAPI(log_dir, "", "").start()
    api.read_path = str(project / SUBDIR_FILE)
    env = {"HOME": str(home), "PATH": os.environ["PATH"], "TERM": "dumb", "TMPDIR": str(root),
           "MYCELIUMS_DATA_DIR": str(home / "myc-data"), **mockapi.session_env(api.url)}
    cmd = [claude, "-p", "probe", "--output-format", "json", "--permission-mode", "dontAsk",
           "--tools", "Read", "--allowedTools", "Read", "--strict-mcp-config",
           "--mcp-config", '{"mcpServers":{}}', "--settings", '{"disableAllHooks":true}',
           "--no-session-persistence"]
    try:
        res = subprocess.run(cmd, env=env, cwd=project, capture_output=True, text=True, timeout=600,
                             stdin=subprocess.DEVNULL)
    finally:
        api.stop()
    bodies = [json.loads(p.read_text()) for p in sorted(log_dir.glob("request*.json"))]
    main = [b for b in bodies if b.get("tools")]
    if not main:
        sys.exit(f"no request reached the mock API ({state}): {res.stdout[-400:]}{res.stderr[-400:]}")
    if keep is not None:
        shutil.copytree(log_dir, keep)
    blobs = ["\n".join(_strings(b.get("messages")) + _strings(b.get("system"))) for b in main]
    paths = [p for p, c in layout.items() if not c.startswith("-> ")]
    return {
        "start": [p for p in paths if f"PROBE[{p}]" in blobs[0]],
        "later": [p for p in paths if f"PROBE[{p}]" not in blobs[0] and f"PROBE[{p}]" in blobs[-1]],
        "routing_blocks": blobs[-1].count(BLOCK_TEXT),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--claude", default="claude", help="Claude Code executable")
    ap.add_argument("--myc", help="myc executable (needed for setup, repair and user)")
    ap.add_argument("--states", default="none,legacy,setup,repair,user")
    ap.add_argument("--layouts", help="comma-separated layout names (default: all)")
    ap.add_argument("--keep", type=Path, help="keep the recorded requests below this directory")
    ap.add_argument("--json", type=Path, help="write the findings here")
    args = ap.parse_args()
    states = args.states.split(",")
    if not args.myc and set(states) & {"setup", "repair", "user"}:
        ap.error("--myc is needed for the states setup, repair and user")
    myc = str(Path(args.myc).resolve()) if args.myc else None
    names = list(LAYOUTS) if not args.layouts else [n for n in LAYOUTS if n in args.layouts.split(",")]
    version = subprocess.run([args.claude, "--version"], capture_output=True, text=True,
                             env={"HOME": tempfile.gettempdir(), "PATH": os.environ["PATH"]}).stdout.strip()
    findings: dict = {"claude": version, "layouts": {}}
    problems = []
    print(f"{version}\n\n| Layout | State | At session start | After reading in packages/app | Routing blocks |")
    print("|---|---|---|---|---:|")
    for i, name in enumerate(names):
        rows = findings["layouts"][name] = {}
        for state in states:
            with tempfile.TemporaryDirectory(prefix="myc-probe-", ignore_cleanup_errors=True) as tmp:
                keep = args.keep / f"{i:02d}-{state}" if args.keep else None
                row = probe(args.claude, myc, LAYOUTS[name], state, Path(tmp).resolve(), keep)
            rows[state] = row
            print(f"| {name} | {state} | {', '.join(row['start']) or '-'} | {', '.join(row['later']) or '-'} "
                  f"| {row['routing_blocks']} |", flush=True)
            base = rows.get("none")
            if state in ("setup", "repair", "user"):
                if row["routing_blocks"] != 1:
                    problems.append(f"{name} / {state}: {row['routing_blocks']} routing blocks")
                if base and (row["start"], row["later"]) != (base["start"], base["later"]):
                    problems.append(f"{name} / {state}: loads {row}, without myceliums {base}")
    if args.json:
        args.json.write_text(json.dumps(findings, indent=1) + "\n")
    for p in problems:
        print("PROBLEM:", p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
