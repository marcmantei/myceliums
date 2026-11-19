"""Prefetch a myceliums context pack and hand it to a Claude Agent SDK agent.

Layer 5 of the agent integration guide (docs/guides/agent-integration.md):
instead of hoping the agent discovers the myceliums tools, the harness asks
the code graph for the symbols relevant to the task *before* the model call
and puts them in the prompt. The agent starts with signatures, call edges and
file:line locations, and can still read and grep as usual.

    python3 prefetch.py --repo /path/to/project "Why does login redirect twice?"
    python3 prefetch.py --repo /path/to/project --dry-run "..."   # print the prompt only

Requirements: `myc` on PATH and an index of the project (`myc analyze .`).
Without --dry-run also `pip install claude-agent-sdk` (Python 3.10+) and
Claude Code credentials. API reference:
https://code.claude.com/docs/en/agent-sdk/python
"""

from __future__ import annotations

import argparse
import asyncio
import json
import subprocess
import sys


def fetch_context_pack(task: str, repo: str, budget: int) -> dict | None:
    """Run `myc context --json` in the project and return the parsed pack.

    `myc context` picks the index that covers its working directory, so it
    runs with cwd=repo. Returns None when there is no index or myc fails: a
    prefetch is an optimisation, and the agent can still work without it.
    """
    try:
        proc = subprocess.run(
            ["myc", "context", "--json", "--budget", str(budget), "--", task],
            cwd=repo,
            capture_output=True,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as err:
        print(f"myceliums prefetch skipped: {err}", file=sys.stderr)
        return None
    if proc.returncode != 0:
        print(f"myceliums prefetch skipped: {proc.stderr.strip()}", file=sys.stderr)
        return None
    return json.loads(proc.stdout)


def render_pack(pack: dict) -> str:
    """Turn the JSON pack into a compact prompt section.

    Fields used (see `myc context --json`): symbols[].{qualified_name, kind,
    file, line, signature, callers, callers_total, callees, callees_total},
    files, freshness.state, truncated.
    """
    lines = []
    for sym in pack["symbols"]:
        lines.append(f"- {sym['qualified_name']} ({sym['kind']}) {sym['file']}:{sym['line']}")
        if sym["signature"]:
            lines.append(f"  {sym['signature']}")
        for label, refs, total in (
            ("called by", sym["callers"], sym["callers_total"]),
            ("calls", sym["callees"], sym["callees_total"]),
        ):
            if total:
                shown = ", ".join(f"{r['name']} ({r['file']}:{r['line']})" for r in refs)
                more = f", +{total - len(refs)} more" if total > len(refs) else ""
                lines.append(f"  {label}: {shown}{more}")
    if pack["files"]:
        lines.append("Files: " + ", ".join(pack["files"]))
    if pack.get("freshness", {}).get("state") == "behind":
        # The index predates HEAD: say so, so the agent verifies by reading.
        lines.append("Note: the index is older than HEAD; confirm locations by reading the files.")
    return "\n".join(lines)


def build_prompt(task: str, pack: dict | None) -> str:
    """The task, preceded by the context pack when there is one."""
    if not pack or not pack["symbols"]:
        return task
    return (
        "Code graph context for this task, from the project's myceliums index. "
        "It lists likely relevant symbols with their call edges; read the code "
        "to confirm before relying on it.\n"
        f"<codebase_context>\n{render_pack(pack)}\n</codebase_context>\n\n"
        f"Task: {task}"
    )


async def run_agent(prompt: str, repo: str, model: str | None) -> None:
    """Run one Agent SDK session on the prompt and print the final answer."""
    # Imported here so --dry-run works without the SDK installed.
    from claude_agent_sdk import ClaudeAgentOptions, ResultMessage, query

    options = ClaudeAgentOptions(
        cwd=repo,
        # Read-only exploration: these run without a permission prompt.
        allowed_tools=["Read", "Grep", "Glob"],
        max_turns=20,
        model=model,
    )
    async for message in query(prompt=prompt, options=options):
        if isinstance(message, ResultMessage):
            print(message.result if not message.is_error else f"error: {message.subtype}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("task", help="the question or task for the agent")
    parser.add_argument("--repo", default=".", help="project directory (indexed with myc analyze)")
    parser.add_argument("--budget", type=int, default=1500, help="context pack size in estimated tokens")
    parser.add_argument("--model", default=None, help="model for the agent (default: the SDK's default)")
    parser.add_argument("--dry-run", action="store_true", help="print the prompt, do not call the model")
    args = parser.parse_args()

    pack = fetch_context_pack(args.task, args.repo, args.budget)
    prompt = build_prompt(args.task, pack)
    if args.dry_run:
        print(prompt)
        return
    asyncio.run(run_agent(prompt, args.repo, args.model))


if __name__ == "__main__":
    main()
