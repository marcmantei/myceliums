"""Run agent sessions: one per (task, condition, repetition), resumable.

Each session gets

* a fresh copy of the pinned repository at ``work_root()/<repo>`` (the same
  path every time, see :func:`aeval.config.work_root`),
* for ``with``: a fresh copy of the prebuilt myceliums index in a per-run temp
  directory, served by ``myc mcp`` with ``MYCELIUMS_DATA_DIR`` and ``HOME``
  pointing into that directory, so the user's ``~/.myceliums`` is never read
  or written,
* the same prompt, model, effort, turn limit and built-in tools in both
  conditions; ``with`` adds only the MCP server (and the permission rule that
  lets its tools run without a prompt).

Sessions of the same repository never overlap (they share the working path);
different repositories run concurrently up to ``--concurrency``.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import random
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from . import config
from .editgrade import apply_gold, changed_files, gold_paths, grade_edit, run_test, snapshot, wrapper_problem
from . import isolation
from .grading import Grade, extract_answer, grade, is_success
from .stream import MCP_PREFIX, check_isolation, parse_transcript
from .searchaudit import audit as search_audit
from .tokens import attribute

NOSEARCH_HOOK = Path(__file__).resolve().parent / "bashcmd.py"

# Where `myc setup-claude` puts the routing block: since #200, and before.
ROUTING_FILES = (".claude/rules/myceliums.md", "CLAUDE.md")

TRANSIENT_API_STATUS = {408, 409, 429, 500, 502, 503, 504, 529}
TRANSIENT_TEXT = re.compile(
    r"overloaded|internal server error|connection error|ECONNRESET|socket hang up|"
    r"timed out|fetch failed|Request was aborted|API Error: 5\d\d|API Error: 429",
    re.IGNORECASE,
)
USAGE_LIMIT_TEXT = re.compile(
    r"usage limit|limit reached|out of (extra )?usage|you'?ve (hit|reached) your\b.*\blimit", re.IGNORECASE)

PROMPT_TEMPLATE = """\
You are working in a checkout of the {repo} repository ({language}); it is your current working directory. Answer the question below about this codebase. Investigate the code with the tools you have; do not guess.

Question: {question}

{format_rules}

When you are done, end your reply with exactly one fenced JSON block in this form, with nothing after it:
```json
{{"answer": ["...", "..."]}}
```"""

FORMAT_RULES = {
    "symbols": (
        "Each answer item is the name of one function or method. Write methods as `Type.method` "
        "(for Go, the receiver type without `*`, e.g. `Server.handle`) and plain functions as "
        "`function_name`. For code inside a nested function, closure or callback, name the "
        "outermost enclosing named function or method. Items are names only: no file paths, "
        "signatures or explanations."
    ),
    # #163: questions whose answers are classes, interfaces or settings.
    "names": (
        "Each answer item is one name of the kind the question asks for (a class, an interface, a setting, ...), "
        "written as it appears in the code, for example `MyClass` or `MAX_SIZE`. Items are names only: no "
        "file paths, signatures or explanations."
    ),
    "files": (
        "Each answer item is a file path relative to the repository root, for example "
        "`src/server/handler.ts`. Items are paths only: no line numbers or explanations."
    ),
}


# A budgeted run (#153) states the turn limit in the prompt, the same text in
# every condition, so that the agent can answer before the limit instead of
# being cut off without an answer. `--max-turns N` counts the agent's replies:
# a reply with tool calls (parallel calls in it included) is one turn, and
# the session ends with `error_max_turns` right after the N-th such reply, so
# N - 1 rounds of tool calls fit before the final answer (checked with `probe`
# against the mock API: N = 1 ends after the first tool round, N = 2 lets the
# final text through). `num_turns` in the result counts differently (tool
# calls + 1) and is not what the limit compares.
BUDGET_RULES = (
    "Budget: this session ends after {max_turns} turns. Each of your replies is one turn: a reply with tool calls "
    "(calls in the same reply run in parallel and share the turn) or your final answer. You can make at most "
    "{tool_turns} replies with tool calls; your final answer must come at the latest as reply {max_turns}, "
    "otherwise the session ends without it and the answer is lost. Answer within the budget even if your "
    "investigation is incomplete."
)


# Edit tasks (#178): the answer is the change itself; see aeval/editgrade.py.
EDIT_PROMPT_TEMPLATE = """\
You are working in a checkout of the {repo} repository ({language}); it is your current working directory. Make the change described below in this codebase. Investigate the code with the tools you have before you edit; change every file the change needs and nothing else. Do not commit.

Change: {question}
{budget}
When you are done, reply with a short summary of what you changed."""


def is_edit_task(task: dict) -> bool:
    return task.get("answer_kind") == "diff"


def nosearch_git_env(tmp: Path) -> dict[str, str]:
    """Git settings for arm D, set by the harness in the session's environment
    (the agent cannot change it: the allowlist refuses assignments and
    ``export``): no system or global config, no hooks (an empty
    ``core.hooksPath``), no fsmonitor command, ``cat`` as every pager, ``true``
    as editor, no implicit bare repositories. ``diff.external`` is not pinned:
    an empty value makes ``git diff`` fail ("cannot run"), and the working
    copy's own config, the only one git still reads, cannot be written in D."""
    hooks = tmp / "no-git-hooks"
    hooks.mkdir(exist_ok=True)
    pairs = [("core.hooksPath", str(hooks)), ("core.fsmonitor", "false"), ("safe.bareRepository", "explicit"),
             ("core.pager", "cat"), ("pager.diff", "cat"), ("pager.log", "cat"), ("pager.show", "cat"),
             ("pager.status", "cat"), ("core.editor", "true"), ("sequence.editor", "true")]
    env = {"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_COUNT": str(len(pairs))}
    for i, (k, v) in enumerate(pairs):
        env[f"GIT_CONFIG_KEY_{i}"] = k
        env[f"GIT_CONFIG_VALUE_{i}"] = v
    return env


def allowed_commands(task: dict) -> list:
    """The task's own build/test commands, which arm D's Bash allowlist
    admits exactly (aeval/bashcmd.py ``task_rules``): ``allowed_commands``
    (strings, or ``{"prefix", "args"}`` with an argument pattern), else the
    test command."""
    if task.get("allowed_commands") is not None:
        return list(task["allowed_commands"])
    return [task["test_command"]] if task.get("test_command") else []


def build_prompt(task: dict, repo: dict, budget_turns: int | None = None) -> str:
    if is_edit_task(task):
        budget = ("\n" + BUDGET_RULES.format(max_turns=budget_turns, tool_turns=budget_turns - 1) + "\n"
                  if budget_turns else "")
        if repo.get("build_note"):  # the same text in every arm (#183)
            budget = "\n" + repo["build_note"] + "\n" + budget
        return EDIT_PROMPT_TEMPLATE.format(repo=repo["display_name"], language=repo["language_name"],
                                           question=task["question"], budget=budget)
    rules = FORMAT_RULES[task["answer_kind"]]
    if budget_turns:
        rules += "\n\n" + BUDGET_RULES.format(max_turns=budget_turns, tool_turns=budget_turns - 1)
    return PROMPT_TEMPLATE.format(
        repo=repo["display_name"],
        language=repo["language_name"],
        question=task["question"],
        format_rules=rules,
    )


@dataclass
class RunConfig:
    label: str
    model: str = "sonnet"
    effort: str | None = "medium"
    max_turns: int = 40
    timeout_s: int = 900
    reps: int = 3
    concurrency: int = 3
    max_retries: int = 2
    success_threshold: float = 0.8
    max_utilization: float = 0.97
    max_sessions: int | None = None
    seed: int = 20261112
    tool_search: str | None = None  # value for ENABLE_TOOL_SEARCH; None = CLI default
    claude: str = "claude"
    myc: str = ""
    models_dir: str = ""
    conditions: tuple[str, ...] = ("with", "without")
    tasks_file: str = ""  # empty: the pilot's set (kept out of old configs)
    extra_env: dict = field(default_factory=dict)  # `probe` only: the mock API
    stop_after_s: int | None = None  # start no session after this many seconds
    prompt_budget: bool = False  # state --max-turns in the prompt (#153's budgeted sessions)
    bash: bool = False  # add the Bash tool in every condition (#163)
    profile: str = ""  # #178: the named profile (config.PROFILES) the run's limits came from
    schedule: str = "paired"  # #182: config.SCHEDULES; "interleaved" counterbalances the arms' order
    mock_api: bool = False  # #182: every session against the scripted mock API (pipeline check, no model)

    def comparable(self) -> dict:
        """Fields that must not change when a run is resumed."""
        d = asdict(self)
        for k in ("concurrency", "max_retries", "max_utilization", "max_sessions", "claude", "extra_env",
                  "stop_after_s"):
            d.pop(k)
        # Fields added after the pilot stay out of its config when unset, so
        # the pilot's config.json still compares equal.
        if tuple(d["conditions"]) == ("with", "without"):
            d.pop("conditions")
        else:
            d["conditions"] = list(d["conditions"])
        if not d["tasks_file"]:
            d.pop("tasks_file")
        if not d["prompt_budget"]:
            d.pop("prompt_budget")
        if not d["bash"]:
            d.pop("bash")
        if not d["profile"]:
            d.pop("profile")
        if d["schedule"] == "paired":  # the order every run before #182 used
            d.pop("schedule")
        if not d["mock_api"]:
            d.pop("mock_api")
        return d


@dataclass
class WorkItem:
    task: dict
    condition: str
    rep: int

    @property
    def run_id(self) -> str:
        return f"{self.task['id']}__{self.condition}__r{self.rep}"

    @property
    def repo(self) -> str:
        return self.task["repo"]


class StopRun(Exception):
    """Raised to stop scheduling: isolation broken, usage limit, budget."""


@dataclass
class Scheduler:
    items: list[WorkItem]
    busy: set = field(default_factory=set)
    work_names: dict = field(default_factory=dict)  # repo -> shared work path (#183); sessions there never overlap
    stop_reason: str | None = None
    cond: threading.Condition = field(default_factory=threading.Condition)

    def key(self, it: WorkItem) -> str:
        return self.work_names.get(it.repo, it.repo)

    def next(self) -> WorkItem | None:
        with self.cond:
            while True:
                if self.stop_reason or not self.items:
                    return None
                for i, it in enumerate(self.items):
                    if self.key(it) not in self.busy:
                        self.busy.add(self.key(it))
                        return self.items.pop(i)
                self.cond.wait()

    def done(self, it: WorkItem) -> None:
        with self.cond:
            self.busy.discard(self.key(it))
            self.cond.notify_all()

    def stop(self, reason: str) -> None:
        with self.cond:
            if not self.stop_reason:
                self.stop_reason = reason
            self.cond.notify_all()


class Runner:
    def __init__(self, cfg: RunConfig, tasks: list[dict], repos: dict[str, dict]):
        self.cfg = cfg
        self.tasks = tasks
        self.repos = repos
        self.run_dir = config.RUNS_DIR / cfg.label
        self.results_path = self.run_dir / "results.jsonl"
        self.ledger_path = config.RUNS_DIR / "ledger.jsonl"
        self.lock = threading.Lock()
        self.sessions_started = 0
        self.cache = config.cache_dir()
        self.work_root = config.work_root()
        self.claude_version = ""  # set by run(): one version for a whole run

    # ---- bookkeeping -----------------------------------------------------
    def _check_config(self) -> None:
        self.run_dir.mkdir(parents=True, exist_ok=True)
        path = self.run_dir / "config.json"
        current = self.cfg.comparable()
        if path.exists():
            saved = config.load_json(path)
            if saved != current:
                diff = {k: (saved.get(k), current.get(k)) for k in set(saved) | set(current)
                        if saved.get(k) != current.get(k)}
                raise SystemExit(f"run '{self.cfg.label}' was started with a different config: {diff}")
        else:
            path.write_text(json.dumps(current, indent=2) + "\n")

    def _pin_claude(self) -> None:
        """One Claude Code for a whole run (#183; it updated itself during the
        #182 run): the binary behind the ``claude`` link, its version kept in the
        run's directory. A resume with another version is refused, and every
        session's init event is checked."""
        self.cfg.claude = os.path.realpath(self.cfg.claude)
        m = re.search(r"\d+\.\d+\.\d+", config.tool_version([self.cfg.claude, "--version"]))
        if not m:
            raise SystemExit(f"cannot read the version of {self.cfg.claude}")
        path = self.run_dir / "claude_version.txt"
        pinned = path.read_text().strip() if path.exists() else m.group(0)
        if pinned != m.group(0):
            raise SystemExit(f"run '{self.cfg.label}' is pinned to Claude Code {pinned}, {self.cfg.claude} is "
                             f"{m.group(0)}; pass --claude PATH of the pinned version")
        path.write_text(pinned + "\n")
        self.claude_version = pinned

    def _check_login(self, item: WorkItem) -> None:
        """Is the CLI logged in *inside* the sandbox profile with a fresh
        configuration directory? `auth status` only, no model call: a profile
        that hides the login would fail every session."""
        tmp = Path(tempfile.mkdtemp(prefix="aeval-run-", dir=self.cache / "tmp"))
        work = tmp / "work"
        work.mkdir()
        try:
            prof = self._sandbox_profile(item, tmp, work, tmp)
            env = config.session_env(isolation.session_environment(tmp), legacy=False)
            res = subprocess.run(isolation.sandboxed([self.cfg.claude, "auth", "status"], prof), cwd=work, env=env,
                                 capture_output=True, text=True, timeout=120)
            if not re.search(r'"loggedIn":\s*true', res.stdout):
                raise SystemExit(f"Claude Code is not logged in inside the sandbox: {(res.stderr or res.stdout).strip()[:200]}")
        finally:
            config.remove_tree(tmp, self.cache / "tmp")

    def completed(self) -> set[str]:
        done = set()
        if self.results_path.exists():
            for line in self.results_path.read_text().splitlines():
                if line.strip():
                    row = json.loads(line)
                    if row.get("final"):
                        done.add(row["run_id"])
        return done

    def _append(self, path: Path, row: dict) -> None:
        with self.lock:
            with open(path, "a", encoding="utf-8") as fh:
                fh.write(json.dumps(row, sort_keys=True) + "\n")

    def plan(self) -> list[WorkItem]:
        """Repetition-major, shuffled order; conditions adjacent per task.

        If a run stops early (usage guard), every completed repetition is a
        complete paired block over all tasks. With ``schedule="interleaved"``
        (#182) the conditions' order inside each task's block is
        counterbalanced: per repetition the blocks cycle through the rotations
        of one shuffled order, assigned to the tasks at random, so each
        condition runs first, second, ... equally often (exactly, when the
        number of tasks is a multiple of the number of conditions). The
        sessions of all arms are thus spread evenly over the run's time,
        and no arm is systematically the first or last of its block.
        """
        rng = random.Random(self.cfg.seed)
        items: list[WorkItem] = []
        for rep in range(1, self.cfg.reps + 1):
            order = list(self.tasks)
            rng.shuffle(order)
            if self.cfg.schedule == "interleaved":
                base = list(self.cfg.conditions)
                rng.shuffle(base)
                k = len(base)
                rotations = [base[i:] + base[:i] for i in range(k)]
                slots = [rotations[i % k] for i in range(len(order))]
                rng.shuffle(slots)
                for task, conds in zip(order, slots):
                    items.extend(WorkItem(task, c, rep) for c in conds)
                continue
            for task in order:
                conds = list(self.cfg.conditions)
                rng.shuffle(conds)
                items.extend(WorkItem(task, c, rep) for c in conds)
        return items

    # ---- one session -----------------------------------------------------
    def _prepare_workspace(self, repo: str) -> Path:
        dst = self.work_root / config.work_name(self.repos[repo])
        if dst.exists():
            config.remove_tree(dst, self.work_root)
        config.copy_tree(self.cache / "repos" / repo, dst)
        head = subprocess.run(["git", "-C", str(dst), "rev-parse", "HEAD"],
                              capture_output=True, text=True).stdout.strip()
        if head != self.repos[repo]["commit"]:
            raise StopRun(f"{repo}: checkout HEAD {head} != pinned {self.repos[repo]['commit']}")
        return dst

    def _copy_index(self, repo: str, tmp: Path) -> tuple[Path, Path]:
        """A fresh copy of the prebuilt index: (data dir, home dir)."""
        template = self.cache / "index" / repo / "home" / ".myceliums"
        if not (template / "repos.json").exists():
            raise StopRun(f"no index for {repo}; run `agent_eval.py index` first")
        meta_path = self.cache / "index" / repo / "meta.json"
        meta = config.load_json(meta_path) if meta_path.exists() else {}
        if meta.get("embeddings") == "skipped" and not (self.cfg.mock_api or "ANTHROPIC_BASE_URL" in self.cfg.extra_env):
            raise StopRun(f"the index of {repo} has no embeddings (index --skip-embeddings, for --mock-api "
                          f"checks only); build it with `agent_eval.py index --repo {repo}`")
        data = tmp / "data"
        home = tmp / "home"
        config.copy_tree(template, data)
        home.mkdir()
        registry = json.loads((data / "repos.json").read_text())
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        for info in registry["repos"].values():
            # The index content is unchanged since indexing (same commit, fresh
            # copy); refreshing the timestamp keeps a cache-aware `analyze` call
            # (or the SessionStart hook) from re-indexing just because the
            # index is older than an hour.
            info["analyzed_at"] = now
        (data / "repos.json").write_text(json.dumps(registry, indent=2))
        return data, home

    def _myc_env(self, data: Path, home: Path) -> dict[str, str]:
        return {"MYCELIUMS_DATA_DIR": str(data), "HOME": str(home),
                "FASTEMBED_CACHE_DIR": self.cfg.models_dir, "RUST_LOG": "warn"}

    def _write_wrapper(self, bin_dir: Path, env: dict[str, str], log_dir: Path) -> Path:
        """``bin_dir/myc``: the real binary with the session's index.

        The hooks that ``myc setup-claude`` installs call ``myc`` from PATH;
        the session's PATH starts with ``bin_dir``, so they reach this script,
        which pins ``MYCELIUMS_DATA_DIR`` and ``HOME`` to the per-session copy.
        For the hook subcommands it also keeps the hook's input, output and
        reason for silence (``MYC_HOOK_DEBUG``) in ``log_dir``; the agent sees
        exactly what it would see without the log.
        """
        bin_dir.mkdir(parents=True, exist_ok=True)
        log_dir.mkdir(parents=True, exist_ok=True)
        exports = " ".join(f"{k}={shlex.quote(v)}" for k, v in env.items())
        script = f"""#!/bin/sh
export {exports}
case "$1" in
  hook-grep|session|format-hook)
    f={shlex.quote(str(log_dir))}/"$1-$$"
    cat > "$f.in"
    MYC_HOOK_DEBUG=1 {shlex.quote(self.cfg.myc)} "$@" < "$f.in" > "$f.out" 2> "$f.err"
    rc=$?
    cat "$f.out"
    exit $rc
    ;;
esac
exec {shlex.quote(self.cfg.myc)} "$@"
"""
        wrapper = bin_dir / "myc"
        wrapper.write_text(script)
        wrapper.chmod(0o755)
        return wrapper

    def _setup_claude(self, item: WorkItem, workdir: Path, bin_dir: Path, env: dict[str, str],
                      out_dir: Path) -> None:
        """Run ``myc setup-claude --scope project`` in the working copy."""
        variant = config.VARIANTS[item.condition]
        cmd = [str(bin_dir / "myc"), "setup-claude", "--scope", "project", "--project", str(workdir)]
        if not variant.agent:
            cmd.append("--no-agent")
        res = subprocess.run(cmd, env={**env, "PATH": f"{bin_dir}:{config.SYSTEM_PATH}"},
                             capture_output=True, text=True, timeout=120)
        (out_dir / "setup-claude.txt").write_text(res.stdout + res.stderr)
        if res.returncode != 0:
            raise StopRun(f"{item.run_id}: myc setup-claude failed: {res.stderr.strip()[:300]}")
        # Claude Code reads a project's AGENTS.md only when there is no CLAUDE.md, and setup-claude writes one: without
        # the import, C alone would miss the project's own instructions (found in the #182 review).
        agents, claude_md = workdir / "AGENTS.md", workdir / "CLAUDE.md"
        if agents.exists() and claude_md.exists() and "@AGENTS.md" not in claude_md.read_text():
            claude_md.write_text("@AGENTS.md\n\n" + claude_md.read_text())
        # Keep what the agent will load, for the record.
        snap = out_dir / "setup-files"
        for rel in (*ROUTING_FILES, ".mcp.json", ".claude/settings.json", f".claude/agents/{config.EXPLORER_AGENT}.md"):
            src = workdir / rel
            if src.exists():
                (snap / rel).parent.mkdir(parents=True, exist_ok=True)
                (snap / rel).write_text(src.read_text())
        # The eval repositories' own CLAUDE.md, if any, is no routing block.
        routed = any("BEGIN myceliums" in (workdir / rel).read_text() for rel in ROUTING_FILES
                     if (workdir / rel).exists())
        if not routed or not (workdir / ".claude" / "settings.json").exists():
            raise StopRun(f"{item.run_id}: setup-claude did not write the routing block and .claude/settings.json")
        if variant.agent != (workdir / ".claude" / "agents" / f"{config.EXPLORER_AGENT}.md").exists():
            raise StopRun(f"{item.run_id}: code-explorer subagent file does not match variant {item.condition}")

    def _prepare_mcp(self, item: WorkItem, tmp: Path, workdir: Path | None = None,
                     out_dir: Path | None = None) -> tuple[Path, Path | None]:
        """MCP config for the session, and the bin dir to put first on PATH."""
        variant = config.VARIANTS[item.condition]
        mcp_path = tmp / "mcp.json"
        if not variant.mcp:
            mcp_path.write_text(json.dumps({"mcpServers": {}}))
            return mcp_path, None
        data, home = self._copy_index(item.repo, tmp)
        env = self._myc_env(data, home)
        if variant.legacy:
            command, bin_dir = self.cfg.myc, None
        else:
            bin_dir = tmp / "bin"
            command = str(self._write_wrapper(bin_dir, env, (out_dir or tmp) / "hooks"))
            if variant.setup_claude:
                self._setup_claude(item, workdir, bin_dir, env, out_dir or tmp)
        server = {"type": "stdio", "command": command, "args": ["mcp"], "env": env}
        mcp_path.write_text(json.dumps({"mcpServers": {config.MCP_SERVER_NAME: server}}, indent=2))
        return mcp_path, bin_dir

    def _builtin_tools(self, item: WorkItem) -> tuple[str, ...]:
        """Built-in tools of a session: read and search (no search in D),
        Bash with ``--bash``, the edit tools on edit tasks (#178)."""
        variant = config.VARIANTS[item.condition]
        tools = tuple(t for t in config.BUILTIN_TOOLS if not (variant.no_search and t in config.SEARCH_TOOLS))
        tools += ("Bash",) if self.cfg.bash else ()
        tools += config.EDIT_TOOLS if is_edit_task(item.task) else ()
        return tools

    def _isolated_settings(self, item: WorkItem, tmp: Path, workdir: Path) -> Path:
        """Settings of an isolated session (#183): one PreToolUse hook on every
        tool, the same in every arm except ``--search`` (aeval/bashcmd.py
        ``isolated-hook``). The hook's script is copied next to them, because
        the session may not read this repository."""
        search = "no" if config.VARIANTS[item.condition].no_search else "yes"
        script = tmp / "policy" / "bashcmd.py"
        script.parent.mkdir()
        script.write_bytes(NOSEARCH_HOOK.read_bytes())
        hook = " ".join(shlex.quote(x) for x in [
            sys.executable, "-I", str(script), "isolated-hook", "--root", os.path.realpath(workdir), "--search", search,
            "--allow-json", json.dumps(allowed_commands(item.task))])
        path = tmp / "isolated-settings.json"
        path.write_text(json.dumps({"hooks": {"PreToolUse": [
            {"matcher": "*", "hooks": [{"type": "command", "command": hook}]}]}}, indent=1))
        return path

    def _sandbox_profile(self, item: WorkItem, tmp: Path, workdir: Path, out_dir: Path) -> Path:
        """The kernel layer (aeval/isolation.py): only this session's own files
        of the protected places are readable."""
        if not isolation.available():
            raise StopRun("an isolated repository (#183) needs macOS's sandbox-exec; not found")
        path = tmp / "session.sb"
        hooks = out_dir / "hooks"
        hooks.mkdir(parents=True, exist_ok=True)
        myc = [Path(self.cfg.myc), Path(self.cfg.models_dir)] if config.VARIANTS[item.condition].mcp else []
        path.write_text(isolation.session_profile(tmp, workdir, hooks, self.cfg.claude,
                                                  config.repo_env(self.repos[item.repo]), myc))
        return path

    def _nosearch_settings(self, tmp: Path, task: dict | None = None, hook_dir: Path | None = None) -> Path:
        """Settings for D (#178): a PreToolUse hook on Bash that lets a command
        through only if every program it runs is on the allowlist
        (aeval/bashcmd.py: ``myc``, plain file operations, git without search,
        and the task's own build/test commands), passed with ``--settings`` so
        it applies whatever the setting sources are. Claude Code shows the
        model the hook's command line with every refusal, so the session runs
        a copy in ``hook_dir``, outside the eval checkout and the cache: the
        original's path would point at the gold sets (#182)."""
        script = NOSEARCH_HOOK
        if hook_dir is not None:
            script = hook_dir / NOSEARCH_HOOK.name
            shutil.copyfile(NOSEARCH_HOOK, script)
            script.chmod(0o444)
        allow = allowed_commands(task or {})
        hook = (f"{shlex.quote(sys.executable)} -I {shlex.quote(str(script))} hook"
                + (f" --allow-json {shlex.quote(json.dumps(allow))}" if allow else ""))
        # Bash: the allowlist; the edit tools: no writes into .git (planted git config could run anything).
        settings = {"hooks": {"PreToolUse": [{"matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                                              "hooks": [{"type": "command", "command": hook}]}]}}
        path = tmp / "nosearch-settings.json"
        path.write_text(json.dumps(settings, indent=1))
        return path

    def _command(self, item: WorkItem, mcp_path: Path, settings_path: Path | None = None) -> list[str]:
        variant = config.VARIANTS[item.condition]
        tools = list(self._builtin_tools(item))
        allowed = list(self._builtin_tools(item))
        if variant.mcp:
            allowed.append(f"mcp__{config.MCP_SERVER_NAME}")
        if variant.agent:
            tools.append("Agent")
            allowed.append(f"Agent({config.EXPLORER_AGENT})")
        cmd = [
            self.cfg.claude, "-p",
            "--output-format", "stream-json", "--verbose",
            "--model", self.cfg.model,
            "--max-turns", str(self.cfg.max_turns),
            "--tools", ",".join(tools),
            "--allowedTools", ",".join(allowed),
        ]
        disallowed = [f"Agent({a})" for a in config.BUILTIN_AGENTS] if variant.agent else []
        if self.repos[item.repo].get("isolated"):
            disallowed += ["WebFetch", "WebSearch"]
        if variant.no_search:
            disallowed += list(config.SEARCH_TOOLS) + (list(config.BASH_SEARCH_RULES) if self.cfg.bash else [])
        if disallowed:
            cmd += ["--disallowedTools", ",".join(disallowed)]
        if settings_path is not None:
            cmd += ["--settings", str(settings_path)]
        cmd += [
            "--permission-mode", "dontAsk",
            "--strict-mcp-config", "--mcp-config", str(mcp_path),
            "--setting-sources", "" if variant.legacy else "project",
            "--disable-slash-commands",
            "--no-session-persistence",
        ]
        if not variant.legacy:
            cmd.append("--include-hook-events")
        if self.cfg.effort:
            cmd += ["--effort", self.cfg.effort]
        return cmd

    def _session(self, item: WorkItem, attempt: int) -> dict:
        task = item.task
        repo = self.repos[item.repo]
        prompt = build_prompt(task, repo, self.cfg.max_turns if self.cfg.prompt_budget else None)
        out_dir = self.run_dir / "sessions" / item.run_id
        out_dir.mkdir(parents=True, exist_ok=True)
        transcript = out_dir / f"attempt{attempt}.jsonl"
        stderr_path = out_dir / f"attempt{attempt}.stderr"
        (out_dir / "prompt.txt").write_text(prompt)

        with self.lock:
            if self.cfg.max_sessions is not None and self.sessions_started >= self.cfg.max_sessions:
                raise StopRun(f"session budget of {self.cfg.max_sessions} reached")
            self.sessions_started += 1
        self._append(self.ledger_path, {
            "label": self.cfg.label, "run_id": item.run_id, "attempt": attempt,
            "model": self.cfg.model, "started": dt.datetime.now(dt.timezone.utc).isoformat(),
            **({"probe": True} if self.cfg.extra_env else {}),
        })

        tmp = Path(tempfile.mkdtemp(prefix="aeval-run-", dir=self.cache / "tmp"))
        variant = config.VARIANTS[item.condition]
        hook_dir = Path(tempfile.mkdtemp(prefix="hook-")) if variant.no_search else None
        try:
            workdir = self._prepare_workspace(item.repo)
            if problem := wrapper_problem(workdir, config.repo_env(repo)):
                raise StopRun(f"{item.run_id}: {problem}")
            mcp_path, bin_dir = self._prepare_mcp(item, tmp, workdir, out_dir)
            (out_dir / "mcp.json").write_text(mcp_path.read_text())
            extra = dict(self.cfg.extra_env)
            if self.cfg.bash:
                # With a shell the agent could run a `myc` binary it finds on
                # disk; this keeps any such run away from the user's data
                # directory: the session's index copy in C, an empty
                # directory without myceliums.
                stray = tmp / "data" if variant.mcp else tmp / "no-myceliums"
                stray.mkdir(exist_ok=True)
                extra["MYCELIUMS_DATA_DIR"] = str(stray)
            if self.cfg.tool_search is not None:
                extra["ENABLE_TOOL_SEARCH"] = self.cfg.tool_search
            isolated = bool(repo.get("isolated"))
            if variant.no_search or isolated:
                extra.update(nosearch_git_env(tmp))
            # #183: what the repository's build needs (JAVA_HOME, Gradle options).
            extra.update(config.repo_env(repo))
            if isolated:
                extra.update(isolation.session_environment(tmp))
            env = config.session_env(extra, legacy=variant.legacy, bin_dir=bin_dir)
            # Files the harness changed before the agent starts (setup-claude's
            # CLAUDE.md, .mcp.json) are not the agent's edits.
            baseline = snapshot(workdir) if is_edit_task(task) else None
            settings = (self._isolated_settings(item, tmp, workdir) if isolated
                        else self._nosearch_settings(tmp, task, hook_dir) if variant.no_search else None)
            if settings is not None:
                (out_dir / "settings.json").write_text(settings.read_text())
            cmd = self._command(item, mcp_path, settings)
            sandbox = self._sandbox_profile(item, tmp, workdir, out_dir) if isolated else None
            if sandbox is not None:
                (out_dir / "session.sb").write_text(sandbox.read_text())
                cmd = isolation.sandboxed(cmd, sandbox)
            (out_dir / "command.json").write_text(json.dumps(cmd, indent=1))
            started = time.time()
            timed_out = False
            with open(transcript, "w") as out, open(stderr_path, "w") as err:
                proc = subprocess.Popen(cmd, cwd=workdir, env=env, stdin=subprocess.PIPE,
                                        stdout=out, stderr=err, text=True, start_new_session=True)
                proc.stdin.write(prompt)
                proc.stdin.close()
                try:
                    proc.wait(timeout=self.cfg.timeout_s)
                except subprocess.TimeoutExpired:
                    timed_out = True
                    _kill_group(proc)
            wall = time.time() - started
            edit = self._grade_edit(item, workdir, out_dir, baseline, sandbox) if is_edit_task(task) else None
        finally:
            config.remove_tree(tmp, self.cache / "tmp")
            if hook_dir is not None:
                shutil.rmtree(hook_dir, ignore_errors=True)
            # The working copy stays until the next session of this repo
            # replaces it; only edit tasks (#178) write to it, and they are
            # graded above, before that.

        return self._summarise(item, attempt, transcript, wall, timed_out, proc.returncode, edit)

    def _grade_edit(self, item: WorkItem, workdir: Path, out_dir: Path, baseline: dict | None = None,
                    sandbox: Path | None = None) -> dict:
        """Grade an edit task's working copy: changed files, then the test
        command, after the commit's own tests (``test_overlay``, #183) are
        written over the agent's working copy."""
        base = self.repos[item.repo]["commit"]  # the parent, not HEAD: the agent may have committed
        changed = changed_files(workdir, baseline, base)
        diff = subprocess.run(["git", "-C", str(workdir), "diff", base], capture_output=True, text=True)
        (out_dir / "changes.diff").write_text(diff.stdout)
        untracked = [f for f in changed if (workdir / f).is_file()
                     and subprocess.run(["git", "-C", str(workdir), "ls-files", "--error-unmatch", f],
                                        capture_output=True).returncode != 0]
        if untracked:
            (out_dir / "untracked.txt").write_text("\n".join(untracked) + "\n")
        tests = None
        overlay = item.task.get("test_overlay") or []
        if item.task.get("test_command"):
            if overlay:
                apply_gold(workdir, config.gold_dir(item.repo), item.task, overlay)
            env = config.repo_env(self.repos[item.repo])
            if problem := wrapper_problem(workdir, env):  # the agent changed the wrapper: never let it connect
                tests = {"command": item.task["test_command"], "passed": False, "returncode": None,
                         "timed_out": False, "seconds": 0.0, "output_tail": problem, "output": problem}
            else:
                tests = run_test(item.task["test_command"], workdir, int(item.task.get("test_timeout_s", 900)), env,
                                 isolation.sandboxed([], sandbox) if sandbox else ())
        if tests is not None:  # the row keeps the tail, the session directory the whole log
            (out_dir / "test.log").write_text(tests.pop("output", ""))
        g = grade_edit(item.task, changed, tests, self.cfg.success_threshold)
        if overlay:
            g["test_overlay"] = list(overlay)
        return g

    def _summarise(self, item: WorkItem, attempt: int, transcript: Path, wall: float,
                   timed_out: bool, returncode: int | None, edit: dict | None = None) -> dict:
        t = parse_transcript(str(transcript))
        res = t.result or {}
        variant = config.VARIANTS[item.condition]
        optional = set(config.BUILTIN_TOOLS) - set(config.REQUIRED_TOOLS)
        required = ((set(self._builtin_tools(item)) - optional) | ({"Agent"} if variant.agent else set()))
        init = dict(t.init) if t.init else None
        if init:  # the init event names the Agent tool by its old name, Task
            init["tools"] = ["Agent" if x == "Task" else x for x in init.get("tools") or []]
        problems = check_isolation(init, required, variant.mcp, optional)
        if init and self.claude_version and init.get("claude_code_version") not in (None, self.claude_version):
            problems.append(f"Claude Code {init.get('claude_code_version')} ran, the run is pinned to {self.claude_version}")
        if variant.agent and t.init and config.EXPLORER_AGENT not in (t.init.get("agents") or []):
            problems.append(f"{config.EXPLORER_AGENT} subagent not loaded: {t.init.get('agents')}")
        hooks = hook_stats(transcript.parent / "hooks")
        text = t.final_text
        if edit is not None:  # #178: graded from the working copy, not from an answer list
            answer = None
            g = None
        else:
            answer = extract_answer(text) or extract_answer("\n".join(t.texts[-3:]))
            g = grade(item.task, answer, self.cfg.success_threshold)

        if problems and t.init:
            status = "isolation_error"
        elif t.rate_limit_rejected or USAGE_LIMIT_TEXT.search(text or "") and res.get("is_error"):
            # Before the timeout: the CLI may sleep until the limit resets, and a session
            # the limit cut off must never be graded as its arm's failure (#182).
            status = "usage_limit"
        elif timed_out:
            status = "timeout"
        elif not res:
            status = "crash"
        elif res.get("subtype") == "error_max_turns":
            status = "max_turns"
        elif res.get("is_error") or res.get("subtype") != "success":
            api = res.get("api_error_status")
            if (isinstance(api, int) and api in TRANSIENT_API_STATUS) or TRANSIENT_TEXT.search(text or ""):
                status = "api_error"
            elif returncode is not None and (returncode < 0 or returncode in (130, 137, 143)):
                status = "crash"  # killed from outside (SIGINT/SIGKILL/SIGTERM): infrastructure, retried (#182)
            else:
                status = "error"
        else:
            status = "ok" if answer is not None or edit is not None else "no_answer"

        counts = t.tool_counts
        mcp_calls = {k[len(MCP_PREFIX):]: v for k, v in counts.items() if k.startswith(MCP_PREFIX)}
        cli = myc_cli_calls(t.tool_calls)
        usage = t.usage()
        if g is None:
            missed = sorted(set(gold_paths(item.task)) - set(edit["files_changed"]))
            g = Grade(edit["file_precision"], edit["file_recall"], edit["file_f1"],
                      len(edit["files_changed"]) - len(edit["files_extra"]), len(edit["files_extra"]), 0,
                      len(item.task.get("gold_files") or []), len(edit["files_changed"]), True,
                      missed=missed, false_positives=edit["files_extra"])
        success = edit["success"] if edit is not None else is_success(g.f1, self.cfg.success_threshold)
        row = {
            "run_id": item.run_id,
            "task_id": item.task["id"],
            "repo": item.repo,
            "type": item.task["type"],
            "condition": item.condition,
            "rep": item.rep,
            "attempt": attempt,
            "status": status,
            "final": status in {"ok", "no_answer", "max_turns", "timeout", "error"},
            "isolation_problems": problems,
            "answer": answer,
            "f1": round(g.f1, 4),
            "precision": round(g.precision, 4),
            "recall": round(g.recall, 4),
            "success": success,
            "tp": g.tp, "fp": g.fp, "neutral": g.neutral, "n_gold": g.n_gold,
            "n_answer": g.n_answer,
            "missed": g.missed,
            "false_positives": g.false_positives,
            "num_turns": res.get("num_turns"),
            "tool_calls": sum(counts.values()),
            "tool_calls_by_name": dict(sorted(counts.items())),
            "mcp_tool_calls": sum(mcp_calls.values()),
            "mcp_tools_used": sorted(mcp_calls),
            "tool_errors": dict(t.tool_errors),
            "first_tool": t.tool_calls[0]["name"] if t.tool_calls else None,
            # #163: with Bash, myceliums can also be reached through its CLI.
            "bash_calls": counts.get("Bash", 0),
            "myc_cli_calls": len(cli),
            "myc_cli_subcommands": sorted({c for c in cli if c}),
            # Variant matrix (#125): who made the calls, and what the hooks did.
            "agent_calls": counts.get("Agent", 0),
            "subagent_tool_calls": sum(1 for c in t.tool_calls if c.get("parent")),
            "subagent_mcp_tool_calls": sum(1 for c in t.tool_calls
                                           if c.get("parent") and c["name"].startswith(MCP_PREFIX)),
            "first_mcp_call_index": next((i for i, c in enumerate(t.tool_calls)
                                          if c["name"].startswith(MCP_PREFIX)), None),
            **hooks,
            **usage,
            "first_turn_input_tokens": t.first_turn_input_tokens,
            "cost_usd": res.get("total_cost_usd"),
            "duration_ms": res.get("duration_ms"),
            "duration_api_ms": res.get("duration_api_ms"),
            "wall_s": round(wall, 2),
            "returncode": returncode,
            "model": (t.init or {}).get("model"),
            "models_used": t.models(),
            "claude_version": (t.init or {}).get("claude_code_version"),
            "n_tools_offered": len((t.init or {}).get("tools") or []),
            "n_mcp_tools_offered": sum(1 for x in (t.init or {}).get("tools") or [] if x.startswith("mcp__")),
            "max_utilization": t.max_utilization,
            "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        }
        # #178: where the tokens went, the cost by cache class, and whether the agent searched.
        att = attribute(str(transcript))
        row.update(att.as_row())
        row["api_calls"] = att.calls  # model calls; the CLI's num_turns is tool calls + 1
        prompt_path = transcript.parent / "prompt.txt"
        row["search_audit"] = search_audit(str(transcript), prompt_path.read_text() if prompt_path.exists() else "")
        if edit is not None:
            row["edit"] = edit
            if item.task.get("weak_test"):
                row["weak_test"] = True
        if self.repos[item.repo].get("isolated"):
            row["isolation_audit"] = isolation.audit(
                str(transcript), str(self.work_root / config.work_name(self.repos[item.repo])))
        return row

    # ---- loop ------------------------------------------------------------
    def _worker(self, sched: Scheduler) -> None:
        while True:
            if self.cfg.stop_after_s is not None and time.time() - self.started_at > self.cfg.stop_after_s:
                sched.stop(f"--stop-after {self.cfg.stop_after_s} s reached")
            if (self.run_dir / "STOP").exists():
                # `touch runs/<label>/STOP` ends a run cleanly: running sessions
                # finish and are recorded, no new one starts.
                sched.stop("STOP file present")
            item = sched.next()
            if item is None:
                return
            try:
                self._run_item(item, sched)
            except StopRun as exc:
                sched.stop(str(exc))
            except Exception as exc:  # noqa: BLE001 - keep other workers going, record why
                sched.stop(f"{item.run_id}: unexpected {type(exc).__name__}: {exc}")
                raise
            finally:
                sched.done(item)

    def _run_item(self, item: WorkItem, sched: Scheduler) -> None:
        for attempt in range(1, self.cfg.max_retries + 2):
            if sched.stop_reason:
                return
            row = self._session(item, attempt)
            self._append(self.results_path, row)
            print(f"[{row['finished_at'][11:19]}] {item.run_id:<52} {row['status']:<10} "
                  f"F1={row['f1']:.2f} tools={row['tool_calls']:>3} (mcp {row['mcp_tool_calls']:>2}) "
                  f"turns={row['num_turns']} in={row['total_input_tokens']:>8} "
                  f"out={row['output_tokens']:>6} {row['wall_s']:>6.0f}s util={row['max_utilization']}",
                  flush=True)
            util = row["max_utilization"]
            if util is not None and util >= self.cfg.max_utilization:
                raise StopRun(f"account utilisation {util:.2f} >= guard {self.cfg.max_utilization}")
            if row["status"] == "isolation_error":
                raise StopRun(f"{item.run_id}: isolation check failed: {row['isolation_problems']}")
            if row["status"] == "usage_limit":
                raise StopRun(f"{item.run_id}: usage limit reached")
            if row["final"]:
                return
            time.sleep(min(300, 60 * attempt))  # transient: back off, then retry
        print(f"giving up on {item.run_id} after {self.cfg.max_retries + 1} attempts", flush=True)

    def run(self, only_tasks: set[str] | None = None, reps: list[int] | None = None,
            conditions: list[str] | None = None) -> str | None:
        if (self.cfg.schedule == "interleaved" and conditions
                and set(conditions) != set(self.cfg.conditions) and not self.cfg.extra_env):
            # Running one arm alone would put its sessions into another stretch of
            # time than the others' (the #171 caveat); `probe` (mock API) may.
            raise SystemExit("an interleaved run cannot be restricted to some of its conditions "
                             f"({', '.join(conditions)} of {', '.join(self.cfg.conditions)}); drop --condition")
        self._check_config()
        self.started_at = time.time()
        (self.cache / "tmp").mkdir(parents=True, exist_ok=True)
        done = self.completed()
        items = [it for it in self.plan()
                 if it.run_id not in done
                 and (not only_tasks or it.task["id"] in only_tasks)
                 and (not reps or it.rep in reps)
                 and (not conditions or it.condition in conditions)]
        order = self.run_dir / "plan.txt"
        if not order.exists():  # the planned order, for the record (#182)
            order.write_text("".join(f"{it.run_id}\n" for it in self.plan()))
        if any(self.repos[it.repo].get("isolated") for it in items):
            self._pin_claude()
            if not self.cfg.extra_env:  # a real run, not the mock API
                self._check_login(next(it for it in items if self.repos[it.repo].get("isolated")))
        # Before the first session: every wrapper distribution of the run is on disk (#183).
        missing = {p for it in items if (self.cache / "repos" / it.repo).is_dir()
                   and (p := wrapper_problem(self.cache / "repos" / it.repo, self.repos[it.repo].get("session_env")))}
        if missing:
            raise SystemExit("\n".join(sorted(missing)))
        print(f"{len(done)} runs already final, {len(items)} to go "
              f"(concurrency {self.cfg.concurrency})", flush=True)
        sched = Scheduler(items, work_names={n: config.work_name(r) for n, r in self.repos.items()})
        threads = [threading.Thread(target=self._worker, args=(sched,), daemon=True)
                   for _ in range(max(1, self.cfg.concurrency))]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        if sched.stop_reason:
            print(f"STOPPED: {sched.stop_reason}", flush=True)
        return sched.stop_reason


_SHELL_SPLIT = re.compile(r"[\s;&|()`$<>]+")


def myc_cli_calls(tool_calls: list[dict]) -> list[str]:
    """The ``myc`` CLI runs in a session's Bash calls: one entry per Bash call
    that runs ``myc`` (any word whose basename is ``myc``), the subcommand
    (the next word) or ``""``."""
    out = []
    for call in tool_calls:
        if call.get("name") != "Bash":
            continue
        cmd = str((call.get("input") or {}).get("command") or "")
        words = [w for w in _SHELL_SPLIT.split(cmd) if w]
        for i, w in enumerate(words):
            if w.rsplit("/", 1)[-1] == "myc":
                nxt = words[i + 1] if i + 1 < len(words) else ""
                out.append(nxt if re.fullmatch(r"[a-z][a-z-]*", nxt) else "")
                break
    return out


def hook_stats(log_dir: Path) -> dict:
    """What the setup-claude hooks did in a session, from the wrapper's logs.

    ``hook_grep_calls`` counts PreToolUse runs of ``myc hook-grep`` (one per
    Grep call), ``hook_grep_hints`` the runs that added a hint to the agent's
    context; ``hook_grep_silent_reasons`` tallies why the others stayed silent
    (``MYC_HOOK_DEBUG``). ``session_hook`` is the SessionStart hook's output.
    """
    out = {"hook_grep_calls": 0, "hook_grep_hints": 0, "hook_grep_silent_reasons": {},
           "session_hook": None, "format_hook_calls": 0}
    if not log_dir.is_dir():
        return out
    reasons: dict[str, int] = {}
    for f in sorted(log_dir.glob("hook-grep-*.out")):
        out["hook_grep_calls"] += 1
        if f.read_text().strip():
            out["hook_grep_hints"] += 1
        else:
            err = f.with_suffix(".err")
            why = err.read_text().strip() if err.exists() else ""
            why = re.sub(r'".*?"|\[.*?\]|/\S+', "…", why.replace("myc hook-grep: silent: ", ""))[:80] or "?"
            reasons[why] = reasons.get(why, 0) + 1
    out["hook_grep_silent_reasons"] = dict(sorted(reasons.items()))
    sessions = sorted(log_dir.glob("session-*.out"))
    if sessions:
        out["session_hook"] = sessions[0].read_text().strip()[:200]
    out["format_hook_calls"] = len(list(log_dir.glob("format-hook-*.out")))
    return out


def _kill_group(proc: subprocess.Popen) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=15)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()
