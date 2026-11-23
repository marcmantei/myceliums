#!/usr/bin/env python3
"""Agent-level benchmark for myceliums: does the MCP server help a coding agent?

Subcommands (see evals/agent/README.md for the full procedure):

  fetch         shallow-fetch the pinned repositories into the cache
  index         build one myceliums index per repository (myc analyze)
  check-tasks   validate tasks.json and every gold item's evidence line
  run           run the sessions (resumable) for a label
  export        copy the final rows + metadata of a run into results/<label>/
  report        render results/<label>/REPORT.md (--check: verify it is current)
  tokens        per-tool token attribution and cost of a committed run: TOKENS.md (#178)
  precheck-edit check that each edit task's test command fails before the change (#178)
  probe         check a variant's session setup against a mock API (no model call)
  plan          print a run's session order and how its arms are interleaved (#182)
  toolsel-list  print / save the live tools/list of a myc binary
  toolsel-run   run the tool-selection eval
  toolsel-report render the tool-selection report
  replay        replay a run's myceliums calls against two myc builds: sizes and equivalence (#181)

Python standard library only; nothing here is part of `cargo test`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from aeval import config, prep, report, toolsel  # noqa: E402
from aeval.runner import RunConfig, Runner  # noqa: E402


def _default_myc() -> str:
    return str(config.cache_dir() / "bin" / "myc")


def _default_models() -> str:
    import os
    return os.environ.get("FASTEMBED_CACHE_DIR", str(config.cache_dir() / "models"))


def cmd_fetch(args) -> int:
    repos = config.load_repos()
    if args.repo:
        repos = {k: v for k, v in repos.items() if k in set(args.repo)}
    prep.fetch(repos)
    return 0


def cmd_index(args) -> int:
    only = set(args.repo) if args.repo else None
    prep.index(config.load_repos(), args.myc, args.models_dir, only, record_only=args.record_only,
               default_config=set(args.default_config or []), skip_embeddings=args.skip_embeddings)
    return 0


def cmd_check_tasks(args) -> int:
    tasks = config.load_tasks()
    problems = prep.check_tasks(tasks, config.load_repos(), check_evidence=not args.schema_only)
    for p in problems:
        print("PROBLEM:", p)
    by_type: dict[str, int] = {}
    for t in tasks:
        by_type[t["type"]] = by_type.get(t["type"], 0) + 1
    print(f"{len(tasks)} tasks {by_type}; {len(problems)} problems")
    return 1 if problems else 0


def _run_config(args) -> RunConfig:
    conditions = tuple(args.conditions.split(",")) if args.conditions else ("with", "without")
    for c in conditions:
        if c not in config.CONDITIONS:
            sys.exit(f"unknown condition {c!r}; known: {', '.join(config.CONDITIONS)}")
    max_turns, reps, prompt_budget = args.max_turns, args.reps, getattr(args, "prompt_budget", False)
    profile = getattr(args, "profile", None) or ""
    schedule = getattr(args, "schedule", None) or ""
    if profile:
        # #178: a named profile fixes the limits; explicit flags that disagree are an error.
        p = config.PROFILES[profile]
        if prompt_budget and not p["prompt_budget"]:
            sys.exit(f"--profile {profile} never states the turn limit in the prompt; drop --prompt-budget")
        if schedule and schedule != p.get("schedule", "paired"):
            sys.exit(f"--profile {profile} runs the {p.get('schedule', 'paired')} schedule; drop --schedule")
        max_turns, reps, prompt_budget = p["max_turns"], p["reps"], p["prompt_budget"]
        schedule = p.get("schedule", "paired")
    return RunConfig(
        label=args.label, model=args.model, effort=args.effort or None, max_turns=max_turns,
        timeout_s=args.timeout, reps=reps, concurrency=args.concurrency,
        max_retries=args.max_retries, success_threshold=args.threshold,
        max_utilization=args.max_utilization, max_sessions=args.max_sessions, seed=args.seed,
        tool_search=args.tool_search, claude=config.resolve_claude(args.claude),
        myc=args.myc, models_dir=args.models_dir, conditions=conditions,
        tasks_file=os.environ.get("AEVAL_TASKS", ""), stop_after_s=getattr(args, "stop_after", None),
        prompt_budget=prompt_budget,
        bash=getattr(args, "bash", False),
        profile=profile,
        schedule=schedule or "paired",
        mock_api=getattr(args, "mock_api", False),
    )


def cmd_run(args) -> int:
    cfg = _run_config(args)
    tasks = config.load_tasks()
    repos = config.load_repos()
    problems = prep.check_tasks(tasks, repos)
    if problems:
        print("\n".join(problems))
        return 1
    if not Path(cfg.myc).exists():
        print(f"myc binary not found: {cfg.myc}")
        return 1
    runner = Runner(cfg, tasks, repos)
    only = set(args.task) if args.task else None
    api = None
    if cfg.mock_api:
        # #182: the whole pipeline (schedule, isolation, grading, token rows) against the
        # scripted mock API: no model call, no usage. Each session ends with the task's
        # gold answer (--mock-answers gold) or an empty one.
        from aeval import mockapi
        log_dir = config.RUNS_DIR / cfg.label / "mock"
        log_dir.mkdir(parents=True, exist_ok=True)
        answers = {t["question"]: ([g["name"] for g in t.get("gold", [])] if args.mock_answers == "gold" else [])
                   for t in tasks}
        api = mockapi.MockAPI(log_dir, args.mock_grep, args.mock_callers, answers=answers).start()
        cfg.extra_env = mockapi.session_env(api.url)
    try:
        stop = runner.run(only_tasks=only, reps=args.rep or None, conditions=args.condition or None)
    finally:
        if api is not None:
            api.stop()
    return 2 if stop else 0


def cmd_plan(args) -> int:
    """Print a run's session order without running it (#182): every session in
    order, then per repetition how often each condition takes each position of
    its task's block, and the mean position of each condition in the run."""
    cfg = _run_config(args)
    tasks = config.load_tasks()
    plan = Runner(cfg, tasks, config.load_repos()).plan()
    if args.verbose:
        for i, it in enumerate(plan, 1):
            print(f"{i:4d}  r{it.rep}  {it.condition:<8} {it.task['id']}")
    k = len(cfg.conditions)
    print(f"{len(plan)} sessions: {len(tasks)} tasks x {k} conditions x {cfg.reps} reps; schedule {cfg.schedule}")
    for rep in range(1, cfg.reps + 1):
        rows = [it for it in plan if it.rep == rep]
        counts = {c: [0] * k for c in cfg.conditions}
        for i, it in enumerate(rows):
            counts[it.condition][i % k] += 1
        print(f"  rep {rep}: position counts " + "; ".join(f"{c} {counts[c]}" for c in cfg.conditions))
    mean = {c: sum(i for i, it in enumerate(plan) if it.condition == c) / max(1, sum(
        1 for it in plan if it.condition == c)) for c in cfg.conditions}
    print("  mean index in the run: " + ", ".join(f"{c} {v:.1f}" for c, v in mean.items()))
    return 0


def cmd_probe(args) -> int:
    """Start one session per condition against the scripted mock API (no
    model call, no usage) and report what the session was given."""
    from aeval import mockapi, probe
    tasks = {t["id"]: t for t in config.load_tasks()}
    task = tasks[args.task]
    repos = config.load_repos()
    ok = True
    for cond in args.conditions.split(","):
        cfg = _run_config(args)
        cfg.label = f"probe-{args.label}-{cond}"
        cfg.conditions = (cond,)
        cfg.reps = 1
        cfg.max_retries = 0
        run_dir = config.RUNS_DIR / cfg.label
        if run_dir.exists():
            sys.exit(f"{run_dir} exists; pick another --label")
        log_dir = run_dir / "mock"
        log_dir.mkdir(parents=True)
        api = mockapi.MockAPI(log_dir, args.grep, args.callers).start()
        cfg.extra_env = mockapi.session_env(api.url)
        try:
            Runner(cfg, [task], repos).run(only_tasks={task["id"]}, reps=[1], conditions=[cond])
        finally:
            api.stop()
        findings = probe.summarise(run_dir, cond)
        print(json.dumps(findings, indent=1))
        ok &= not findings.get("problems")
    return 0 if ok else 1


def _git_head(path: Path) -> str:
    return subprocess.run(["git", "-C", str(path), "rev-parse", "--short=12", "HEAD"],
                          capture_output=True, text=True).stdout.strip()


def cmd_export(args) -> int:
    run_dir = config.RUNS_DIR / args.label
    cfg = config.load_json(run_dir / "config.json")
    repos = config.load_repos()
    first = None
    rows = [json.loads(x) for x in (run_dir / "results.jsonl").read_text().splitlines() if x.strip()]
    for r in rows:
        if r.get("model"):
            first = r
            break
    ledger = config.RUNS_DIR / "ledger.jsonl"
    launched = 0
    if ledger.exists():
        launched = sum(1 for x in ledger.read_text().splitlines()
                       if x.strip() and json.loads(x)["label"] == args.label)
    with_rows = [r for r in rows if config.VARIANTS[r["condition"]].mcp and r.get("n_mcp_tools_offered")]
    meta = {
        "label": args.label,
        "date": args.date or (first or {}).get("finished_at", "")[:10],
        "model_alias": cfg["model"],
        "model": (first or {}).get("model"),
        "effort": cfg["effort"],
        "max_turns": cfg["max_turns"],
        **({"prompt_budget": True} if cfg.get("prompt_budget") else {}),
        **({"bash": True} if cfg.get("bash") else {}),
        **({"profile": cfg["profile"]} if cfg.get("profile") else {}),
        **({"schedule": cfg["schedule"]} if cfg.get("schedule") else {}),
        **({"mock_api": True} if cfg.get("mock_api") else {}),
        # #182: an interleaved run compares every pair of its conditions, without a baseline.
        **({"comparisons": "pairwise"} if cfg.get("schedule") == "interleaved" else {}),
        "timeout_s": cfg["timeout_s"],
        "reps": cfg["reps"],
        "success_threshold": cfg["success_threshold"],
        "tool_search": cfg["tool_search"],
        "claude_version": (first or {}).get("claude_version"),
        "myc_version": config.tool_version([cfg["myc"], "--version"]),
        "myc_sha256": config.sha256_file(Path(cfg["myc"])),
        "myc_commit": args.myc_commit or _git_head(config.EVAL_DIR),
        "mcp_tools_offered": with_rows[0]["n_mcp_tools_offered"] if with_rows else None,
        "repos": repos,
        "index": prep.index_meta(),
        "sessions_launched_ledger": launched,
        "notes": args.note or [],
    }
    if cfg.get("conditions"):  # runs other than the pilot's with/without
        meta["conditions"] = cfg["conditions"]
        # --tasks names the file the run used (it may have been renamed since).
        meta["tasks_file"] = os.path.relpath(config.tasks_file(), config.EVAL_DIR)
    out = report.export(args.label, meta)
    print(f"exported to {out}")
    return 0


def cmd_regrade(args) -> int:
    """Re-grade a committed run from its stored answers against the current gold (e.g. after #164)."""
    label = args.label
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    used = meta.get("tasks_file", "tasks/tasks.json")
    current = os.path.relpath(config.tasks_file(), config.EVAL_DIR)
    if used != current:
        sys.exit(f"{label} was run on {used}; pass --tasks evals/agent/{used}")
    changed = report.regrade(label, args.note)
    report.write_report(label)
    print(f"{label}: {changed} sessions changed grade; REPORT.md regenerated")

def cmd_export_scale(args) -> int:
    """Merge the per-budget runs of the outcome eval at scale (#163) into one
    results directory (rows tagged with their budget)."""
    from aeval import report_scale
    runs = args.runs.split(",")
    cfgs = [config.load_json(config.RUNS_DIR / r / "config.json") for r in runs]
    first = None
    for r in runs:
        for x in (config.RUNS_DIR / r / "results.jsonl").read_text().splitlines():
            row = json.loads(x) if x.strip() else {}
            if row.get("model"):
                first = row
                break
        if first:
            break
    with_rows = [json.loads(x) for r in runs for x in (config.RUNS_DIR / r / "results.jsonl").read_text().splitlines()
                 if x.strip() and json.loads(x).get("n_mcp_tools_offered")]
    keep = ("model", "effort", "reps", "success_threshold", "tool_search", "myc", "conditions", "tasks_file", "bash")
    for k in keep:
        vals = {json.dumps(c.get(k)) for c in cfgs}
        if len(vals) > 1:
            sys.exit(f"runs differ in {k}: {vals}")
    cfg = cfgs[0]
    meta = {
        "label": args.label, "date": args.date or (first or {}).get("finished_at", "")[:10],
        "model_alias": cfg["model"], "model": (first or {}).get("model"), "effort": cfg["effort"],
        "reps": cfg["reps"], "success_threshold": cfg["success_threshold"], "tool_search": cfg["tool_search"],
        "bash": bool(cfg.get("bash")), "conditions": cfg["conditions"],
        "claude_version": (first or {}).get("claude_version"),
        "myc_version": config.tool_version([cfg["myc"], "--version"]),
        "myc_sha256": config.sha256_file(Path(cfg["myc"])),
        "myc_commit": args.myc_commit or _git_head(config.EVAL_DIR),
        "mcp_tools_offered": with_rows[0]["n_mcp_tools_offered"] if with_rows else None,
        "repos": {k: v for k, v in config.load_repos().items()
                  if k in {t["repo"] for t in config.load_tasks()}},
        "index": {k: v for k, v in prep.index_meta().items()
                  if k in {t["repo"] for t in config.load_tasks()}},
        "tasks_file": os.path.relpath(config.tasks_file(), config.EVAL_DIR),
        "notes": args.note or [],
    }
    if args.rerun_of:
        # #171: these runs re-run one condition of a committed run; the report
        # compares them with that run's rows, paired by task.
        base = config.load_json(config.RESULTS_DIR / args.rerun_of / "meta.json")
        meta["rerun_of"] = args.rerun_of
        meta["continuity_exclude"] = base.get("primary_exclude") or {}
        if args.primary_exclude_reason:  # the base run's confound still holds: keep its primary analysis
            meta["primary_exclude"] = {k: args.primary_exclude_reason for k in meta["continuity_exclude"]}
    out = report_scale.export(args.label, runs, meta)
    print(f"exported to {out}")
    return 0


def cmd_report(args) -> int:
    ok = report.write_report(args.label, check=args.check)
    if args.check:
        print("REPORT.md is current" if ok else "REPORT.md / summary.json do not match results.jsonl")
        return 0 if ok else 1
    print(f"wrote {config.RESULTS_DIR / args.label / 'REPORT.md'}")
    return 0


def cmd_tokens(args) -> int:
    """Attribute a committed run's tokens per tool (#178) and render TOKENS.md."""
    from aeval import report_tokens
    if not args.check and not args.render_only:
        root = Path(args.runs_root) if args.runs_root else None
        print(f"wrote {report_tokens.collect(args.label, root, args.prices)}")
    ok = report_tokens.write(args.label, check=args.check)
    if args.check:
        print("TOKENS.md is current" if ok else "TOKENS.md / tokens_summary.json do not match tokens.jsonl")
        return 0 if ok else 1
    print(f"wrote {config.RESULTS_DIR / args.label / 'TOKENS.md'}")
    return 0


def cmd_replay(args) -> int:
    """Replay a run's myceliums MCP calls against two builds (#181): REPLAY.md."""
    from aeval import replay
    out_dir = config.RESULTS_DIR / args.label
    summary_path = out_dir / "replay_summary.json"
    if args.check:
        data = json.loads(summary_path.read_text())
        ok = (out_dir / "REPLAY.md").read_text() == replay.render(data["summary"], data["comparison"],
                                                                  data["meta"])
        print("REPLAY.md is current" if ok else "REPLAY.md does not match replay_summary.json")
        return 0 if ok else 1
    work = {}
    for item in args.work:
        name, _, path = item.partition("=")
        work[name] = Path(path).resolve()
    data_dir = Path(args.data).resolve()
    calls = replay.extract_calls([Path(r) for r in args.runs_root], args.run_glob)
    calls = [c for c in calls if c["repo"] in work]
    if args.index:
        replay.index(args.before, work, data_dir)
    before = replay.replay(args.before, calls, work, data_dir)
    after = replay.replay(args.after, calls, work, data_dir)
    if args.answers_dir:
        answers = Path(args.answers_dir)
        answers.mkdir(parents=True, exist_ok=True)
        (answers / "before.json").write_text(json.dumps(before))
        (answers / "after.json").write_text(json.dumps(after))
    summary = replay.summarize(before, after)
    comparison = replay.compare(before, after)
    runs = sorted({c["run"] for c in calls})
    meta = {"title": args.title or args.label, "runs": runs, "calls": len(calls),
            "before": args.before_name or args.before, "after": args.after_name or args.after,
            "indexes": args.indexes or ", ".join(sorted(work))}
    out_dir.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(json.dumps({"meta": meta, "summary": summary, "comparison": comparison},
                                       indent=1, sort_keys=True) + "\n")
    (out_dir / "REPLAY.md").write_text(replay.render(summary, comparison, meta))
    print(f"wrote {out_dir / 'REPLAY.md'}")
    differ = sum(len(c["differ"]) for c in comparison.values())
    print(f"{len(calls)} calls replayed; {differ} differ in substance")
    return 1 if differ else 0


def cmd_precheck_edit(args) -> int:
    """Run every edit task's test command on the pinned (parent) commit and
    record the result in the task file (#178): it must fail there."""
    from aeval import editgrade
    path = config.tasks_file()
    data = config.load_json(path)
    repos = config.load_repos()
    scratch = config.cache_dir() / "tmp"
    scratch.mkdir(parents=True, exist_ok=True)
    bad = 0
    for t in data["tasks"]:
        if t.get("answer_kind") != "diff" or not t.get("test_command"):
            continue
        if args.task and t["id"] not in args.task:
            continue
        checkout = config.cache_dir() / "repos" / t["repo"]
        rec = editgrade.precheck(t, checkout, scratch, repos[t["repo"]]["commit"],
                                 int(t.get("test_timeout_s", 900)))
        t["precheck"] = rec
        bad += not rec["fails_on_parent"]
        print(f"{t['id']}: test command {'fails' if rec['fails_on_parent'] else 'does NOT fail'} on the parent "
              f"(exit {rec['returncode']}{', timed out' if rec['timed_out'] else ''})")
    path.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    return 1 if bad else 0


def _profile_intents(myc: str, toolsets: str) -> tuple[list[dict], list[dict], list[str]]:
    """The profile's tools, the intents it is scored on, and intent problems
    (checked against the full tool list, so renames fail loudly in any profile)."""
    intents = config.load_json(config.INTENTS_FILE)["intents"]
    every = toolsel.list_tools(myc, "all")
    problems = toolsel.check_intents(intents, every)
    tools = every if toolsets == "all" else toolsel.list_tools(myc, toolsets)
    return tools, toolsel.offered_intents(intents, tools), problems


def cmd_toolsel_list(args) -> int:
    tools, intents, problems = _profile_intents(args.myc, args.toolsets)
    print(f"{len(tools)} tools ({args.toolsets}), sha256 {toolsel.tools_digest(tools)[:16]}; "
          f"{len(intents)} intents offered")
    for p in problems:
        print("PROBLEM:", p)
    if args.out:
        Path(args.out).write_text(json.dumps(tools, indent=1) + "\n")
    return 1 if problems else 0


def cmd_toolsel_run(args) -> int:
    tools, intents, problems = _profile_intents(args.myc, args.toolsets)
    if problems:
        print("\n".join(problems))
        return 1
    run_dir = config.RUNS_DIR / args.label
    run_dir.mkdir(parents=True, exist_ok=True)
    claude = config.resolve_claude(args.claude)
    meta = {
        "date": dt.date.today().isoformat(), "model_alias": args.model,
        "myc_version": config.tool_version([args.myc, "--version"]),
        "claude_version": config.tool_version([claude, "--version"]),
        "n_tools": len(tools), "tools_sha256": toolsel.tools_digest(tools),
        "tool_names": [t["name"] for t in tools], "toolsets": args.toolsets,
        "intent_ids": [it["id"] for it in intents],
    }
    meta_path = run_dir / "toolsel_meta.json"
    if not meta_path.exists():
        meta_path.write_text(json.dumps(meta, indent=2) + "\n")
    toolsel.run(args.label, intents, args.passes, args.model, args.myc, claude, args.concurrency,
                args.toolsets)
    return 0


def cmd_toolsel_report(args) -> int:
    run_dir = config.RUNS_DIR / args.label
    out_dir = config.RESULTS_DIR / args.label
    if run_dir.exists() and not args.check:
        meta = config.load_json(run_dir / "toolsel_meta.json")
        intents = config.load_json(config.INTENTS_FILE)["intents"]
        if "intent_ids" in meta:  # a run on a smaller profile scores only its intents
            intents = [it for it in intents if it["id"] in set(meta["intent_ids"])]
        meta["intents"] = intents
        rows = toolsel.rescore(args.label, meta["intents"])
        meta["model"] = next((r["model"] for r in rows if r.get("model")), None)
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "toolsel_meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
        with open(out_dir / "toolsel_results.jsonl", "w") as fh:
            for r in sorted(rows, key=lambda r: r["run_id"]):
                fh.write(json.dumps(r, sort_keys=True) + "\n")
    meta = config.load_json(out_dir / "toolsel_meta.json")
    rows = [json.loads(x) for x in (out_dir / "toolsel_results.jsonl").read_text().splitlines() if x.strip()]
    text, summary = toolsel.render(rows, meta["intents"], meta, args.label)
    target = out_dir / "TOOLSEL.md"
    if args.check:
        ok = target.exists() and target.read_text() == text
        print("TOOLSEL.md is current" if ok else "TOOLSEL.md does not match toolsel_results.jsonl")
        return 0 if ok else 1
    target.write_text(text)
    print(f"wrote {target}")
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--tasks", help="task file (default: the pilot's tasks/tasks.json; "
                   "the variant matrix uses tasks/tasks-hard.json)")
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("fetch")
    s.add_argument("--repo", action="append", help="only these repositories (default: all)")
    s.set_defaults(fn=cmd_fetch)

    s = sub.add_parser("index")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--models-dir", default=_default_models())
    s.add_argument("--repo", action="append")
    s.add_argument("--record-only", action="store_true",
                   help="describe an existing index (needs its analyze.log) instead of rebuilding it")
    s.add_argument("--skip-embeddings", action="store_true",
                   help="#182: no vectors (minutes instead of hours), for `run --mock-api` checks only; "
                        "a real run refuses such an index")
    s.add_argument("--default-config", action="append", metavar="REPO",
                   help="index this repository with myceliums' default configuration, ignoring its "
                        "index_exclude_extra in repos.json (#171: Django with tests/)")
    s.set_defaults(fn=cmd_index)

    s = sub.add_parser("check-tasks")
    s.add_argument("--schema-only", action="store_true", help="skip evidence lines (no checkout needed)")
    s.set_defaults(fn=cmd_check_tasks)

    s = sub.add_parser("run")
    s.add_argument("--label", required=True)
    s.add_argument("--model", default="sonnet")
    s.add_argument("--effort", default="medium", help="'' to leave the CLI default")
    s.add_argument("--max-turns", type=int, default=40)
    s.add_argument("--timeout", type=int, default=900, help="seconds per session")
    s.add_argument("--reps", type=int, default=3)
    s.add_argument("--rep", type=int, action="append", help="only these repetitions")
    s.add_argument("--task", action="append", help="only these task ids")
    s.add_argument("--condition", action="append", choices=config.CONDITIONS, help="only this condition")
    s.add_argument("--concurrency", type=int, default=3)
    s.add_argument("--max-retries", type=int, default=2, help="retries for transient API errors")
    s.add_argument("--threshold", type=float, default=0.8, help="success = F1 >= threshold")
    s.add_argument("--max-utilization", type=float, default=0.97,
                   help="stop scheduling when the account's reported utilisation reaches this")
    s.add_argument("--max-sessions", type=int, help="hard cap on sessions launched by this invocation")
    s.add_argument("--seed", type=int, default=20261112)
    s.add_argument("--tool-search", help="value for ENABLE_TOOL_SEARCH (default: CLI default)")
    s.add_argument("--claude")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--models-dir", default=_default_models())
    s.add_argument("--conditions", help="comma-separated conditions of this run (default with,without; "
                   "variant matrix: A,C,E; calibration: none; myceliums only: D)")
    s.add_argument("--stop-after", type=int, help="start no new session after this many seconds "
                   "(running ones finish), to run a long matrix in bounded chunks")
    s.add_argument("--prompt-budget", action="store_true",
                   help="state the turn limit (--max-turns) in the prompt, identically in every condition (#153)")
    s.add_argument("--bash", action="store_true",
                   help="add the Bash tool in every condition (#163); the myc CLI is then reachable in C")
    s.add_argument("--profile", choices=sorted(config.PROFILES),
                   help="named limits (#178): realistic = --max-turns 100 as a cap only (not in the prompt), "
                        "3 repetitions, interleaved schedule (#182)")
    s.add_argument("--schedule", choices=config.SCHEDULES,
                   help="session order (#182): paired (default) or interleaved (arms' order counterbalanced; "
                        "no --condition subsets); fixed by --profile realistic")
    s.add_argument("--mock-api", action="store_true",
                   help="#182: run every session against the scripted mock API (no model call, no usage) "
                        "to check the pipeline end to end")
    s.add_argument("--mock-answers", choices=("gold", "empty"), default="gold",
                   help="with --mock-api: the final answer of each session (default: the task's gold)")
    s.add_argument("--mock-grep", default="parseAccept", help="with --mock-api: identifier the scripted grep looks for")
    s.add_argument("--mock-callers", default="parseAccept", help="with --mock-api: symbol the scripted myc callers asks")
    s.set_defaults(fn=cmd_run)

    s = sub.add_parser("plan", help="print a run's session order and how the arms are interleaved (#182)")
    s.add_argument("--label", default="plan")
    s.add_argument("--conditions", required=True)
    s.add_argument("--reps", type=int, default=3)
    s.add_argument("--max-turns", type=int, default=40)
    s.add_argument("--seed", type=int, default=20261112)
    s.add_argument("--profile", choices=sorted(config.PROFILES))
    s.add_argument("--schedule", choices=config.SCHEDULES)
    s.add_argument("--verbose", "-v", action="store_true", help="list every session")
    s.set_defaults(model="sonnet", effort="medium", timeout=900, concurrency=1, max_retries=0, threshold=0.8,
                   max_utilization=0.97, max_sessions=None, tool_search=None, claude="claude", myc="",
                   models_dir="", prompt_budget=False, fn=cmd_plan)

    s = sub.add_parser("probe", help="check a variant's session setup against a mock API (no model call)")
    s.add_argument("--label", required=True)
    s.add_argument("--task", required=True)
    s.add_argument("--conditions", required=True)
    s.add_argument("--grep", default="parseAccept", help="identifier the scripted Grep call searches for")
    s.add_argument("--callers", default="parseAccept", help="symbol the scripted subagent asks find_callers for")
    for flag, default in (("--model", "sonnet"), ("--effort", "medium"), ("--seed", 20261112),
                          ("--max-turns", 10), ("--timeout", 300)):
        s.add_argument(flag, default=default, type=type(default))
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--models-dir", default=_default_models())
    s.add_argument("--bash", action="store_true")
    s.add_argument("--prompt-budget", action="store_true")
    s.set_defaults(reps=1, concurrency=1, max_retries=0, threshold=0.8, max_utilization=0.97,
                   max_sessions=None, tool_search=None, claude=None, fn=cmd_probe)

    s = sub.add_parser("export")
    s.add_argument("--label", required=True)
    s.add_argument("--date")
    s.add_argument("--myc-commit", help="commit the myc binary was built from (default: HEAD)")
    s.add_argument("--note", action="append")
    s.set_defaults(fn=cmd_export)

    s = sub.add_parser("regrade", help="re-grade a committed run's stored answers with the current gold")
    s.add_argument("label")
    s.add_argument("--note", required=True, help="what changed in the gold; appended to the report's notes")
    s.set_defaults(fn=cmd_regrade)

    s = sub.add_parser("export-scale", help="merge the per-budget runs of #163 into one results directory")
    s.add_argument("--label", required=True)
    s.add_argument("--runs", required=True, help="comma-separated run labels, one per budget")
    s.add_argument("--rerun-of", help="committed results label whose rows these runs are compared with (#171)")
    s.add_argument("--primary-exclude-reason",
                   help="with --rerun-of: leave the base run's primary_exclude tasks out of the primary analysis "
                        "too, for this reason")
    s.add_argument("--date")
    s.add_argument("--myc-commit")
    s.add_argument("--note", action="append")
    s.set_defaults(fn=cmd_export_scale)

    s = sub.add_parser("report")
    s.add_argument("label")
    s.add_argument("--check", action="store_true")
    s.set_defaults(fn=cmd_report)

    s = sub.add_parser("precheck-edit", help="check that each edit task's test command fails on the pinned "
                       "(parent) commit, and record it in the task file (#178)")
    s.add_argument("--task", action="append", help="only these task ids")
    s.set_defaults(fn=cmd_precheck_edit)

    s = sub.add_parser("tokens", help="per-tool token attribution and cost of a committed run (#178)")
    s.add_argument("label")
    s.add_argument("--runs-root", help="directory holding the run's transcripts (default: evals/agent/runs)")
    s.add_argument("--prices", help="JSON file {model: {input, cache_write_5m, cache_write_1h, cache_read, output}} "
                   "in USD per million tokens, merged over the built-in table")
    s.add_argument("--render-only", action="store_true", help="re-render TOKENS.md from the committed tokens.jsonl")
    s.add_argument("--check", action="store_true", help="verify TOKENS.md matches tokens.jsonl")
    s.set_defaults(fn=cmd_tokens)

    s = sub.add_parser("replay", help="replay a run's myceliums calls against two myc builds (#181)")
    s.add_argument("label", help="results/<label>/ receives REPLAY.md and replay_summary.json")
    s.add_argument("--runs-root", action="append", default=[],
                   help="directory with <run>/sessions/<session>/attempt*.jsonl (repeatable)")
    s.add_argument("--run-glob", default="*", help="which runs under each root (default: all)")
    s.add_argument("--before", help="myc binary of the old format")
    s.add_argument("--after", help="myc binary of the new format")
    s.add_argument("--before-name", help="how REPLAY.md names the old build (default: its path)")
    s.add_argument("--after-name", help="how REPLAY.md names the new build (default: its path)")
    s.add_argument("--work", action="append", default=[],
                   help="REPO=PATH: the working copy the sessions of REPO ran in (repeatable)")
    s.add_argument("--data", help="myceliums data directory holding (or receiving) the indexes")
    s.add_argument("--index", action="store_true", help="first index every --work copy (skip_embeddings)")
    s.add_argument("--indexes", help="how REPLAY.md describes the indexes")
    s.add_argument("--title")
    s.add_argument("--answers-dir", help="also keep every answer of both builds here (large; not committed)")
    s.add_argument("--check", action="store_true", help="verify REPLAY.md matches replay_summary.json")
    s.set_defaults(fn=cmd_replay)

    toolsets_help = ("myc mcp toolset profile to offer (default all; `core` is myc's own default "
                     "and is scored on the intents it can answer)")
    s = sub.add_parser("toolsel-list")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--toolsets", default="all", help=toolsets_help)
    s.add_argument("--out")
    s.set_defaults(fn=cmd_toolsel_list)

    s = sub.add_parser("toolsel-run")
    s.add_argument("--label", required=True)
    s.add_argument("--model", default="haiku")
    s.add_argument("--passes", type=int, default=2)
    s.add_argument("--concurrency", type=int, default=3)
    s.add_argument("--claude")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--toolsets", default="all", help=toolsets_help)
    s.set_defaults(fn=cmd_toolsel_run)

    s = sub.add_parser("toolsel-report")
    s.add_argument("label")
    s.add_argument("--check", action="store_true")
    s.set_defaults(fn=cmd_toolsel_report)

    args = p.parse_args(argv)
    if args.tasks:
        os.environ["AEVAL_TASKS"] = str(Path(args.tasks).resolve())
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
