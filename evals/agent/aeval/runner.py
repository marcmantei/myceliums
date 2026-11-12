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
import signal
import subprocess
import tempfile
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from . import config
from .grading import extract_answer, grade, is_success
from .stream import MCP_PREFIX, check_isolation, parse_transcript

TRANSIENT_API_STATUS = {408, 409, 429, 500, 502, 503, 504, 529}
TRANSIENT_TEXT = re.compile(
    r"overloaded|internal server error|connection error|ECONNRESET|socket hang up|"
    r"timed out|fetch failed|Request was aborted|API Error: 5\d\d|API Error: 429",
    re.IGNORECASE,
)
USAGE_LIMIT_TEXT = re.compile(r"usage limit|limit reached|out of extra usage", re.IGNORECASE)

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
    "files": (
        "Each answer item is a file path relative to the repository root, for example "
        "`src/server/handler.ts`. Items are paths only: no line numbers or explanations."
    ),
}


def build_prompt(task: dict, repo: dict) -> str:
    return PROMPT_TEMPLATE.format(
        repo=repo["display_name"],
        language=repo["language_name"],
        question=task["question"],
        format_rules=FORMAT_RULES[task["answer_kind"]],
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

    def comparable(self) -> dict:
        """Fields that must not change when a run is resumed."""
        d = asdict(self)
        for k in ("concurrency", "max_retries", "max_utilization", "max_sessions", "claude"):
            d.pop(k)
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
    stop_reason: str | None = None
    cond: threading.Condition = field(default_factory=threading.Condition)

    def next(self) -> WorkItem | None:
        with self.cond:
            while True:
                if self.stop_reason or not self.items:
                    return None
                for i, it in enumerate(self.items):
                    if it.repo not in self.busy:
                        self.busy.add(it.repo)
                        return self.items.pop(i)
                self.cond.wait()

    def done(self, it: WorkItem) -> None:
        with self.cond:
            self.busy.discard(it.repo)
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
        complete paired block over all tasks.
        """
        rng = random.Random(self.cfg.seed)
        items: list[WorkItem] = []
        for rep in range(1, self.cfg.reps + 1):
            order = list(self.tasks)
            rng.shuffle(order)
            for task in order:
                conds = list(config.CONDITIONS)
                rng.shuffle(conds)
                items.extend(WorkItem(task, c, rep) for c in conds)
        return items

    # ---- one session -----------------------------------------------------
    def _prepare_workspace(self, repo: str) -> Path:
        dst = self.work_root / repo
        if dst.exists():
            config.remove_tree(dst, self.work_root)
        config.copy_tree(self.cache / "repos" / repo, dst)
        head = subprocess.run(["git", "-C", str(dst), "rev-parse", "HEAD"],
                              capture_output=True, text=True).stdout.strip()
        if head != self.repos[repo]["commit"]:
            raise StopRun(f"{repo}: checkout HEAD {head} != pinned {self.repos[repo]['commit']}")
        return dst

    def _prepare_mcp(self, item: WorkItem, tmp: Path) -> Path:
        mcp_path = tmp / "mcp.json"
        if item.condition == "without":
            mcp_path.write_text(json.dumps({"mcpServers": {}}))
            return mcp_path
        template = self.cache / "index" / item.repo / "home" / ".myceliums"
        if not (template / "repos.json").exists():
            raise StopRun(f"no index for {item.repo}; run `agent_eval.py index` first")
        data = tmp / "data"
        home = tmp / "home"
        config.copy_tree(template, data)
        home.mkdir()
        registry = json.loads((data / "repos.json").read_text())
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        for info in registry["repos"].values():
            # The index content is unchanged since indexing (same commit, fresh
            # copy); refreshing the timestamp keeps a cache-aware `analyze` call
            # from re-indexing just because the index is older than an hour.
            info["analyzed_at"] = now
        (data / "repos.json").write_text(json.dumps(registry, indent=2))
        server = {
            "type": "stdio",
            "command": self.cfg.myc,
            "args": ["mcp"],
            "env": {
                "MYCELIUMS_DATA_DIR": str(data),
                "HOME": str(home),
                "FASTEMBED_CACHE_DIR": self.cfg.models_dir,
                "RUST_LOG": "warn",
            },
        }
        mcp_path.write_text(json.dumps({"mcpServers": {config.MCP_SERVER_NAME: server}}, indent=2))
        return mcp_path

    def _command(self, item: WorkItem, mcp_path: Path) -> list[str]:
        allowed = list(config.BUILTIN_TOOLS)
        if item.condition == "with":
            allowed.append(f"mcp__{config.MCP_SERVER_NAME}")
        cmd = [
            self.cfg.claude, "-p",
            "--output-format", "stream-json", "--verbose",
            "--model", self.cfg.model,
            "--max-turns", str(self.cfg.max_turns),
            "--tools", ",".join(config.BUILTIN_TOOLS),
            "--allowedTools", ",".join(allowed),
            "--permission-mode", "dontAsk",
            "--strict-mcp-config", "--mcp-config", str(mcp_path),
            "--setting-sources", "",
            "--disable-slash-commands",
            "--no-session-persistence",
        ]
        if self.cfg.effort:
            cmd += ["--effort", self.cfg.effort]
        return cmd

    def _session(self, item: WorkItem, attempt: int) -> dict:
        task = item.task
        repo = self.repos[item.repo]
        prompt = build_prompt(task, repo)
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
        })

        tmp = Path(tempfile.mkdtemp(prefix="aeval-run-", dir=self.cache / "tmp"))
        try:
            workdir = self._prepare_workspace(item.repo)
            mcp_path = self._prepare_mcp(item, tmp)
            (out_dir / "mcp.json").write_text(mcp_path.read_text())
            extra = {}
            if self.cfg.tool_search is not None:
                extra["ENABLE_TOOL_SEARCH"] = self.cfg.tool_search
            env = config.session_env(extra)
            cmd = self._command(item, mcp_path)
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
        finally:
            config.remove_tree(tmp, self.cache / "tmp")
            # The working copy stays until the next session of this repo
            # replaces it; nothing writes to it (the agent has no write tools).

        return self._summarise(item, attempt, transcript, wall, timed_out, proc.returncode)

    def _summarise(self, item: WorkItem, attempt: int, transcript: Path, wall: float,
                   timed_out: bool, returncode: int | None) -> dict:
        t = parse_transcript(str(transcript))
        res = t.result or {}
        problems = check_isolation(t.init, set(config.REQUIRED_TOOLS), item.condition == "with",
                                   set(config.BUILTIN_TOOLS) - set(config.REQUIRED_TOOLS))
        text = t.final_text
        answer = extract_answer(text) or extract_answer("\n".join(t.texts[-3:]))
        g = grade(item.task, answer, self.cfg.success_threshold)

        if problems and t.init:
            status = "isolation_error"
        elif timed_out:
            status = "timeout"
        elif t.rate_limit_rejected or USAGE_LIMIT_TEXT.search(text or "") and res.get("is_error"):
            status = "usage_limit"
        elif not res:
            status = "crash"
        elif res.get("subtype") == "error_max_turns":
            status = "max_turns"
        elif res.get("is_error") or res.get("subtype") != "success":
            api = res.get("api_error_status")
            if (isinstance(api, int) and api in TRANSIENT_API_STATUS) or TRANSIENT_TEXT.search(text or ""):
                status = "api_error"
            else:
                status = "error"
        else:
            status = "ok" if answer is not None else "no_answer"

        counts = t.tool_counts
        mcp_calls = {k[len(MCP_PREFIX):]: v for k, v in counts.items() if k.startswith(MCP_PREFIX)}
        usage = t.usage()
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
            "success": is_success(g.f1, self.cfg.success_threshold),
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
        return row

    # ---- loop ------------------------------------------------------------
    def _worker(self, sched: Scheduler) -> None:
        while True:
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
        self._check_config()
        (self.cache / "tmp").mkdir(parents=True, exist_ok=True)
        done = self.completed()
        items = [it for it in self.plan()
                 if it.run_id not in done
                 and (not only_tasks or it.task["id"] in only_tasks)
                 and (not reps or it.rep in reps)
                 and (not conditions or it.condition in conditions)]
        print(f"{len(done)} runs already final, {len(items)} to go "
              f"(concurrency {self.cfg.concurrency})", flush=True)
        sched = Scheduler(items)
        threads = [threading.Thread(target=self._worker, args=(sched,), daemon=True)
                   for _ in range(max(1, self.cfg.concurrency))]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        if sched.stop_reason:
            print(f"STOPPED: {sched.stop_reason}", flush=True)
        return sched.stop_reason


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
