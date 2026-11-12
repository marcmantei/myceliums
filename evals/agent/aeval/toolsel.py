"""Tool-selection eval: does a model pick the right myceliums tool for an intent?

1. ``list_tools`` starts ``myc mcp`` (empty, throw-away data directory) and
   reads the live ``tools/list``. Every expected tool in ``intents.json`` must
   exist there; a renamed or removed tool fails the eval loudly.
2. ``run`` asks the model once per intent and pass, in a single turn, with the
   myceliums server attached and nothing else: no built-in tools, tool search
   off so every tool definition is in context, and no permission to run any
   tool. The model's first ``tool_use`` is its choice; it is never executed
   (``--permission-mode dontAsk`` denies it, ``--max-turns 1`` ends the
   session).
3. ``score`` reports accuracy per pass and the most frequent confusions.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import re
import subprocess
import tempfile
import threading
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from . import config
from .stream import MCP_PREFIX, parse_transcript

PROMPT = (
    "You are helping a developer with the codebase in the current directory, which has "
    "already been indexed by the myceliums code knowledge graph. Respond to the request "
    "below by calling the single most appropriate tool. Call exactly one tool and do not "
    "write any text first.\n\nRequest: {intent}"
)


def list_tools(myc: str) -> list[dict]:
    with tempfile.TemporaryDirectory(prefix="aeval-tools-") as tmp:
        env = {"MYCELIUMS_DATA_DIR": str(Path(tmp) / "data"), "HOME": tmp,
               "PATH": "/usr/bin:/bin", "RUST_LOG": "error"}
        proc = subprocess.Popen([myc, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, env=env, cwd=tmp, text=True)

        def call(msg: dict) -> dict | None:
            proc.stdin.write(json.dumps(msg) + "\n")
            proc.stdin.flush()
            if "id" not in msg:
                return None
            while True:
                line = proc.stdout.readline()
                if not line:
                    raise RuntimeError("myc mcp closed stdout")
                resp = json.loads(line)
                if resp.get("id") == msg["id"]:
                    if "error" in resp:
                        raise RuntimeError(resp["error"])
                    return resp["result"]

        try:
            call({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                  "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                             "clientInfo": {"name": "aeval", "version": "1"}}})
            call({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools: list[dict] = []
            cursor = None
            req_id = 2
            while True:
                params = {"cursor": cursor} if cursor else {}
                res = call({"jsonrpc": "2.0", "id": req_id, "method": "tools/list", "params": params})
                tools.extend(res.get("tools", []))
                cursor = res.get("nextCursor")
                req_id += 1
                if not cursor:
                    break
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=10)
    return sorted(tools, key=lambda t: t["name"])


def tools_digest(tools: list[dict]) -> str:
    blob = json.dumps([{k: t.get(k) for k in ("name", "description", "inputSchema")} for t in tools],
                      sort_keys=True)
    return hashlib.sha256(blob.encode()).hexdigest()


def check_intents(intents: list[dict], tools: list[dict]) -> list[str]:
    names = {t["name"] for t in tools}
    problems = []
    seen = set()
    for it in intents:
        if it["id"] in seen:
            problems.append(f"{it['id']}: duplicate id")
        seen.add(it["id"])
        if not it.get("expected"):
            problems.append(f"{it['id']}: no expected tool")
        for name in it.get("expected", []):
            if name not in names:
                problems.append(f"{it['id']}: expected tool {name!r} is not in the live tools/list")
    return problems


def run(label: str, intents: list[dict], passes: int, model: str, myc: str, claude: str,
        concurrency: int = 3) -> Path:
    out_dir = config.RUNS_DIR / label
    out_dir.mkdir(parents=True, exist_ok=True)
    results_path = out_dir / "results.jsonl"
    done = set()
    if results_path.exists():
        for line in results_path.read_text().splitlines():
            if line.strip():
                row = json.loads(line)
                done.add(row.get("run_id") or row.get("key"))
    lock = threading.Lock()
    ledger = config.RUNS_DIR / "ledger.jsonl"

    def one(it: dict, p: int) -> None:
        run_id = f"{it['id']}__p{p}"
        if run_id in done:
            return
        with tempfile.TemporaryDirectory(prefix="aeval-toolsel-") as tmp:
            tmpd = Path(tmp)
            (tmpd / "data").mkdir()
            (tmpd / "home").mkdir()
            (tmpd / "cwd").mkdir()
            cfg = {"mcpServers": {config.MCP_SERVER_NAME: {
                "type": "stdio", "command": myc, "args": ["mcp"],
                "env": {"MYCELIUMS_DATA_DIR": str(tmpd / "data"), "HOME": str(tmpd / "home"),
                        "RUST_LOG": "error"}}}}
            (tmpd / "mcp.json").write_text(json.dumps(cfg))
            cmd = [claude, "-p", "--output-format", "stream-json", "--verbose",
                   "--model", model, "--max-turns", "1", "--tools", "",
                   "--permission-mode", "dontAsk",
                   "--strict-mcp-config", "--mcp-config", str(tmpd / "mcp.json"),
                   "--setting-sources", "", "--disable-slash-commands", "--no-session-persistence"]
            env = config.session_env({"ENABLE_TOOL_SEARCH": "false"})
            transcript = out_dir / "sessions" / f"{run_id}.jsonl"
            transcript.parent.mkdir(parents=True, exist_ok=True)
            with lock:
                with open(ledger, "a") as fh:
                    fh.write(json.dumps({"label": label, "run_id": run_id, "attempt": 1, "model": model,
                                         "started": dt.datetime.now(dt.timezone.utc).isoformat()}) + "\n")
            with open(transcript, "w") as fh:
                proc = subprocess.run(cmd, input=PROMPT.format(intent=it["intent"]), stdout=fh,
                                      stderr=subprocess.DEVNULL, text=True, cwd=tmpd / "cwd",
                                      env=env, timeout=300)
        t = parse_transcript(str(transcript))
        first = t.tool_calls[0]["name"] if t.tool_calls else None
        chosen = first[len(MCP_PREFIX):] if first and first.startswith(MCP_PREFIX) else first
        # A pick written out as text instead of a tool call is still a miss,
        # but it is recorded so the report can tell it apart from a wrong pick.
        m = re.search(re.escape(MCP_PREFIX) + r"(\w+)", t.final_text or "") if not t.tool_calls else None
        row = {
            "run_id": run_id, "intent_id": it["id"], "pass": p, "chosen": chosen,
            "expected": it["expected"], "correct": chosen in it["expected"],
            "text_choice": m.group(1) if m else None,
            "n_tools_offered": len((t.init or {}).get("tools") or []),
            "model": (t.init or {}).get("model"), "returncode": proc.returncode,
            # The pick must have been denied, i.e. never executed.
            "permission_denials": len((t.result or {}).get("permission_denials") or []),
            "cost_usd": (t.result or {}).get("total_cost_usd"),
            "max_utilization": t.max_utilization,
        }
        with lock:
            with open(results_path, "a") as fh:
                fh.write(json.dumps(row, sort_keys=True) + "\n")
        print(f"{run_id:<40} chose {chosen!s:<28} expected {it['expected']} -> "
              f"{'OK' if row['correct'] else 'MISS'}", flush=True)

    jobs = [(it, p) for p in range(1, passes + 1) for it in intents]
    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        for fut in [pool.submit(one, it, p) for it, p in jobs]:
            fut.result()
    return results_path


def rescore(label: str, intents: list[dict]) -> list[dict]:
    """Re-derive every row's pick from its transcript (picks are a pure function of it)."""
    by_id = {i["id"]: i for i in intents}
    rows = []
    for line in (config.RUNS_DIR / label / "results.jsonl").read_text().splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        # Rows written before the field was renamed carry "key" (gitleaks reads
        # a "key" field holding an id as a leaked secret).
        row["run_id"] = row.pop("key", None) or row["run_id"]
        t = parse_transcript(str(config.RUNS_DIR / label / "sessions" / f"{row['run_id']}.jsonl"))
        first = t.tool_calls[0]["name"] if t.tool_calls else None
        chosen = first[len(MCP_PREFIX):] if first and first.startswith(MCP_PREFIX) else first
        m = re.search(re.escape(MCP_PREFIX) + r"(\w+)", t.final_text or "") if not t.tool_calls else None
        row.update(chosen=chosen, expected=by_id[row["intent_id"]]["expected"],
                   correct=chosen in by_id[row["intent_id"]]["expected"],
                   text_choice=m.group(1) if m else None)
        rows.append(row)
    return rows


def score(label: str, intents: list[dict], tools_meta: dict) -> tuple[str, dict]:
    rows = [json.loads(line) for line in (config.RUNS_DIR / label / "results.jsonl").read_text().splitlines()
            if line.strip()]
    return render(rows, intents, tools_meta, label)


def render(rows: list[dict], intents: list[dict], tools_meta: dict, label: str) -> tuple[str, dict]:
    by_pass: dict[int, list[dict]] = {}
    for r in rows:
        by_pass.setdefault(r["pass"], []).append(r)
    intent_by_id = {i["id"]: i for i in intents}
    confusions = Counter((r["expected"][0], r["chosen"] or "(no tool call)") for r in rows if not r["correct"])
    per_intent = Counter(r["intent_id"] for r in rows if r["correct"])
    agree = 0
    pairs = 0
    if len(by_pass) >= 2:
        p1 = {r["intent_id"]: r["chosen"] for r in by_pass[min(by_pass)]}
        p2 = {r["intent_id"]: r["chosen"] for r in by_pass[max(by_pass)]}
        for k in p1.keys() & p2.keys():
            pairs += 1
            agree += p1[k] == p2[k]
    L = [f"# Tool-selection eval: {label}", ""]
    L.append(f"- **Date:** {tools_meta.get('date')}")
    L.append(f"- **Model:** `{tools_meta.get('model_alias')}` (resolved `{tools_meta.get('model')}`), "
             f"one turn, Claude Code {tools_meta.get('claude_version')}")
    L.append(f"- **Tools offered:** the live `tools/list` of `{tools_meta.get('myc_version')}` "
             f"({tools_meta.get('n_tools')} tools, sha256 `{tools_meta.get('tools_sha256', '')[:16]}…`), "
             f"as native tool definitions, no built-in tools, tool search off")
    L.append(f"- **Intents:** {len(intents)} natural-language requests, each with one or more acceptable tools "
             f"(`toolsel/intents.json`); a pick is correct if it is one of them")
    L.append("")
    L.append("| Pass | Intents | Correct | Accuracy |")
    L.append("|---:|---:|---:|---:|")
    summary = {"passes": {}}
    for p, rs in sorted(by_pass.items()):
        c = sum(r["correct"] for r in rs)
        L.append(f"| {p} | {len(rs)} | {c} | {100 * c / len(rs):.0f} % |")
        summary["passes"][p] = {"n": len(rs), "correct": c}
    if pairs:
        L.append("")
        L.append(f"The two passes chose the same tool for {agree} of {pairs} intents.")
    as_text = [r for r in rows if not r["chosen"] and r.get("text_choice")]
    if as_text:
        L.append("")
        L.append(f"{len(as_text)} pick(s) were written out as text instead of being made as a tool call; "
                 "they count as misses: " + ", ".join(
                     f"`{r['intent_id']}` (pass {r['pass']}: `{r['text_choice']}`, "
                     f"{'right' if r['text_choice'] in r['expected'] else 'wrong'} tool)" for r in as_text) + ".")
    summary["agreement"] = {"same": agree, "pairs": pairs}
    L.append("")
    L.append("## Most frequent confusions")
    L.append("")
    L.append("| Expected | Chosen instead | Times |")
    L.append("|---|---|---:|")
    for (exp, got), n in confusions.most_common(15):
        L.append(f"| `{exp}` | `{got}` | {n} |")
    summary["confusions"] = [[e, g, n] for (e, g), n in confusions.most_common()]
    L.append("")
    L.append("## Per intent")
    L.append("")
    L.append("| Intent | Expected | Chosen (per pass) |")
    L.append("|---|---|---|")
    chosen_by: dict[str, list[str]] = {}
    for r in sorted(rows, key=lambda r: (r["intent_id"], r["pass"])):
        chosen_by.setdefault(r["intent_id"], []).append(
            f"{'✓' if r['correct'] else '✗'} `{r['chosen']}`")
    for it in intents:
        if it["id"] in chosen_by:
            L.append(f"| {it['intent']} | {', '.join('`' + e + '`' for e in it['expected'])} | "
                     f"{'; '.join(chosen_by[it['id']])} |")
    summary["per_intent_correct"] = dict(per_intent)
    _ = intent_by_id
    return "\n".join(L) + "\n", summary
