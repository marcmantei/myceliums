"""Replay recorded myceliums MCP calls against two ``myc`` builds (#181).

An agent session re-sends every tool result with each later model call, so
the size of a result matters as much as its content. This module measures a
change of answer format offline, without a model:

1. ``extract_calls`` reads the stream-json transcripts of a run and collects
   every ``mcp__myceliums__*`` call with its arguments.
2. ``replay`` serves ``myc mcp`` (stdio) from each repository's working copy
   and sends ``tools/list`` and every recorded call again (``repo_id``
   dropped: the server resolves the repository from its working directory).
   Both builds read the same index, so only the output format differs.
3. ``compare`` checks that both builds give the same answer in substance:
   per call the same hits (file, qualified name, start line, depth or chain
   position, ``via``, ``[imports]`` and ``resolved`` marks), the same totals,
   the same ``Incomplete`` reasons and the same ``Next`` step; error results
   must be the same text. It reads the JSON of ``find_callers`` /
   ``find_callees`` (before #181, or ``detail: "full"``), the bullet lists of
   the older text answers, and the compact lists grouped by file.
4. ``summarize`` counts characters (and ``chars / 4``) per tool, plus
   ``o200k_base`` tokens when ``tiktoken`` is importable. ``o200k_base`` is
   an OpenAI tokenizer, not Claude's: it shows the ratio, not Claude's token
   count. It also sizes the tool definitions as Claude Code sends them to the
   model (name with the ``mcp__myceliums__`` prefix, description, input
   schema) and the server instructions.

Entry point: ``agent_eval.py replay`` (README, "Replaying myceliums calls").
"""

from __future__ import annotations

import json
import re
import subprocess
from collections import Counter
from pathlib import Path

from .stream import MCP_PREFIX, parse_transcript

# ── 1. Calls from transcripts ────────────────────────────────────────────


def extract_calls(runs_roots: list[Path], run_glob: str = "*") -> list[dict]:
    """Every myceliums MCP call in ``<root>/<run>/sessions/<session>/attempt*.jsonl``.

    ``repo`` is the session's task id up to its first ``-`` (``django-…``,
    ``vue-…``), the key of ``--work``.
    """
    calls = []
    for root in runs_roots:
        for path in sorted(Path(root).glob(f"{run_glob}/sessions/*/attempt*.jsonl")):
            session = path.parent.name
            t = parse_transcript(str(path))
            for call in t.tool_calls:
                name = str(call.get("name") or "")
                if not name.startswith(MCP_PREFIX):
                    continue
                calls.append({"run": path.parents[2].name, "session": session,
                              "repo": session.split("-", 1)[0], "tool": name[len(MCP_PREFIX):],
                              "input": call.get("input") or {}})
    return calls


# ── 2. Replay ────────────────────────────────────────────────────────────


class McpClient:
    """A minimal JSON-RPC client for ``myc mcp`` over stdio."""

    def __init__(self, myc: str, cwd: Path, env: dict):
        self.proc = subprocess.Popen([myc, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, env=env, cwd=str(cwd), text=True)
        self.next_id = 0
        init = self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                           "clientInfo": {"name": "aeval-replay", "version": "0"}})
        self.instructions = (init.get("result") or {}).get("instructions") or ""
        self.notify("notifications/initialized")

    def notify(self, method: str) -> None:
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError(f"myc mcp exited during {method}")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:
                return reply

    def close(self) -> None:
        self.proc.stdin.close()
        self.proc.wait(timeout=30)


def _env(data: Path) -> dict:
    # Never the user's ~/.myceliums: both the data dir and HOME point at `data`.
    return {"HOME": str(data), "MYCELIUMS_DATA_DIR": str(data), "PATH": "/usr/bin:/bin", "RUST_LOG": "error"}


def index(myc: str, work: dict[str, Path], data: Path) -> None:
    """``myc analyze --skip-embeddings`` of every working copy into ``data``."""
    for name, path in work.items():
        res = subprocess.run([myc, "analyze", str(path), "--skip-embeddings"], env=_env(data),
                             capture_output=True, text=True)
        if res.returncode != 0:
            raise SystemExit(f"myc analyze failed for {name}:\n{res.stdout}{res.stderr}")


def replay(myc: str, calls: list[dict], work: dict[str, Path], data: Path) -> dict:
    """Send ``tools/list`` and every call whose repository is in ``work``."""
    out = {"myc": myc, "tools": [], "instructions": "", "answers": []}
    for repo, path in work.items():
        client = McpClient(myc, path, _env(data))
        try:
            out["tools"] = client.request("tools/list", {})["result"]["tools"]
            out["instructions"] = client.instructions
            for i, call in enumerate(calls):
                if call["repo"] != repo:
                    continue
                args = {k: v for k, v in call["input"].items() if k != "repo_id"}
                reply = client.request("tools/call", {"name": call["tool"], "arguments": args})
                if "error" in reply:
                    text, is_error = json.dumps(reply["error"], sort_keys=True), True
                else:
                    result = reply["result"]
                    text = "".join(b.get("text", "") for b in result.get("content") or []
                                   if b.get("type") == "text")
                    is_error = bool(result.get("isError"))
                out["answers"].append({"i": i, "tool": call["tool"], "args": args,
                                       "text": text, "is_error": is_error})
        finally:
            client.close()
    out["answers"].sort(key=lambda a: a["i"])
    return out


# ── 3. Same answer in substance ──────────────────────────────────────────

# `  * Name (Kind) path:line …`, `  -> Name (Kind) path:line …`, chain start lines.
_SYMBOL_LINE = re.compile(r"^\s+(?:\* |-> )?(\S+) \(([^)]+)\) (\S+):(\d+)(.*)$")
# A compact hit under its file heading: `  Name Kind :start[-end] …`.
_COMPACT_LINE = re.compile(r"^  (\S+) (\w+) :(\d+)(?:-\d+)?(.*)$")
_COUNT_HEADER = re.compile(r"^(\d+) (?:callers|callees) of ")
_TOTAL_HEADERS = ("Impact of changing", "Impact (risk", "Shortest call chain", "No call chain",
                  "Every symbol between them")


def _marks(rest: str) -> tuple:
    """Depth or chain numbers, ``via``, ``[imports]`` and ``resolved`` of a hit's tail."""
    ints: tuple = ()
    via = None
    for bracket in re.findall(r"\[([^\]]*)\]", rest):
        if bracket == "imports":
            continue
        ints = tuple(int(n) for n in re.findall(r"\d+", bracket.split(" via ")[0]))
        m = re.search(r"via (\S+)", bracket)
        via = m.group(1).rstrip(",") if m else None
    resolved = re.search(r"\(resolved: ([a-z-]+)\)", rest)
    return ints, via, "[imports]" in rest, resolved.group(1) if resolved else None


def _json_answer(text: str) -> dict | None:
    try:
        data = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return None
    return data if isinstance(data, dict) and ("callers" in data or "callees" in data) else None


def substance(text: str) -> dict:
    """What an answer says, independent of its format."""
    data = _json_answer(text)
    if data is not None:
        hits = []
        for h in data.get("callers", data.get("callees")) or []:
            deep = h["depth"] > 1
            resolved = h.get("resolved") if h.get("resolved") in ("name-match", "ambiguous") else None
            hits.append((h["file_path"], h["qualified_name"], h["start_line"],
                         (h["depth"],) if deep else (), h.get("via") if deep else None, False, resolved))
        inc = data.get("incomplete") or {}
        return {"hits": Counter(hits), "totals": [[data.get("total_count")]],
                "reasons": list(inc.get("reasons") or []), "next": inc.get("next")}
    hits: Counter = Counter()
    totals, reasons, nxt = [], [], None
    file = None
    in_incomplete = False
    for line in text.splitlines():
        if line.startswith("Incomplete ("):
            in_incomplete = True
            continue
        if line.startswith("Next: "):
            nxt = line[len("Next: "):]
            in_incomplete = False
            continue
        if in_incomplete and line.startswith("  - "):
            reasons.append(line[4:])
            continue
        in_incomplete = False
        count = _COUNT_HEADER.match(line)
        if count:
            totals.append([int(count.group(1))])
        elif line.startswith(_TOTAL_HEADERS):
            totals.append([int(n) for n in re.findall(r"\d+", line.split("[chain N")[0])])
        m = _SYMBOL_LINE.match(line)
        if m:
            hits[(m.group(3), m.group(1), int(m.group(4)), *_marks(m.group(5)))] += 1
            continue
        m = _COMPACT_LINE.match(line)
        if m and file:
            hits[(file, m.group(1), int(m.group(3)), *_marks(m.group(4)))] += 1
            continue
        if line and not line.startswith(" "):
            # A file heading is a bare path; any other unindented line ends the group.
            file = line if " " not in line and not line.endswith(":") else None
    return {"hits": hits, "totals": totals, "reasons": reasons, "next": nxt}


def compare(before: dict, after: dict) -> dict:
    """Per tool: answers compared and the calls whose substance differs."""
    out: dict[str, dict] = {}
    for b, a in zip(before["answers"], after["answers"]):
        if b["i"] != a["i"]:
            raise ValueError("the two replays answered different calls")
        row = out.setdefault(b["tool"], {"compared": 0, "errors": 0, "differ": []})
        row["compared"] += 1
        if b["is_error"] or a["is_error"]:
            row["errors"] += 1
            if (b["is_error"], b["text"]) != (a["is_error"], a["text"]):
                row["differ"].append({"i": b["i"], "args": b["args"], "what": "error"})
            continue
        sb, sa = substance(b["text"]), substance(a["text"])
        what = [k for k in ("hits", "totals", "reasons", "next") if sb[k] != sa[k]]
        if not sb["hits"] and not sb["totals"]:
            what.append("nothing parsed")
        if what:
            row["differ"].append({"i": b["i"], "args": b["args"], "what": ",".join(what)})
    return out


# ── 4. Sizes ─────────────────────────────────────────────────────────────


def _tokenizer():
    try:
        import tiktoken  # optional; not Claude's tokenizer
    except ImportError:
        return None
    try:
        return tiktoken.get_encoding("o200k_base")
    except Exception:  # noqa: BLE001 - no cached encoding offline
        return None


def _compact(obj) -> str:
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


def model_definitions(tools: list[dict]) -> str:
    """The tool definitions as Claude Code sends them to the model."""
    return _compact([{"name": MCP_PREFIX + t["name"], "description": t.get("description", ""),
                      "input_schema": t.get("inputSchema")} for t in tools])


def summarize(before: dict, after: dict) -> dict:
    enc = _tokenizer()

    def size(text: str) -> dict:
        row = {"chars": len(text), "chars_4": len(text) // 4}
        if enc is not None:
            row["o200k"] = len(enc.encode(text))
        return row

    def add(into: dict, text: str) -> None:
        for k, v in size(text).items():
            into[k] = into.get(k, 0) + v

    out: dict = {"tokenizer": "o200k_base (tiktoken; not Claude's tokenizer)" if enc else None,
                 "definitions": {}, "instructions": {}, "tools_list": {}, "per_tool_definition": {},
                 "answers": {}, "answers_total": {}}
    for side, data in (("before", before), ("after", after)):
        out["definitions"][side] = size(model_definitions(data["tools"]))
        out["instructions"][side] = size(data["instructions"])
        out["tools_list"][side] = size(_compact(data["tools"]))
        for t in data["tools"]:
            out["per_tool_definition"].setdefault(t["name"], {})[side] = \
                size(model_definitions([t]))["chars"]
        total: dict = {}
        for a in data["answers"]:
            row = out["answers"].setdefault(a["tool"], {}).setdefault(side, {"calls": 0})
            row["calls"] += 1
            add(row, a["text"])
            add(total, a["text"])
        out["answers_total"][side] = total
    return out


def _pct(b: int, a: int) -> str:
    return f"{(a - b) / b * 100:+.0f} %" if b else "–"


def render(summary: dict, comparison: dict, meta: dict) -> str:
    """REPLAY.md: what was replayed, the sizes, and the equivalence check."""
    tok = "o200k" if summary.get("tokenizer") else None
    lines = [f"# Replayed myceliums calls: {meta.get('title', '')}".rstrip(": "), "",
             f"Calls from the transcripts of {', '.join(meta.get('runs', []))}, replayed with "
             f"`agent_eval.py replay` (`aeval/replay.py`) against two `myc` builds on the same "
             f"indexes: before `{meta.get('before', '?')}`, after `{meta.get('after', '?')}`. "
             f"Indexes: {meta.get('indexes', '?')}.", ""]
    if tok:
        lines += ["Token columns use `o200k_base` (tiktoken), an OpenAI tokenizer, **not Claude's**; "
                  "they show the ratio, not Claude's token count.", ""]
    head = "| | chars before | chars after | change | chars/4 before | chars/4 after |"
    sep = "|---|---:|---:|---:|---:|---:|"
    if tok:
        head += " o200k before | o200k after |"
        sep += "---:|---:|"

    def row(label, b, a, extra=""):
        cells = f"| {label}{extra} | {b['chars']:,} | {a['chars']:,} | {_pct(b['chars'], a['chars'])} " \
                f"| {b['chars_4']:,} | {a['chars_4']:,} |"
        if tok:
            cells += f" {b.get('o200k', 0):,} | {a.get('o200k', 0):,} |"
        return cells

    lines += ["## Tool definitions (sent with every model call)", "", head, sep,
              row("tool definitions (name, description, input schema)", summary["definitions"]["before"],
                  summary["definitions"]["after"]),
              row("server instructions", summary["instructions"]["before"], summary["instructions"]["after"]),
              row("`tools/list` JSON", summary["tools_list"]["before"], summary["tools_list"]["after"]), "",
              "| tool | definition chars before | after |", "|---|---:|---:|"]
    for name, d in sorted(summary["per_tool_definition"].items()):
        lines.append(f"| `{name}` | {d.get('before', 0):,} | {d.get('after', 0):,} |")
    lines += ["", "## Answers", "", head.replace("| |", "| tool |"), sep]
    for name, d in sorted(summary["answers"].items()):
        lines.append(row(f"`{name}`", d["before"], d["after"], f" ({d['before']['calls']} calls)"))
    lines += [row("**total**", summary["answers_total"]["before"], summary["answers_total"]["after"]), "",
              "## Same answer in substance", "",
              "Per call: the same hits (file, qualified name, start line, depth or chain position, `via`, "
              "`[imports]` and `resolved` marks), totals, `Incomplete` reasons and `Next` step; error "
              "results the same text.", "",
              "| tool | calls | errors (same text) | differ |", "|---|---:|---:|---:|"]
    for name, c in sorted(comparison.items()):
        lines.append(f"| `{name}` | {c['compared']} | {c['errors']} | {len(c['differ'])} |")
    for name, c in sorted(comparison.items()):
        for d in c["differ"]:
            lines.append(f"- `{name}` call {d['i']} {json.dumps(d['args'])}: {d['what']}")
    return "\n".join(lines) + "\n"
