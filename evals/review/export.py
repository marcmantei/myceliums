#!/usr/bin/env python3
"""Export every agent-drafted label as one reviewable item (#164).

    python3 evals/review/export.py            # writes evals/review/items.jsonl
    python3 evals/review/export.py --check    # exit 1 if items.jsonl is out of date

One item per claim a human can confirm or reject, from:

* retrieval-queries / retrieval-test-seeking
  (crates/myceliums-benchmarks/real-repos/{queries,test-seeking}.json):
  one item per (query, labelled symbol) and one "anything important missing?"
  item per query;
* graph-gold (crates/myceliums-benchmarks/graph-gold/<language>.json):
  every gold symbol, CALLS edge, dynamic call and trap;
* graph-held-out (graph-gold/held-out/real-repos.json): one item per call site;
* agent-tasks, agent-tasks-hard, agent-tasks-hard-v1, agent-tasks-outcome,
  agent-tasks-scale, agent-tasks-java (evals/agent/tasks/*.json): every gold and optional answer, and one
  "is the gold set complete?" item per task.

Ids are derived from the labels' own keys (query id, symbol id, caller and
call line, task id), never from positions, so they survive reordering.
Code snippets come from the pinned checkouts (see CHECKOUT_ROOTS); a checkout
at any other commit is not used. Output is deterministic: no timestamps.
Python 3.10+ standard library only.
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

ROOT = common.REPO_ROOT
BENCH = ROOT / "crates" / "myceliums-benchmarks"
REAL_REPOS = BENCH / "real-repos"
GRAPH_GOLD = BENCH / "graph-gold"
FIXTURES = ROOT / "tests" / "fixtures"
AGENT = ROOT / "evals" / "agent"
GRAPH_LANGUAGES = ("python", "typescript", "go", "rust", "java")
TASK_FILES = (
    ("agent-tasks", "tasks.json", "#99"),
    ("agent-tasks-hard", "tasks-hard.json", "#125"),
    ("agent-tasks-hard-v1", "tasks-hard-v1.json", "#125"),
    ("agent-tasks-outcome", "tasks-outcome.json", "#153"),
    ("agent-tasks-scale", "tasks-scale.json", "#163"),
    ("agent-tasks-java", "tasks-java.json", "#182"),
)

MAX_BLOCK = 40     # lines of a definition shown
CONTEXT = 4        # lines around a call site / evidence line
MAX_EVIDENCE = 3   # evidence snippets per task answer


def set_root(root: Path) -> None:
    """Point every dataset path at another checkout of this repository (tests)."""
    global ROOT, BENCH, REAL_REPOS, GRAPH_GOLD, FIXTURES, AGENT
    ROOT = root
    BENCH = root / "crates" / "myceliums-benchmarks"
    REAL_REPOS = BENCH / "real-repos"
    GRAPH_GOLD = BENCH / "graph-gold"
    FIXTURES = root / "tests" / "fixtures"
    AGENT = root / "evals" / "agent"
    common.REPO_ROOT = root


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


# ------------------------------------------------------------------ checkouts

def checkout_roots() -> list[Path]:
    """Where pinned checkouts may live, in the order they are tried."""
    home = Path.home()
    roots = []
    for env in ("MYC_EVAL_REPOS_DIR",):
        if os.environ.get(env):
            roots.append(Path(os.environ[env]))
    cache = Path(os.environ.get("XDG_CACHE_HOME") or home / ".cache")
    roots.append(cache / "myceliums-eval-repos")
    if os.environ.get("AEVAL_CACHE"):
        roots.append(Path(os.environ["AEVAL_CACHE"]) / "repos")
    roots.append(home / ".cache" / "myceliums-agent-eval" / "repos")
    roots.extend(sorted((home / ".cache").glob("myceliums-agent-eval-*/repos")))
    return roots


class Checkouts:
    """Finds a checkout of `repo` whose HEAD is exactly `commit`."""

    def __init__(self, roots: list[Path] | None = None, fixed: dict[str, Path] | None = None):
        self.roots = checkout_roots() if roots is None else roots
        self.fixed = fixed  # tests: repo -> directory, no git check
        self._heads: dict[Path, str | None] = {}
        self.missing: dict[tuple[str, str], str] = {}

    def _head(self, d: Path) -> str | None:
        if d not in self._heads:
            try:
                out = subprocess.run(["git", "-C", str(d), "rev-parse", "HEAD"],
                                     capture_output=True, text=True, timeout=30)
                self._heads[d] = out.stdout.strip() if out.returncode == 0 else None
            except (OSError, subprocess.SubprocessError):
                self._heads[d] = None
        return self._heads[d]

    def find(self, repo: str, commit: str) -> Path | None:
        if self.fixed is not None:
            return self.fixed.get(repo)
        for root in self.roots:
            d = root / repo
            if (d / ".git").exists() and self._head(d) == commit:
                return d
        self.missing[(repo, commit)] = repo
        return None


class Sources:
    """Reads files (cached) and cuts snippets."""

    def __init__(self):
        self._cache: dict[Path, list[str] | None] = {}

    def lines(self, path: Path) -> list[str] | None:
        if path not in self._cache:
            try:
                self._cache[path] = path.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                self._cache[path] = None
        return self._cache[path]


SRC = Sources()


def snippet(base: Path | None, file: str, start: int, end: int, focus: list[int],
            label: str, note: str | None = None) -> dict:
    """Lines start..end (1-based, inclusive) of base/file, clipped to the file."""
    ev: dict = {"file": file, "line": focus[0] if focus else start, "source": label}
    if base is None:
        ev["missing"] = "pinned checkout not found"
        return ev
    lines = SRC.lines(base / file)
    if lines is None:
        ev["missing"] = f"{file} not found in the pinned checkout"
        return ev
    start = max(1, start)
    end = min(len(lines), max(start, end))
    ev.update({"start": start, "lines": lines[start - 1:end], "focus": sorted(set(focus))})
    if note:
        ev["note"] = note
    return ev


def block_end(lines: list[str], line: int, python: bool, cap: int = MAX_BLOCK) -> tuple[int, bool]:
    """Last line of the definition starting at `line` (heuristic), and whether it was capped."""
    n = len(lines)
    last = min(n, line + cap - 1)
    if python:
        indent = len(lines[line - 1]) - len(lines[line - 1].lstrip())
        end = line
        opened = lines[line - 1].rstrip().endswith((":", "(", "[", "{", ","))
        for i in range(line + 1, n + 1):
            text = lines[i - 1]
            if not text.strip():
                continue
            ind = len(text) - len(text.lstrip())
            if ind <= indent and not (opened and text.strip().startswith((")", "]", "}"))):
                break
            end = i
            opened = False
        return (min(end, last), end > last)
    depth = 0
    seen = False
    for i in range(line, n + 1):
        text = re.sub(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|`(?:\\.|[^`\\])*`', '""', lines[i - 1])
        text = text.split("//", 1)[0]
        for ch in text:
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
        if seen and depth <= 0:
            return (min(i, last), i > last)
        if not seen and i - line >= 2 and (text.rstrip().endswith((";", ")")) or i - line >= 6):
            return (min(i, last), i > last)
        if not seen and i == line and text.rstrip().endswith(";"):
            return (i, False)
    return (last, n > last)


def definition_snippet(base: Path | None, file: str, line: int, label: str, cap: int = MAX_BLOCK,
                       note: str | None = None) -> dict:
    if base is None or SRC.lines(base / file) is None:
        return snippet(base, file, line, line, [line], label, note)
    lines = SRC.lines(base / file)
    line = max(1, min(line, len(lines)))
    end, capped = block_end(lines, line, file.endswith(".py"), cap)
    start = line
    # include decorators / doc comments directly above
    while start > 1 and start > line - 3 and lines[start - 2].strip().startswith(("@", "///", "//", "#[")):
        start -= 1
    if capped:
        note = (note + "; " if note else "") + f"definition truncated after {cap} lines"
    return snippet(base, file, start, end, [line], label, note)


# ------------------------------------------------------------ symbol locator

def _py_defs(lines: list[str]) -> dict[str, int]:
    try:
        tree = ast.parse("\n".join(lines))
    except SyntaxError:
        return {}
    out: dict[str, int] = {}

    def walk(node, prefix: str):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                q = f"{prefix}{child.name}"
                out.setdefault(q, child.lineno)
                walk(child, q + ".")
            elif isinstance(child, (ast.Assign, ast.AnnAssign)) and not prefix:
                targets = child.targets if isinstance(child, ast.Assign) else [child.target]
                for t in targets:
                    if isinstance(t, ast.Name):
                        out.setdefault(t.id, child.lineno)
            elif isinstance(child, (ast.If, ast.Try, ast.With)):
                walk(child, prefix)
    walk(tree, "")
    return out


_TS_MOD = r"(?:(?:export|default|declare|public|private|protected|static|async|readonly|override|abstract|get|set)\s+)*"


def locate(lines: list[str], file: str, symbol: str) -> tuple[int | None, int]:
    """1-based definition line of a qualified symbol in a file, and how many candidates."""
    if file.endswith(".py"):
        defs = _py_defs(lines)
        if symbol in defs:
            return defs[symbol], 1
    parts = symbol.split(".")
    name = re.escape(parts[-1])
    container = re.escape(parts[-2]) if len(parts) > 1 else None
    pats: list[str] = []
    if file.endswith(".go"):
        if container:
            pats.append(rf"^func\s*\(\s*\w*\s*\*?\s*{container}(?:\[[^\]]*\])?\s*\)\s*{name}\b")
        pats += [rf"^func\s+{name}\b", rf"^type\s+{name}\b", rf"^(?:var|const)\s+{name}\b",
                 rf"^\s+{name}\s+(?:struct|interface|func|=|\[|\*|[A-Za-z])", rf"^\s+{name}\("]
    elif file.endswith((".ts", ".tsx", ".js", ".mjs", ".cjs", ".jsx")):
        pats += [rf"^\s*{_TS_MOD}(?:function\*?|class|interface|type|enum|const|let|var|namespace)\s+{name}\b",
                 rf"^\s*{_TS_MOD}#?{name}\s*(?:<[^>]*>)?\s*[\(=:?!]"]
    else:
        pats += [rf"^\s*(?:async\s+)?(?:def|class)\s+{name}\b",
                 rf"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:fn|struct|enum|trait|type|impl)\s+{name}\b",
                 rf"^\s*{name}\s*="]
    start = 0
    if container and not file.endswith(".go"):
        cpat = re.compile(rf"\b(?:class|interface|struct|trait|impl|type|const|let|var|namespace)\s+{container}\b")
        for i, text in enumerate(lines):
            if cpat.search(text):
                start = i
                break
    hits: list[int] = []
    for pat in pats:
        rx = re.compile(pat)
        hits = [i + 1 for i in range(start, len(lines)) if rx.search(lines[i])]
        if hits:
            break
    if not hits and container and not file.endswith(".go") and start:
        line, n = locate(lines[start:], file, parts[-1])
        if line is not None:
            return line + start, n
    if not hits:
        rx = re.compile(rf"\b{name}\b")
        hits = [i + 1 for i, text in enumerate(lines) if rx.search(text)][:1]
    return (hits[0] if hits else None), len(hits)


def symbol_snippet(base: Path | None, file: str, symbol: str, label: str) -> dict:
    if base is None:
        return snippet(None, file, 1, 1, [], label)
    lines = SRC.lines(base / file)
    if lines is None:
        return snippet(base, file, 1, 1, [], label)
    line, n = locate(lines, file, symbol)
    if line is None:
        ev = snippet(base, file, 1, min(15, len(lines)), [], label,
                     f"definition of {symbol} not located; showing the top of the file")
        return ev
    note = f"{n} definitions named {symbol.split('.')[-1]} matched; showing the first" if n > 1 else None
    return definition_snippet(base, file, line, label, note=note)


# ------------------------------------------------------------------- helpers

def source_line(text_lines: list[str], *needles: str, start: int = 0) -> int | None:
    """1-based line of the last needle, each searched after the previous one."""
    i = start
    found = None
    for needle in needles:
        while i < len(text_lines) and needle not in text_lines[i]:
            i += 1
        if i >= len(text_lines):
            return found
        found = i + 1
    return found


def jq(value: str) -> str:
    """A JSON-quoted string, as it appears in the dataset file."""
    return json.dumps(value, ensure_ascii=False)


def item(dataset: str, local_id: str, kind: str, claim: str, label: str, *, context: dict,
         location: str | None, repo: str | None, commit: str | None, evidence: list[dict],
         source_file: Path, pointer: str, source_line_no: int | None) -> dict:
    return {
        "dataset": dataset,
        "id": f"{dataset}:{local_id}",
        "kind": kind,
        "claim": claim,
        "label": label,
        "context": context,
        "location": location,
        "repo": repo,
        "commit": commit,
        "evidence": evidence,
        "source": {"file": rel(source_file), "pointer": pointer, "line": source_line_no},
    }


def loc(ev: dict) -> str:
    return f"{ev['file']}:{ev['line']}" if ev.get("line") else ev["file"]


# ----------------------------------------------------------------- retrieval

def export_retrieval(dataset: str, path: Path, co: Checkouts, snippets: bool) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    text = path.read_text(encoding="utf-8").splitlines()
    pins = {r["name"]: r for r in data["repos"]}
    out = []
    for qi, q in enumerate(data["queries"]):
        pin = pins[q["repo"]]
        base = co.find(q["repo"], pin["commit"]) if snippets else None
        label = f"{q['repo']}@{pin['commit'][:10]}"
        qline = source_line(text, f'"id": {jq(q["id"])}')
        ctx_q = {"query": q["query"], "repo": q["repo"], "intent": q["intent"], "rationale": q["rationale"],
                 "labelled_by": q["labelled_by"]}
        for ri, r in enumerate(q["relevant"]):
            ev = symbol_snippet(base, r["file"], r["symbol"], label) if snippets else \
                {"file": r["file"], "line": None, "source": label}
            out.append(item(
                dataset, f"{q['id']}:label:{r['file']}::{r['symbol']}", "retrieval-label",
                f"The query “{q['query']}” ({q['repo']}, {q['intent']}) is answered by "
                f"`{r['symbol']}` in {r['file']}: a reader should read it first.",
                "relevant",
                context={**ctx_q, "why": r["why"]},
                location=loc(ev), repo=q["repo"], commit=pin["commit"], evidence=[ev],
                source_file=path, pointer=f"/queries/{qi}/relevant/{ri}",
                source_line_no=source_line(text, f'"id": {jq(q["id"])}', f'"symbol": {jq(r["symbol"])}')))
        names = "; ".join(f"`{r['symbol']}` ({r['file']})" for r in q["relevant"])
        out.append(item(
            dataset, f"{q['id']}:complete", "retrieval-complete",
            f"Nothing important is missing from the labels of the query “{q['query']}” "
            f"({q['repo']}, {q['intent']}): {names}.",
            f"complete ({len(q['relevant'])} labels)",
            context={**ctx_q, "labels": [f"{r['file']}::{r['symbol']}" for r in q["relevant"]]},
            location=None, repo=q["repo"], commit=pin["commit"], evidence=[],
            source_file=path, pointer=f"/queries/{qi}", source_line_no=qline))
    return out


# ---------------------------------------------------------------- graph gold

def _split(gid: str) -> tuple[str, str]:
    return tuple(gid.split("::", 1)) if "::" in gid else ("", gid)  # type: ignore[return-value]


def export_graph_gold(co: Checkouts, snippets: bool) -> list[dict]:
    dataset = "graph-gold"
    out = []
    for lang in GRAPH_LANGUAGES:
        path = GRAPH_GOLD / f"{lang}.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        text = path.read_text(encoding="utf-8").splitlines()
        base = FIXTURES / data["fixture"] if snippets else None
        label = f"tests/fixtures/{data['fixture']}"
        sym_line = {s["id"]: s["line"] for s in data["symbols"]}
        common_ctx = {"language": lang, "fixture": label, "labelled_by": data["labelled_by"]}

        def defn(gid: str, cap: int = MAX_BLOCK) -> dict:
            f, _ = _split(gid)
            ln = sym_line.get(gid)
            if ln is None:
                return {"file": f, "line": None, "source": label, "missing": f"{gid} is not a gold symbol"}
            return definition_snippet(base, f, ln, label, cap) if snippets else {"file": f, "line": ln, "source": label}

        def site(gid: str, line: int) -> dict:
            f, _ = _split(gid)
            if not snippets:
                return {"file": f, "line": line, "source": label}
            caller_line = sym_line.get(gid)
            if caller_line and 0 < line - caller_line <= 20:
                return snippet(base, f, caller_line, line + 2, [line], label)
            return snippet(base, f, line - CONTEXT, line + CONTEXT, [line], label)

        for si, s in enumerate(data["symbols"]):
            f, q = _split(s["id"])
            ev = defn(s["id"])
            out.append(item(
                dataset, f"{lang}:symbol:{s['id']}", "graph-symbol",
                f"{label}/{f} defines the in-scope {s['kind']} `{q}`, named on line {s['line']}.",
                f"symbol ({s['kind']})", context=common_ctx,
                location=f"{label}/{f}:{s['line']}", repo=None, commit=None, evidence=[ev],
                source_file=path, pointer=f"/symbols/{si}",
                source_line_no=source_line(text, f'"id": {jq(s["id"])}')))
        for ci, c in enumerate(data["calls"]):
            f, _ = _split(c["from"])
            accept = c.get("also_accept") or []
            extra = f" (also accepted: {', '.join(f'`{a}`' for a in accept)})" if accept else ""
            evs = [site(c["from"], c["line"]), defn(c["to"], 12)]
            out.append(item(
                dataset, f"{lang}:call:{c['from']}->{c['to']}", "graph-call",
                f"`find_callers({c['to']})` should include `{c['from']}`: it calls it statically, "
                f"first on line {c['line']}{extra}.",
                "CALLS edge",
                context={**common_ctx, "why": c["why"], "tags": c.get("tags", [])},
                location=f"{label}/{f}:{c['line']}", repo=None, commit=None, evidence=evs,
                source_file=path, pointer=f"/calls/{ci}",
                source_line_no=source_line(text, f'"from": {jq(c["from"])}, "to": {jq(c["to"])}')))
        for di, d in enumerate(data.get("dynamic", [])):
            evs = [defn(d["from"]), defn(d["to"], 12)]
            out.append(item(
                dataset, f"{lang}:dynamic:{d['from']}->{d['to']}", "graph-dynamic",
                f"`{d['from']}` reaches `{d['to']}` only through a function value: a run-time call "
                f"with no static target, so it is not a scored edge.",
                "dynamic (not scored)", context={**common_ctx, "why": d["why"]},
                location=loc(evs[0]), repo=None, commit=None, evidence=evs,
                source_file=path, pointer=f"/dynamic/{di}",
                source_line_no=source_line(text, f'"from": {jq(d["from"])}, "to": {jq(d["to"])}')))
        seen: dict[str, int] = {}
        for ti, t in enumerate(data.get("traps", [])):
            f, _ = _split(t["in"])
            key = f"{lang}:trap:{t['in']}:{t['name']}@{t['line']}"
            seen[key] = seen.get(key, 0) + 1
            if seen[key] > 1:
                key += f"#{seen[key]}"
            ev = site(t["in"], t["line"])
            out.append(item(
                dataset, key, "graph-trap",
                f"The call of `{t['name']}` on line {t['line']} inside `{t['in']}` must not produce an "
                f"edge to any fixture symbol.",
                "no edge", context={**common_ctx, "why": t["why"]},
                location=f"{label}/{f}:{t['line']}", repo=None, commit=None, evidence=[ev],
                source_file=path, pointer=f"/traps/{ti}",
                source_line_no=source_line(text, f'"in": {jq(t["in"])}, "name": {jq(t["name"])}, "line": {t["line"]}')))
    return out


def export_held_out(co: Checkouts, snippets: bool) -> list[dict]:
    dataset = "graph-held-out"
    path = GRAPH_GOLD / "held-out" / "real-repos.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    text = path.read_text(encoding="utf-8").splitlines()
    out = []
    for pi, p in enumerate(data["pairs"]):
        commit = data["repos"][p["repo"]]
        base = co.find(p["repo"], commit) if snippets else None
        label = f"{p['repo']}@{commit[:10]}"
        evs = []
        if snippets:
            if p["call_line"] - p["caller_line"] <= 30:
                evs.append(snippet(base, p["file"], p["caller_line"], p["call_line"] + 2,
                                   [p["caller_line"], p["call_line"]], label))
            else:
                evs.append(snippet(base, p["file"], p["caller_line"], p["caller_line"] + 2,
                                   [p["caller_line"]], label))
                evs.append(snippet(base, p["file"], p["call_line"] - CONTEXT, p["call_line"] + CONTEXT,
                                   [p["call_line"]], label))
            for t in p["targets"]:
                evs.append(definition_snippet(base, t["file"], t["line"], label, 12))
        else:
            evs.append({"file": p["file"], "line": p["call_line"], "source": label})
        if p["targets"]:
            tg = ", ".join(f"`{t['name']}` ({t['file']}:{t['line']})" for t in p["targets"])
            claim = (f"In {p['repo']}, the call of `{p['callee']}` on line {p['call_line']} inside "
                     f"`{p['caller']}` ({p['file']}) binds to {tg}.")
            lab = "targets: " + ", ".join(f"{t['file']}::{t['name']}" for t in p["targets"])
        else:
            claim = (f"In {p['repo']}, the call of `{p['callee']}` on line {p['call_line']} inside "
                     f"`{p['caller']}` ({p['file']}) binds to no project symbol (built-in, library or a value).")
            lab = "no project target"
        out.append(item(
            dataset, f"{p['repo']}:{p['file']}:{p['caller']}@{p['call_line']}:{p['callee']}", "held-out-pair",
            claim, lab, context={"why": p["why"], "repo": p["repo"], "rules": data["rules"],
                                 "labelled_by": data["labelled_by"]},
            location=f"{p['file']}:{p['call_line']}", repo=p["repo"], commit=commit, evidence=evs,
            source_file=path, pointer=f"/pairs/{pi}",
            source_line_no=source_line(text, f'"file": {jq(p["file"])}, "caller": {jq(p["caller"])}, '
                                             f'"caller_line": {p["caller_line"]}, "call_line": {p["call_line"]}')))
    return out


# --------------------------------------------------------------- agent tasks

def export_tasks(dataset: str, filename: str, issue: str, co: Checkouts, snippets: bool) -> list[dict]:
    path = AGENT / "tasks" / filename
    data = json.loads(path.read_text(encoding="utf-8"))
    text = path.read_text(encoding="utf-8").splitlines()
    pins = {r["name"]: r for r in json.loads((AGENT / "repos.json").read_text(encoding="utf-8"))["repos"]}
    out = []
    for ti, t in enumerate(data["tasks"]):
        commit = pins[t["repo"]]["commit"]
        base = co.find(t["repo"], commit) if snippets else None
        label = f"{t['repo']}@{commit[:10]}"
        tline = source_line(text, f'"id": {jq(t["id"])}')
        ctx_t = {"task": t["id"], "issue": issue, "question": t["question"], "type": t["type"],
                 "answer_kind": t["answer_kind"], "rationale": t["rationale"],
                 "verified_with": t.get("verified_with", [])}
        what = "file" if t["answer_kind"] == "files" else "symbol"
        for gi, g in enumerate(t["gold"]):
            evs = []
            for e in g.get("evidence", [])[:MAX_EVIDENCE]:
                if snippets:
                    ln = e.get("line") or 1
                    evs.append(snippet(base, e["file"], ln - CONTEXT, ln + CONTEXT, [ln], label,
                                       f"expected to contain {e['contains']!r}"))
                else:
                    evs.append({"file": e["file"], "line": e.get("line"), "source": label})
            n_ev = len(g.get("evidence", []))
            if n_ev > MAX_EVIDENCE:
                ctx_more = {"evidence_not_shown": n_ev - MAX_EVIDENCE}
            else:
                ctx_more = {}
            out.append(item(
                dataset, f"{t['id']}:gold:{g['name']}", "task-gold",
                f"Task {t['id']}: the {what} `{g['name']}` belongs in the answer (required).",
                "required",
                context={**ctx_t, "note": g.get("note", ""), "aliases": g.get("aliases", []), **ctx_more},
                location=loc(evs[0]) if evs else None, repo=t["repo"], commit=commit, evidence=evs,
                source_file=path, pointer=f"/tasks/{ti}/gold/{gi}",
                source_line_no=source_line(text, f'"id": {jq(t["id"])}', f'"name": {jq(g["name"])}')))
        for oi, o in enumerate(t.get("optional", [])):
            name = o["name"] if isinstance(o, dict) else str(o)
            why = o.get("why", "") if isinstance(o, dict) else ""
            out.append(item(
                dataset, f"{t['id']}:optional:{name}", "task-optional",
                f"Task {t['id']}: the {what} `{name}` is a defensible extra answer: neutral in grading, "
                f"neither required nor wrong.",
                "optional", context={**ctx_t, "why": why},
                location=None, repo=t["repo"], commit=commit, evidence=[],
                source_file=path, pointer=f"/tasks/{ti}/optional/{oi}",
                source_line_no=source_line(text, f'"id": {jq(t["id"])}', '"optional"', f'"name": {jq(name)}')))
        out.append(item(
            dataset, f"{t['id']}:complete", "task-complete",
            f"Task {t['id']}: the gold answer is complete: no required {what} is missing "
            f"({len(t['gold'])} listed).",
            f"complete ({len(t['gold'])} required)",
            context={**ctx_t, "gold": [g["name"] for g in t["gold"]],
                     "optional": [o["name"] if isinstance(o, dict) else str(o) for o in t.get("optional", [])]},
            location=None, repo=t["repo"], commit=commit, evidence=[],
            source_file=path, pointer=f"/tasks/{ti}", source_line_no=tline))
    return out


# ---------------------------------------------------------------------- main

def build_items(checkouts: Checkouts | None = None, snippets: bool = True) -> list[dict]:
    co = checkouts or Checkouts()
    items: list[dict] = []
    items += export_retrieval("retrieval-queries", REAL_REPOS / "queries.json", co, snippets)
    items += export_retrieval("retrieval-test-seeking", REAL_REPOS / "test-seeking.json", co, snippets)
    items += export_graph_gold(co, snippets)
    items += export_held_out(co, snippets)
    for dataset, filename, issue in TASK_FILES:
        items += export_tasks(dataset, filename, issue, co, snippets)
    ids = [it["id"] for it in items]
    dupes = sorted({i for i in ids if ids.count(i) > 1}) if len(ids) != len(set(ids)) else []
    if dupes:
        raise SystemExit(f"duplicate item ids: {dupes[:5]}")
    return items


def render(items: list[dict]) -> str:
    return "".join(json.dumps(it, ensure_ascii=False, sort_keys=True) + "\n" for it in items)


def counts(items: list[dict]) -> list[tuple[str, int, dict[str, int], int]]:
    rows = []
    for ds in common.DATASETS:
        sub = [it for it in items if it["dataset"] == ds]
        kinds: dict[str, int] = {}
        for it in sub:
            kinds[it["kind"]] = kinds.get(it["kind"], 0) + 1
        unresolved = sum(1 for it in sub for ev in it["evidence"] if ev.get("missing") or ev.get("note", "").startswith("definition of"))
        rows.append((ds, len(sub), kinds, unresolved))
    return rows


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=common.ITEMS_FILE)
    ap.add_argument("--check", action="store_true", help="exit 1 if --out differs from a fresh export")
    args = ap.parse_args(argv)
    co = Checkouts()
    items = build_items(co)
    text = render(items)
    if args.check:
        current = args.out.read_text(encoding="utf-8") if args.out.exists() else ""
        if current != text:
            print(f"{args.out} is out of date: re-run export.py", file=sys.stderr)
            return 1
        print(f"{args.out} is up to date ({len(items)} items)")
        return 0
    common.write_atomic(args.out, text)
    print(f"wrote {len(items)} items to {args.out}")
    print(f"{'dataset':<24}{'items':>7}  kinds  [unlocated snippets]")
    for ds, n, kinds, unresolved in counts(items):
        k = ", ".join(f"{name} {c}" for name, c in kinds.items())
        extra = f"  [{unresolved}]" if unresolved else ""
        print(f"{ds:<24}{n:>7}  {k}{extra}")
    for (repo, commit) in sorted(co.missing):
        hint = ("crates/myceliums-benchmarks/real-repos/fetch.sh" if repo in ("requests", "flask", "gin", "hono")
                else "python3 evals/agent/agent_eval.py fetch")
        print(f"warning: no checkout of {repo} at {commit} — snippets missing; fetch with {hint}",
              file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
