#!/usr/bin/env python3
"""Agent-level benchmark for myceliums: does the MCP server help a coding agent?

Subcommands (see evals/agent/README.md for the full procedure):

  fetch         shallow-fetch the pinned repositories into the cache
  index         build one myceliums index per repository (myc analyze)
  check-tasks   validate tasks.json and every gold item's evidence line
  run           run the sessions (resumable) for a label
  export        copy the final rows + metadata of a run into results/<label>/
  report        render results/<label>/REPORT.md (--check: verify it is current)
  toolsel-list  print / save the live tools/list of a myc binary
  toolsel-run   run the tool-selection eval
  toolsel-report render the tool-selection report

Python standard library only; nothing here is part of `cargo test`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
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
    prep.fetch(config.load_repos())
    return 0


def cmd_index(args) -> int:
    only = set(args.repo) if args.repo else None
    prep.index(config.load_repos(), args.myc, args.models_dir, only, record_only=args.record_only)
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
    return RunConfig(
        label=args.label, model=args.model, effort=args.effort or None, max_turns=args.max_turns,
        timeout_s=args.timeout, reps=args.reps, concurrency=args.concurrency,
        max_retries=args.max_retries, success_threshold=args.threshold,
        max_utilization=args.max_utilization, max_sessions=args.max_sessions, seed=args.seed,
        tool_search=args.tool_search, claude=config.resolve_claude(args.claude),
        myc=args.myc, models_dir=args.models_dir,
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
    stop = runner.run(only_tasks=only, reps=args.rep or None, conditions=args.condition or None)
    return 2 if stop else 0


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
    with_rows = [r for r in rows if r["condition"] == "with" and r.get("n_mcp_tools_offered")]
    meta = {
        "label": args.label,
        "date": args.date or (first or {}).get("finished_at", "")[:10],
        "model_alias": cfg["model"],
        "model": (first or {}).get("model"),
        "effort": cfg["effort"],
        "max_turns": cfg["max_turns"],
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
    out = report.export(args.label, meta)
    print(f"exported to {out}")
    return 0


def cmd_report(args) -> int:
    ok = report.write_report(args.label, check=args.check)
    if args.check:
        print("REPORT.md is current" if ok else "REPORT.md / summary.json do not match results.jsonl")
        return 0 if ok else 1
    print(f"wrote {config.RESULTS_DIR / args.label / 'REPORT.md'}")
    return 0


def cmd_toolsel_list(args) -> int:
    tools = toolsel.list_tools(args.myc)
    intents = config.load_json(config.INTENTS_FILE)["intents"]
    problems = toolsel.check_intents(intents, tools)
    print(f"{len(tools)} tools, sha256 {toolsel.tools_digest(tools)[:16]}; {len(intents)} intents")
    for p in problems:
        print("PROBLEM:", p)
    if args.out:
        Path(args.out).write_text(json.dumps(tools, indent=1) + "\n")
    return 1 if problems else 0


def cmd_toolsel_run(args) -> int:
    tools = toolsel.list_tools(args.myc)
    intents = config.load_json(config.INTENTS_FILE)["intents"]
    problems = toolsel.check_intents(intents, tools)
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
        "tool_names": [t["name"] for t in tools],
    }
    meta_path = run_dir / "toolsel_meta.json"
    if not meta_path.exists():
        meta_path.write_text(json.dumps(meta, indent=2) + "\n")
    toolsel.run(args.label, intents, args.passes, args.model, args.myc, claude, args.concurrency)
    return 0


def cmd_toolsel_report(args) -> int:
    run_dir = config.RUNS_DIR / args.label
    out_dir = config.RESULTS_DIR / args.label
    if run_dir.exists() and not args.check:
        meta = config.load_json(run_dir / "toolsel_meta.json")
        meta["intents"] = config.load_json(config.INTENTS_FILE)["intents"]
        rows = toolsel.rescore(args.label, meta["intents"])
        meta["model"] = next((r["model"] for r in rows if r.get("model")), None)
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "toolsel_meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
        with open(out_dir / "toolsel_results.jsonl", "w") as fh:
            for r in sorted(rows, key=lambda r: r["key"]):
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
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("fetch").set_defaults(fn=cmd_fetch)

    s = sub.add_parser("index")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--models-dir", default=_default_models())
    s.add_argument("--repo", action="append")
    s.add_argument("--record-only", action="store_true",
                   help="describe an existing index (needs its analyze.log) instead of rebuilding it")
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
    s.set_defaults(fn=cmd_run)

    s = sub.add_parser("export")
    s.add_argument("--label", required=True)
    s.add_argument("--date")
    s.add_argument("--myc-commit", help="commit the myc binary was built from (default: HEAD)")
    s.add_argument("--note", action="append")
    s.set_defaults(fn=cmd_export)

    s = sub.add_parser("report")
    s.add_argument("label")
    s.add_argument("--check", action="store_true")
    s.set_defaults(fn=cmd_report)

    s = sub.add_parser("toolsel-list")
    s.add_argument("--myc", default=_default_myc())
    s.add_argument("--out")
    s.set_defaults(fn=cmd_toolsel_list)

    s = sub.add_parser("toolsel-run")
    s.add_argument("--label", required=True)
    s.add_argument("--model", default="haiku")
    s.add_argument("--passes", type=int, default=2)
    s.add_argument("--concurrency", type=int, default=3)
    s.add_argument("--claude")
    s.add_argument("--myc", default=_default_myc())
    s.set_defaults(fn=cmd_toolsel_run)

    s = sub.add_parser("toolsel-report")
    s.add_argument("label")
    s.add_argument("--check", action="store_true")
    s.set_defaults(fn=cmd_toolsel_report)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
