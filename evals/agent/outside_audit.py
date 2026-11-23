#!/usr/bin/env python3
"""Post-hoc audit of a run's transcripts (#182): what did the sessions touch outside their working copy?

Sessions run as the same user as the harness, without a sandbox: they can in
principle read gold sets, caches or other checkouts, and arms with Bash can use
the network. This lists, for every tool call of every final session,

* outside: a path that is not below the working copy: absolute paths, `~`,
  `$HOME`-like variables, and `..` that resolves above the working copy;
* network: WebFetch / WebSearch, network programs, git commands that contact a
  remote, build tools that download, network code in an interpreter, URLs;
* marker: a tool *result* that names the eval's own files (gold sets, caches).

It over-reports on purpose (a regex like '/foo' or a URL searched for in XML is
a hit); every hit is read by hand and its verdict recorded.

    python3 evals/agent/outside_audit.py LABEL WORK_COPY [--out FILE]
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from aeval import config  # noqa: E402
from aeval.stream import _result_text  # noqa: E402

SAFE = {"/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr", "/"}
NETWORK = re.compile(
    r"\b(curl|wget|nc|ncat|ssh|scp|rsync|ftp|telnet|dig|nslookup|host|gh|brew|pip3?|npm|npx|yarn|mvn|mvnw|gradlew?)\b"
    r"|\bgit\b[^;|&\n]*\b(fetch|pull|clone|ls-remote|submodule|remote\s+(update|show|add))\b"
    r"|urllib|requests\.(get|post)|http\.client|socket\.|fetch\(|/dev/tcp/|https?://", re.IGNORECASE)
MARKER = re.compile(r"gold_method|labelled_by|tasks-java|agent-tasks-java|myceliums-eval|\.cache/myceliums|evals/agent")
HOMEVAR = re.compile(r"~(?=/|\s|$|['\"])|\$\{?(HOME|OLDPWD|TMPDIR|USER)\b")
TOKEN = re.compile(r"[^\s'\"`;|&()<>=,{}\[\]]+")


def _under(path: str, roots: tuple[str, ...]) -> bool:
    p = os.path.normpath(path)
    return any(p == r or p.startswith(r + "/") for r in roots)


def outside_paths(text: str, roots: tuple[str, ...], cwd: str) -> list[str]:
    """Paths in a string that are (or may be) outside ``roots``; `cd` inside it is followed."""
    hits = [m.group(0) for m in HOMEVAR.finditer(text)]
    toks = TOKEN.findall(text)
    for i, t in enumerate(toks):
        if i and toks[i - 1] == "cd" and not t.startswith("-"):
            cwd = t if t.startswith("/") else os.path.normpath(os.path.join(cwd, t))
        if t.startswith("/"):
            if t.rstrip("/") not in {s.rstrip("/") for s in SAFE} - {""} and not _under(t, roots) and t != "//":
                hits.append(t)
        elif ".." in t.split("/") and not _under(os.path.join(cwd, t), roots):
            hits.append(t)
    # `find /` and `grep -r x /`: the bare root is only safe inside another token
    hits += re.findall(r"\b(?:find|ls|grep[^|;&]*|rg[^|;&]*|fd)\s+/(?=\s|$)", text)
    return hits


def audit_transcript(path: Path, roots: tuple[str, ...]) -> list[dict]:
    hits = []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        content = (ev.get("message") or {}).get("content")
        for part in content if isinstance(content, list) else []:
            if not isinstance(part, dict):
                continue
            if part.get("type") == "tool_use":
                tool, inp = part.get("name", ""), part.get("input") or {}
                if tool in ("WebFetch", "WebSearch"):
                    hits.append({"kind": "network", "tool": tool, "what": json.dumps(inp)[:300], "match": [tool]})
                for v in inp.values():
                    if not isinstance(v, str):
                        continue
                    out = outside_paths(v, roots, roots[0])
                    if out:
                        hits.append({"kind": "outside", "tool": tool, "what": v[:600], "match": out})
                    net = [m.group(0) for m in NETWORK.finditer(v)] if tool == "Bash" else []
                    if net:
                        hits.append({"kind": "network", "tool": tool, "what": v[:600], "match": net})
            elif part.get("type") == "tool_result":
                m = MARKER.search(_result_text(part.get("content")))
                if m:
                    t = _result_text(part.get("content"))
                    hits.append({"kind": "marker", "tool": "result", "what": t[max(0, m.start() - 150):m.end() + 150],
                                 "match": [m.group(0)]})
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("label")
    ap.add_argument("work_copy")
    ap.add_argument("--out")
    a = ap.parse_args()
    work = a.work_copy.rstrip("/")
    roots = tuple(dict.fromkeys((work, work.removeprefix("/private"))))
    rows = {}
    for x in (config.RUNS_DIR / a.label / "results.jsonl").read_text().splitlines():
        r = json.loads(x)
        if r.get("final"):
            rows[r["run_id"]] = r
    out, sessions, calls = [], Counter(), Counter()
    for r in rows.values():
        p = config.RUNS_DIR / a.label / "sessions" / r["run_id"] / f"attempt{r['attempt']}.jsonl"
        sessions[r["condition"]] += 1
        calls[r["condition"]] += r["tool_calls"]
        for h in audit_transcript(p, roots):
            h = json.loads(json.dumps(h).replace(str(Path.home()), "~"))
            out.append({"run_id": r["run_id"], "condition": r["condition"], **h})
    doc = {"label": a.label, "sessions": dict(sessions), "tool_calls": dict(calls),
           "hits": {c: dict(Counter(h["kind"] for h in out if h["condition"] == c)) for c in sessions}, "items": out}
    if a.out:
        Path(a.out).write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n")
    print(json.dumps({k: doc[k] for k in ("label", "sessions", "tool_calls", "hits")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
