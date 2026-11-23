"""What a probe session was given, read from the mock API's request log.

``agent_eval.py probe`` runs a session against :mod:`aeval.mockapi`; this
module turns the recorded request bodies and the transcript into findings and
flags the ones that would make a variant different from what it claims to be.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from . import config
from .stream import parse_transcript

USER_MEMORY = str(Path(os.path.expanduser("~")) / ".claude" / "CLAUDE.md")


def _texts(value) -> list[str]:
    """Every string inside a request body."""
    out: list[str] = []
    if isinstance(value, str):
        out.append(value)
    elif isinstance(value, dict):
        for v in value.values():
            out.extend(_texts(v))
    elif isinstance(value, list):
        for v in value:
            out.extend(_texts(v))
    return out


def _agents_offered(text: str) -> list[str]:
    """Agent types in the "Available agent types for the Agent tool" list."""
    head = "Available agent types for the Agent tool:"
    if head not in text:
        return []
    out = []
    for line in text.split(head, 1)[1].splitlines()[1:]:
        if not line.startswith("- "):
            break
        out.append(line[2:].split(":", 1)[0].strip())
    return out


def _tool_result_text(requests: list[dict], tool_use_id: str) -> str:
    """The content of the tool result for ``tool_use_id`` in the last request."""
    for r in reversed(requests):
        for m in r.get("messages") or []:
            for b in m.get("content") if isinstance(m.get("content"), list) else []:
                if isinstance(b, dict) and b.get("type") == "tool_result" and b.get("tool_use_id") == tool_use_id:
                    return "\n".join(_texts(b.get("content")))
    return ""


def summarise(run_dir: Path, condition: str) -> dict:
    variant = config.VARIANTS[condition]
    requests = [json.loads(p.read_text()) for p in sorted((run_dir / "mock").glob("request*.json"))]
    main = [r for r in requests if r.get("tools") and "myceliums code graph" not in json.dumps(r.get("system"))]
    sub = [r for r in requests if r.get("tools") and "myceliums code graph" in json.dumps(r.get("system"))]
    first = main[0] if main else {}
    blob = "\n".join(_texts(first.get("messages")) + _texts(first.get("system")))
    later = "\n".join(t for r in main[1:] for t in _texts(r.get("messages")))
    tools = first.get("tools") or []
    names = [t.get("name") for t in tools]
    deferred = sorted(t.get("name") for t in tools if t.get("defer_loading"))
    agent_tool = next((t for t in tools if t.get("name") == "Agent"), None)
    sessions = sorted((run_dir / "sessions").glob("*/attempt1.jsonl"))
    t = parse_transcript(str(sessions[0])) if sessions else None
    hooks_dir = next(iter(sorted((run_dir / "sessions").glob("*/hooks"))), None)
    hook_logs = sorted(p.name for p in hooks_dir.glob("*.out")) if hooks_dir else []
    hook_out = {p.name: p.read_text().strip()[:300] for p in hooks_dir.glob("*.out")} if hooks_dir else {}
    hook_err = {p.name: p.read_text().strip()[:300] for p in hooks_dir.glob("*.err")} if hooks_dir else {}
    findings = {
        "condition": condition,
        "requests": len(requests),
        "main_requests": len(main),
        "subagent_requests": len(sub),
        "user_claude_md_loaded": USER_MEMORY in blob,
        "project_claude_md_loaded": "BEGIN myceliums" in blob or "Code navigation: myceliums" in blob,
        "tools": names,
        "deferred_tools": deferred,
        "myc_tools_with_schema": sorted(n for n in names if n and n.startswith("mcp__myceliums__")
                                        and n not in deferred),
        "agents_offered": _agents_offered(blob) if agent_tool else None,
        "subagent_tools": sorted(x.get("name") for x in (sub[0].get("tools") or [])) if sub else None,
        "grep_hint_in_context": "in the myceliums index" in later,
        "session_hook_text_in_context": "[myceliums]" in blob or "[myceliums]" in later,
        "hook_logs": hook_logs,
        "hook_outputs": hook_out,
        "hook_stderr": hook_err,
        "init_tools": (t.init or {}).get("tools") if t else None,
        "init_agents": (t.init or {}).get("agents") if t else None,
        "init_mcp": (t.init or {}).get("mcp_servers") if t else None,
        "init_api_key_source": (t.init or {}).get("apiKeySource") if t else None,
        "tool_calls": [(c["name"], c.get("parent")) for c in t.tool_calls] if t else None,
        "hook_events": len(t.hook_events) if t else None,
        "bash_result": _tool_result_text(main, "toolu_probe_bash")[:400] or None,
        # The hook's refusal echoes its own command line first (long with a task's
        # --allow-json), so keep enough to reach the message.
        "search_result": _tool_result_text(main, "toolu_probe_search")[:3000] or None,
        # #183: the task's test command, which D's allowlist must let run.
        "test_result": _tool_result_text(main, "toolu_probe_test")[:300] or None,
    }
    problems = []
    if findings["user_claude_md_loaded"]:
        problems.append("~/.claude/CLAUDE.md reached the session")
    if variant.setup_claude != findings["project_claude_md_loaded"]:
        problems.append(f"routing block loaded={findings['project_claude_md_loaded']}, expected {variant.setup_claude}")
    if variant.agent:
        if findings["agents_offered"] != [config.EXPLORER_AGENT]:
            problems.append(f"Agent tool offers {findings['agents_offered']}, expected only {config.EXPLORER_AGENT}")
        if not findings["subagent_tools"] or not any(x.startswith("mcp__myceliums__") for x in findings["subagent_tools"]):
            problems.append(f"subagent got no myceliums tools: {findings['subagent_tools']}")
    elif agent_tool is not None:
        problems.append("Agent tool offered outside variant E")
    if variant.no_search:  # #178: arm D
        offered = sorted(set(names) & set(config.SEARCH_TOOLS))
        if offered:
            problems.append(f"search tools offered in a no-search arm: {offered}")
        if findings["search_result"] is not None and not any(m in findings["search_result"] for m in ("This session has no search", "Searching is not available")):
            problems.append(f"grep in Bash was not blocked: {findings['search_result'][:120]!r}")
    if variant.mcp and not findings["myc_tools_with_schema"]:
        problems.append("no myceliums tool schema in the first request")
    findings["problems"] = problems
    return findings
