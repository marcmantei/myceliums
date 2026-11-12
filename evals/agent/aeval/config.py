"""Paths, pinned inputs and the isolated environment for agent sessions."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

EVAL_DIR = Path(__file__).resolve().parent.parent
REPOS_FILE = EVAL_DIR / "repos.json"
TASKS_FILE = EVAL_DIR / "tasks" / "tasks.json"
INTENTS_FILE = EVAL_DIR / "toolsel" / "intents.json"
RUNS_DIR = EVAL_DIR / "runs"  # raw transcripts; git-ignored
RESULTS_DIR = EVAL_DIR / "results"  # compact, committed results

CONDITIONS = ("with", "without")
# Built-in tools of both conditions: read and search only, no shell, no writes.
# ToolSearch is part of the default Claude Code tool set: with it, MCP tool
# definitions are deferred (listed by name, loaded on demand), which is how a
# Claude Code user meets the myceliums server by default. Without MCP servers
# the CLI may drop it, so the isolation check treats it as optional.
BUILTIN_TOOLS = ("Read", "Grep", "Glob", "ToolSearch")
REQUIRED_TOOLS = ("Read", "Grep", "Glob")
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


def load_tasks() -> list[dict]:
    return load_json(TASKS_FILE)["tasks"]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


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


def session_env(extra: dict[str, str] | None = None) -> dict[str, str]:
    env = {k: os.environ[k] for k in PASSTHROUGH_ENV if k in os.environ}
    env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
    env.update(ISOLATION_ENV)
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
