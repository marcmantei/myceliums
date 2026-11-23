"""Did the agent meet and follow myceliums' Incomplete / Next hints? (#171)

Since #169 a graph walk that stops early says so: the text result carries an
``Incomplete (N branch(es) not followed):`` block that ends in ``Next: …``.
``find_callers`` / ``find_callees`` answered with JSON holding an
``incomplete`` object with a ``next`` field until #181 (the #163/#171
transcripts); since #181 their compact text carries the same block, and the
JSON form is what ``detail: "full"`` returns. Both forms are read. ``Next`` names one or more steps joined by
``; then `` or ``; or ``: a tool call (``find_callers(repo_id=…,
symbol_name=…, depth=3)``, ``trace_flow(…, max_depth=7)``, ``analyze(…,
force=true)``), "<tool> on the symbols at depth N", "read the source at the
lines listed above" or "search the source for `X(` (grep)".

``hint_stats`` reads one transcript and counts the myceliums results (MCP
calls and ``myc`` runs in Bash) that carried such a block, and whether a tool
call made after the result arrived took one of the steps it named. Taking a
step is judged by the kind of call, not its exact arguments:

* a tool call step: a later call of the same myceliums tool (MCP, or the
  matching ``myc`` subcommand in Bash); a CLI step (``myc callers X --depth
  3``, what ``myc`` itself prints): the same subcommand or its MCP tool;
* "<tool> on the symbols at depth N": a later call of that tool;
* "read the source": a later ``Read``, ``Grep`` or Bash call that does not run
  ``myc``;
* "search the source for `X(`": a later ``Grep`` or Bash call whose input
  contains ``X``.

``source_after_hint`` is broader: any ``Read``, ``Grep`` or non-``myc`` Bash
call after the first hinted result, whatever the hint said.
"""

from __future__ import annotations

import json
import re

from .stream import MCP_PREFIX, Transcript

# myc CLI subcommand for each MCP tool a Next step can name.
CLI_FOR_TOOL = {"find_callers": "callers", "find_callees": "callees", "impact_of_change": "impact",
                "trace_flow": "flow", "analyze": "analyze"}
_STEP_SPLIT = re.compile(r";\s+(?:then|or)\s+")
_CALL = re.compile(r"^`?([a-z_]+)\s*\(")
_CLI = re.compile(r"^`?myc\s+([a-z][a-z-]*)")
_ON_SYMBOLS = re.compile(r"^`?([a-z_]+) on the symbols at depth")
_GREP = re.compile(r"search the source for `([^`(]+)\(?`")
_NEXT_LINE = re.compile(r"^\s*Next:\s*(.+)$", re.MULTILINE)
_SHELL_SPLIT = re.compile(r"[\s;&|()`$<>]+")


def _runs_myc(call: dict) -> str | None:
    """The myc subcommand a Bash call runs ('' for none named), or None."""
    if call.get("name") != "Bash":
        return None
    words = [w for w in _SHELL_SPLIT.split(str((call.get("input") or {}).get("command") or "")) if w]
    for i, w in enumerate(words):
        if w.rsplit("/", 1)[-1] == "myc":
            return words[i + 1] if i + 1 < len(words) else ""
    return None


def _is_myc(call: dict) -> bool:
    return str(call.get("name") or "").startswith(MCP_PREFIX) or _runs_myc(call) is not None


def _structured_nexts(text: str) -> list[str]:
    """``incomplete.next`` of a JSON result (find_callers / find_callees)."""
    try:
        data = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return []
    inc = data.get("incomplete") if isinstance(data, dict) else None
    return [inc["next"]] if isinstance(inc, dict) and inc.get("next") else []


def nexts_of(text: str) -> list[str] | None:
    """The Next steps of a result with an Incomplete block; None without one."""
    structured = _structured_nexts(text)
    if structured:
        return structured
    if "Incomplete (" not in text:
        return None
    tail = text[text.index("Incomplete ("):]
    return [m.group(1).strip() for m in _NEXT_LINE.finditer(tail)][:1] or [""]


def step_kind(step: str) -> str:
    s = step.strip()
    if _GREP.search(s):
        return "grep_source"
    if "read the source" in s:
        return "read_source"
    m = _CALL.match(s) or _ON_SYMBOLS.match(s) or _CLI.match(s)
    if m:
        return "full_analysis" if m.group(1) == "analyze" else "call"
    return "other"


def _takes(step: str, call: dict) -> bool:
    s = step.strip()
    name = str(call.get("name") or "")
    sub = _runs_myc(call)
    m = _GREP.search(s)
    if m:
        blob = json.dumps(call.get("input") or {})
        return (name == "Grep" or (name == "Bash" and sub is None)) and m.group(1) in blob
    if "read the source" in s:
        return name in ("Read", "Grep") or (name == "Bash" and sub is None)
    m = _CALL.match(s) or _ON_SYMBOLS.match(s)
    if m:
        tool = m.group(1)
        return name == MCP_PREFIX + tool or (sub is not None and sub == CLI_FOR_TOOL.get(tool, tool))
    m = _CLI.match(s)
    if m:  # a CLI step (`myc callers X --depth 3`): the same subcommand, or its MCP tool
        tools = [t for t, c in CLI_FOR_TOOL.items() if c == m.group(1)]
        return sub == m.group(1) or any(name == MCP_PREFIX + t for t in tools)
    return False


def hint_stats(t: Transcript) -> dict:
    """Per-session counts of Incomplete hints met and Next steps taken."""
    by_id = {c["id"]: c for c in t.tool_calls}
    hinted = []  # (index of the first call made after the result, steps)
    for res in t.tool_results:
        call = by_id.get(res["id"])
        if not call or not _is_myc(call):
            continue
        nexts = nexts_of(res["text"])
        if nexts is None:
            continue
        steps = [s for n in nexts for s in _STEP_SPLIT.split(n) if s.strip()]
        hinted.append((res["after"], steps))
    kinds: dict[str, int] = {}
    followed = 0
    for after, steps in hinted:
        for s in steps:
            kinds[step_kind(s)] = kinds.get(step_kind(s), 0) + 1
        later = t.tool_calls[after:]
        if any(_takes(s, c) for s in steps for c in later):
            followed += 1
    first = min((a for a, _ in hinted), default=None)
    source_after = first is not None and any(
        c.get("name") in ("Read", "Grep") or (c.get("name") == "Bash" and _runs_myc(c) is None)
        for c in t.tool_calls[first:])
    return {"incomplete_results": len(hinted), "next_followed": followed,
            "next_kinds": dict(sorted(kinds.items())), "source_after_hint": bool(source_after)}
