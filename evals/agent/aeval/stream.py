"""Parse a ``claude -p --output-format stream-json --verbose`` transcript.

The transcript is one JSON object per line. The events used here:

* ``{"type": "system", "subtype": "init", ...}`` - model, tools, MCP servers;
  checked against the expected isolation before a run is accepted.
* ``{"type": "assistant", "message": {"content": [...]}}`` - text and
  ``tool_use`` blocks (tool calls are counted from these).
* ``{"type": "user", "message": {"content": [{"type": "tool_result", ...}]}}``
  - tool results, used to count tool errors.
* ``{"type": "rate_limit_event", ...}`` - account utilisation; the runner stops
  scheduling new sessions above a configurable threshold.
* ``{"type": "result", ...}`` - final text, cumulative usage, cost, turns.
"""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass, field

MCP_PREFIX = "mcp__myceliums__"


@dataclass
class Transcript:
    init: dict | None = None
    result: dict | None = None
    texts: list[str] = field(default_factory=list)
    tool_calls: list[dict] = field(default_factory=list)  # {"id", "name", "input"}
    tool_errors: Counter = field(default_factory=Counter)
    rate_limits: list[dict] = field(default_factory=list)
    first_turn_input_tokens: int | None = None  # prompt size of the first model call
    n_lines: int = 0
    bad_lines: int = 0

    # ---- derived -----------------------------------------------------------
    @property
    def final_text(self) -> str:
        if self.result and isinstance(self.result.get("result"), str):
            return self.result["result"]
        return self.texts[-1] if self.texts else ""

    @property
    def tool_counts(self) -> Counter:
        return Counter(c["name"] for c in self.tool_calls)

    @property
    def max_utilization(self) -> float | None:
        vals = []
        for ev in self.rate_limits:
            info = ev.get("rate_limit_info") or {}
            if isinstance(info.get("utilization"), (int, float)):
                vals.append(float(info["utilization"]))
            for win in (info.get("unifiedWindows") or {}).values():
                if isinstance(win, dict) and isinstance(win.get("utilization"), (int, float)):
                    vals.append(float(win["utilization"]))
        return max(vals) if vals else None

    @property
    def rate_limit_rejected(self) -> bool:
        for ev in self.rate_limits:
            info = ev.get("rate_limit_info") or {}
            if info.get("status") == "rejected":
                return True
        return False

    def usage(self) -> dict:
        """Cumulative token usage over every model the session called."""
        out = {"input_tokens": 0, "cache_creation_input_tokens": 0,
               "cache_read_input_tokens": 0, "output_tokens": 0}
        model_usage = (self.result or {}).get("modelUsage") or {}
        if model_usage:
            for mu in model_usage.values():
                out["input_tokens"] += int(mu.get("inputTokens") or 0)
                out["cache_creation_input_tokens"] += int(mu.get("cacheCreationInputTokens") or 0)
                out["cache_read_input_tokens"] += int(mu.get("cacheReadInputTokens") or 0)
                out["output_tokens"] += int(mu.get("outputTokens") or 0)
        else:
            u = (self.result or {}).get("usage") or {}
            for k in out:
                out[k] = int(u.get(k) or 0)
        out["total_input_tokens"] = (out["input_tokens"] + out["cache_creation_input_tokens"]
                                     + out["cache_read_input_tokens"])
        return out

    def models(self) -> list[str]:
        return sorted(((self.result or {}).get("modelUsage") or {}).keys())


def parse_transcript(path: str) -> Transcript:
    t = Transcript()
    by_id: dict[str, str] = {}
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            t.n_lines += 1
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                t.bad_lines += 1
                continue
            typ = ev.get("type")
            if typ == "system" and ev.get("subtype") == "init":
                t.init = ev
            elif typ == "assistant":
                usage = (ev.get("message") or {}).get("usage") or {}
                if t.first_turn_input_tokens is None and usage:
                    t.first_turn_input_tokens = sum(int(usage.get(k) or 0) for k in (
                        "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
                for block in (ev.get("message") or {}).get("content") or []:
                    if block.get("type") == "text" and block.get("text"):
                        t.texts.append(block["text"])
                    elif block.get("type") == "tool_use":
                        call = {"id": block.get("id"), "name": block.get("name"),
                                "input": block.get("input")}
                        t.tool_calls.append(call)
                        by_id[call["id"]] = call["name"]
            elif typ == "user":
                content = (ev.get("message") or {}).get("content")
                if isinstance(content, list):
                    for block in content:
                        if block.get("type") == "tool_result" and block.get("is_error"):
                            t.tool_errors[by_id.get(block.get("tool_use_id"), "?")] += 1
            elif typ == "rate_limit_event":
                t.rate_limits.append(ev)
            elif typ == "result":
                t.result = ev
    return t


def check_isolation(init: dict | None, required_tools: set[str], expect_mcp: bool,
                    optional_tools: set[str] = frozenset()) -> list[str]:
    """Problems with the session's tool/MCP/context setup; empty when clean.

    The built-in tools must be ``required_tools`` plus any subset of
    ``optional_tools``. With ``expect_mcp`` the session must additionally
    expose myceliums tools (and nothing else), with exactly one connected MCP
    server named ``myceliums``; without it, no MCP server or tool at all.
    """
    if not init:
        return ["no init event in transcript"]
    problems = []
    tools = set(init.get("tools") or [])
    builtin = {t for t in tools if not t.startswith("mcp__")}
    mcp_tools = {t for t in tools if t.startswith("mcp__")}
    if not (required_tools <= builtin <= required_tools | set(optional_tools)):
        problems.append(f"built-in tools {sorted(builtin)} != expected {sorted(required_tools)}"
                        f" (+ optional {sorted(optional_tools)})")
    servers = init.get("mcp_servers") or []
    if expect_mcp:
        if [s.get("name") for s in servers] != ["myceliums"]:
            problems.append(f"MCP servers {servers} != [myceliums]")
        elif servers[0].get("status") != "connected":
            problems.append(f"myceliums MCP server not connected: {servers[0]}")
        if not mcp_tools:
            problems.append("no myceliums tools exposed")
        if any(not t.startswith(MCP_PREFIX) for t in mcp_tools):
            problems.append(f"foreign MCP tools: {sorted(t for t in mcp_tools if not t.startswith(MCP_PREFIX))}")
    else:
        if servers:
            problems.append(f"unexpected MCP servers: {servers}")
        if mcp_tools:
            problems.append(f"unexpected MCP tools: {sorted(mcp_tools)[:5]}")
    for key in ("slash_commands", "skills"):
        if init.get(key):
            problems.append(f"{key} leaked into the session: {init.get(key)[:5]}")
    return problems
