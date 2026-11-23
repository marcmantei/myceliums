"""Post-hoc search audit of a session (#178): did the agent search anyway?

Arm D blocks search up front (allowlist hook, denied tools); this audit reads
the transcript afterwards and counts what got through or was tried, so a D
run can show its isolation held rather than assume it. It runs on every
session (in the other arms it simply measures how much the agent searched).

Per session:

* ``grep_glob_calls``: ``Grep`` / ``Glob`` tool calls (0 in D if the tool
  denial held);
* ``search_attempts``: Bash calls :func:`aeval.bashcmd.classify` calls
  search; ``search_blocked``: those the hook refused; ``search_leaks``: the
  rest, which ran;
* ``interpreted_calls`` / ``interpreted_ran``: Bash calls that run code
  through an interpreter (``python -c``, heredocs, ``node -e``), whose intent
  the classifier cannot see (in D the allowlist refuses them unless the task
  allows the interpreter);
* ``reads``, ``files_read``, ``max_reads_per_turn``;
* ``blind_reads``: ``Read`` calls of a file whose name (basename) appeared
  nowhere before: not in the prompt, an earlier tool input or result, or an
  earlier reply. Reading many such files is search by reading. The test is
  lenient (a basename mentioned anywhere counts as known);
* ``flagged``: any Grep/Glob call, search leak or interpreter run, or at least
  :data:`BLIND_READ_LIMIT` blind reads.
"""

from __future__ import annotations

import json
import os

from .bashcmd import classify, interpreted

DENY_MARKERS = ("This session has no search", "Searching is not available")
BLIND_READ_LIMIT = 5


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(str(b.get("text") or "") for b in content if isinstance(b, dict))
    return ""


def audit(path: str, prompt: str = "") -> dict:
    known = [prompt or ""]
    calls: dict[str, dict] = {}
    out = {"grep_glob_calls": 0, "bash_calls": 0, "bash_blocked": 0, "search_attempts": 0, "search_blocked": 0,
           "search_leaks": 0, "interpreted_calls": 0, "interpreted_ran": 0, "reads": 0, "files_read": 0,
           "blind_reads": 0, "max_reads_per_turn": 0}
    files: set[str] = set()
    seen_msgs: dict[str, int] = {}
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            typ = ev.get("type")
            if typ == "assistant":
                msg = ev.get("message") or {}
                mid = msg.get("id") or ""
                for b in msg.get("content") or []:
                    if b.get("type") == "text":
                        known.append(b.get("text") or "")
                    elif b.get("type") != "tool_use":
                        continue
                    else:
                        name, inp = b.get("name"), b.get("input") or {}
                        if name in ("Grep", "Glob"):
                            out["grep_glob_calls"] += 1
                        elif name == "Read":
                            out["reads"] += 1
                            fp = str(inp.get("file_path") or "")
                            files.add(fp)
                            seen_msgs[mid] = seen_msgs.get(mid, 0) + 1
                            if os.path.basename(fp) and not any(os.path.basename(fp) in k for k in known):
                                out["blind_reads"] += 1
                        elif name == "Bash":
                            cmd = str(inp.get("command") or "")
                            calls[b.get("id")] = {"search": classify(cmd) == "search", "interp": interpreted(cmd)}
                            out["bash_calls"] += 1
                        known.append(json.dumps(inp))
            elif typ == "user":
                content = (ev.get("message") or {}).get("content")
                for b in content if isinstance(content, list) else []:
                    if not isinstance(b, dict) or b.get("type") != "tool_result":
                        continue
                    text = _text(b.get("content"))
                    known.append(text)
                    c = calls.get(b.get("tool_use_id"))
                    if c is None:
                        continue
                    blocked = any(m in text for m in DENY_MARKERS)
                    out["bash_blocked"] += blocked
                    if c["search"]:
                        out["search_attempts"] += 1
                        out["search_blocked"] += blocked
                        out["search_leaks"] += not blocked
                    if c["interp"]:
                        out["interpreted_calls"] += 1
                        out["interpreted_ran"] += not blocked
    out["files_read"] = len(files)
    out["max_reads_per_turn"] = max(seen_msgs.values(), default=0)
    out["flagged"] = bool(out["grep_glob_calls"] or out["search_leaks"] or out["interpreted_ran"]
                          or out["blind_reads"] >= BLIND_READ_LIMIT)
    return out


def summarise(rows: list[dict]) -> dict:
    """Totals per condition over rows that carry ``search_audit``."""
    out: dict[str, dict] = {}
    for r in rows:
        a = r.get("search_audit")
        if not a:
            continue
        s = out.setdefault(r["condition"], {"sessions": 0, "flagged": 0})
        s["sessions"] += 1
        s["flagged"] += bool(a.get("flagged"))
        for k, v in a.items():
            if k != "flagged":
                s[k] = s.get(k, 0) + v
    return out
