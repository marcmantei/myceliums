"""Paths, pinned inputs and the isolated environment for agent sessions."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
import os
import shutil
import subprocess
import sys
from pathlib import Path

EVAL_DIR = Path(__file__).resolve().parent.parent
REPOS_FILE = EVAL_DIR / "repos.json"
TASKS_FILE = EVAL_DIR / "tasks" / "tasks.json"  # the pilot's set (#99)
HARD_TASKS_FILE = EVAL_DIR / "tasks" / "tasks-hard.json"  # multi-hop set (#125)
INTENTS_FILE = EVAL_DIR / "toolsel" / "intents.json"
RUNS_DIR = EVAL_DIR / "runs"  # raw transcripts; git-ignored
RESULTS_DIR = EVAL_DIR / "results"  # compact, committed results

# Conditions of a run. `with` / `without` are the pilot's (#99) two arms and
# keep its exact setup. The variant matrix of #125 adds `none` (no MCP
# server, used for calibration) and the integration variants `A`, `C`, `E`,
# which all load project settings and CLAUDE.md files from the working copy
# (`--setting-sources project`) and differ only in what `myc setup-claude`
# put there; see VARIANTS. #178 adds `D`, "myceliums only": the MCP server and
# `Read`, with every way to search denied.
CONDITIONS = ("with", "without", "none", "A", "C", "D", "E")


@dataclass(frozen=True)
class Variant:
    """What a condition adds to the session."""

    mcp: bool  # the myceliums MCP server (default core toolset)
    setup_claude: bool = False  # `myc setup-claude --scope project` files in the working copy
    agent: bool = False  # ... including the code-explorer subagent, and the Agent tool
    legacy: bool = False  # the pilot's flags: no setting sources, CLAUDE.md disabled
    no_search: bool = False  # #178: Grep and Glob denied, search commands in Bash blocked by a hook


VARIANTS = {
    "with": Variant(mcp=True, legacy=True),
    "without": Variant(mcp=False, legacy=True),
    "none": Variant(mcp=False),
    "A": Variant(mcp=True),
    "C": Variant(mcp=True, setup_claude=True),
    "D": Variant(mcp=True, no_search=True),
    "E": Variant(mcp=True, setup_claude=True, agent=True),
}

# Claude Code's built-in subagents (2.1.329). Variant E denies them, so the
# Agent tool offers the code-explorer subagent only and E differs from C by
# that one layer.
BUILTIN_AGENTS = ("Explore", "general-purpose", "Plan", "claude", "statusline-setup", "web-fetch")
EXPLORER_AGENT = "code-explorer"
# Built-in tools of both conditions: read and search only, no shell, no writes.
# ToolSearch is part of the default Claude Code tool set: with it, MCP tool
# definitions are deferred (listed by name, loaded on demand), which is how a
# Claude Code user meets the myceliums server by default. Without MCP servers
# the CLI may drop it, so the isolation check treats it as optional.
BUILTIN_TOOLS = ("Read", "Grep", "Glob", "ToolSearch")
REQUIRED_TOOLS = ("Read", "Grep", "Glob")
SEARCH_TOOLS = ("Grep", "Glob")
# Edit tasks (#178) add these in every condition; they are never offered on
# question tasks.
EDIT_TOOLS = ("Edit", "Write")
# A second layer in D: permission rules that deny the common search programs
# (they only match a command that starts with the program). The first layer
# is the hook's allowlist, which sees every program of a command
# (aeval/bashcmd.py).
BASH_SEARCH_RULES = tuple(f"Bash({p}:*)" for p in ("grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "fd",
                                                   "locate", "git grep"))

# Run profiles (#178). `realistic`: the turn limit is a safety cap only (100,
# never stated in the prompt) and every task runs three times.
# #182: the realistic profile also fixes the interleaved schedule (see
# runner.Runner.plan), so a profile run always compares its arms over the
# same stretch of time.
PROFILES = {"realistic": {"max_turns": 100, "reps": 3, "prompt_budget": False, "schedule": "interleaved"}}
# Schedules of a run's sessions. `paired` (the default since the pilot): per
# repetition a shuffled task order, each task's conditions adjacent in a
# shuffled order. `interleaved` (#182): the same blocks, but the conditions'
# order within a block is counterbalanced, each condition taking each position
# equally often per repetition (rotations of a shuffled order), and a run may not
# be restricted to a subset of its conditions.
SCHEDULES = ("paired", "interleaved")
MCP_SERVER_NAME = "myceliums"


def cache_dir() -> Path:
    return Path(os.environ.get("AEVAL_CACHE", Path.home() / ".cache" / "myceliums-agent-eval"))


def work_root() -> Path:
    """Parent of the fixed per-repository working paths.

    Every session for a repository runs in ``work_root()/<repo>``: a fresh
    copy is made there before each session. The path is fixed because the
    myceliums index is keyed by the repository path, so a prebuilt index can
    only be reused at the path it was built for.
    """
    default = Path(os.path.realpath("/tmp")) / "aeval"
    return Path(os.environ.get("AEVAL_WORK_ROOT", default))


def load_json(path: Path):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def load_repos() -> dict[str, dict]:
    return {r["name"]: r for r in load_json(REPOS_FILE)["repos"]}


def tasks_file() -> Path:
    """The task set in use: ``$AEVAL_TASKS`` (a path) or the pilot's set."""
    return Path(os.environ["AEVAL_TASKS"]) if os.environ.get("AEVAL_TASKS") else TASKS_FILE


def load_tasks() -> list[dict]:
    return load_json(tasks_file())["tasks"]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def pair_notes(res_dir: Path, *arms: str) -> list[str]:
    """Report lines about a pair of conditions that its numbers alone do not show (a confound found after the
    run, with the adjusted value), from ``posthoc.json`` of a results directory; without ``arms``, every pair's."""
    path = res_dir / "posthoc.json"
    notes = load_json(path).get("pair_notes", []) if path.exists() else []
    out: list[str] = []
    for n in notes:
        if not arms or set(n["pair"]) == set(arms):
            out += [f"> **{n['pair'][0]} ÷ {n['pair'][1]}:** {n['text']}", ""]
    return out


def tool_version(cmd: list[str]) -> str:
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        return (out.stdout or out.stderr).strip().splitlines()[0]
    except (OSError, subprocess.SubprocessError, IndexError):
        return "unknown"


def resolve_claude(explicit: str | None) -> str:
    path = explicit or shutil.which("claude")
    if not path:
        sys.exit("claude CLI not found; pass --claude PATH")
    return path


# Environment variables passed through to agent sessions. Everything else of the
# caller's environment is dropped: a session started from inside another agent
# session would otherwise inherit nesting markers, effort overrides and hooks'
# environment.
PASSTHROUGH_ENV = (
    "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "TERM",
    # Credentials: keeps the session on the same login as the caller when the
    # caller uses a non-default secure-storage directory.
    "CLAUDE_SECURESTORAGE_CONFIG_DIR", "CLAUDE_CONFIG_DIR",
    "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN",
)

# Settings that keep the user's own configuration out of the session. Together
# with `--setting-sources ""`, `--strict-mcp-config` and
# `--disable-slash-commands` this removes CLAUDE.md files, auto-memory, user
# hooks and plugins, claude.ai connectors and skills. The init event of every
# transcript is checked against the expected tool set before a run counts.
ISOLATION_ENV = {
    "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
    "DISABLE_AUTOUPDATER": "1",
    "MCP_TIMEOUT": "60000",
    "MCP_TOOL_TIMEOUT": "300000",
}


SYSTEM_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"


def session_env(extra: dict[str, str] | None = None, legacy: bool = True,
                bin_dir: Path | None = None) -> dict[str, str]:
    """The session's environment.

    ``legacy`` (the pilot's arms) disables every CLAUDE.md. The variants keep
    CLAUDE.md files on: with ``--setting-sources project`` Claude Code then
    reads the working copy's CLAUDE.md (where ``myc setup-claude`` writes the
    routing block) but not ``~/.claude/CLAUDE.md`` (checked with ``probe``).
    ``bin_dir`` goes first on PATH: it holds the ``myc`` wrapper the hooks
    call.
    """
    env = {k: os.environ[k] for k in PASSTHROUGH_ENV if k in os.environ}
    env["PATH"] = f"{bin_dir}:{SYSTEM_PATH}" if bin_dir else SYSTEM_PATH
    env.update(ISOLATION_ENV)
    if not legacy:
        env.pop("CLAUDE_CODE_DISABLE_CLAUDE_MDS")
    if extra:
        env.update(extra)
    return env


def copy_tree(src: Path, dst: Path) -> None:
    """Copy a directory tree, using copy-on-write clones where available."""
    if dst.exists():
        raise FileExistsError(dst)
    dst.parent.mkdir(parents=True, exist_ok=True)
    if sys.platform == "darwin":
        res = subprocess.run(["cp", "-c", "-R", str(src), str(dst)], capture_output=True)
        if res.returncode == 0:
            return
    elif shutil.which("cp"):
        res = subprocess.run(["cp", "-a", "--reflink=auto", str(src), str(dst)], capture_output=True)
        if res.returncode == 0:
            return
    shutil.copytree(src, dst, symlinks=True)


def remove_tree(path: Path, must_be_under: Path) -> None:
    """Delete ``path`` only if it lies strictly inside ``must_be_under``."""
    path = path.resolve()
    root = must_be_under.resolve()
    if path == root or root not in path.parents:
        raise RuntimeError(f"refusing to delete {path}: not inside {root}")
    if path.exists():
        shutil.rmtree(path)
