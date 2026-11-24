"""myceliums' graph for a commit (#210): which docs units mention the changed symbols and their neighbours.

Two graph-only indexes per commit, both at one shared path: the parent (symbols the
diff changes or removes) and the parent with the non-docs diff applied (symbols it adds;
the docs are the parent's). ``myc export`` gives nodes and edges; only the neighbourhood
of the changed symbols is kept.
"""

from __future__ import annotations

import itertools
import json
import math
from pathlib import Path

from .change import spans, touched
from .corpus import Corpus
from .select_commits import is_unit

TYPES = {"Class", "Interface", "Enum"}
# relation -> weight, per profile. `touched` and `owner` are the changed declarations and their types.
PROFILES = {
    "direct": {"touched": 3, "owner": 2},
    "hop": {"touched": 3, "owner": 2, "member": 0.5, "dispatch": 1, "docref": 1, "nameref": 1, "sample": 3},
    "hop-light": {"touched": 3, "owner": 2, "member": 0.25, "dispatch": 0.5, "docref": 0.5, "nameref": 0.5, "sample": 3},
    "all": {"touched": 3, "owner": 3, "member": 0.5, "dispatch": 1, "docref": 1, "nameref": 1, "sample": 3, "calls": 0.5},
}
GRID = [{"profile": p, "idf": i, "tf": t, "norm": n} for p, i, t, n in
        itertools.product(PROFILES, (1, 2), ("binary", "log"), (0.0, 0.5, 1.0))]


def load(export: Path) -> tuple[dict, dict]:
    """``(nodes, edges by kind)`` of a ``myc export --format json``."""
    doc = json.loads(export.read_text())
    nodes = {n["id"]: n for n in doc["nodes"]}
    edges: dict[str, list[tuple[str, str]]] = {}
    for e in doc["edges"]:
        if e["kind"] in ("MENTIONS", "MEMBER_OF", "OVERRIDES", "REFERENCES", "CALLS"):
            edges.setdefault(e["kind"], []).append((e["source"], e["target"]))
    return nodes, edges


def key(n: dict) -> str:
    return f"{n['file_path']}:{n['qualified_name']}"


def is_code(n: dict) -> bool:
    return n["kind"] in ("Class", "Interface", "Enum", "Method", "Function", "Constant", "Variable") \
        and not n["file_path"].startswith("framework-docs/")


def seeds(nodes: dict, by_file: dict, files: list[dict], side: str) -> tuple[set[str], set[str]]:
    """``(touched, declared)`` symbol ids on one side (``o`` or ``n``): those whose lines a hunk changes
    and those the scanner finds around it (doc comments lie outside a symbol's lines)."""
    hit, decl = set(), set()
    for f in files:
        path, text = (f["old"], f["old_text"]) if side == "o" else (f["new"], f["new_text"])
        if not path or f["kind"] != "main" or path not in by_file:
            continue
        syms = [nodes[i] for i in by_file[path] if is_code(nodes[i])]
        # A hunk belongs to the innermost symbols around its lines.
        for h in f["hunks"]:
            start, length = h[side + "s"], h[side + "l"]
            for ln in range(start, start + length):
                around = [n for n in syms if n["start_line"] <= ln <= n["end_line"]]
                if around:
                    hit.add(min(around, key=lambda n: (n["end_line"] - n["start_line"], n["id"]))["id"])
        if path.endswith((".java", ".kt")):
            t, d = touched(spans(text), f["hunks"], side)
            names = {(s["owner"], s["name"]) for s in t}
            dnames = {(s["owner"], s["name"]) for s in d}
            for n in syms:
                parts = n["qualified_name"].split(".")
                for k in ((parts[-2] if len(parts) > 1 else None, parts[-1]),):
                    if k in names:
                        hit.add(n["id"])
                    if k in dnames:
                        decl.add(n["id"])
    return hit, decl


def neighbourhood(nodes: dict, edges: dict, files: list[dict], side: str, member_of: bool = False) -> dict:
    """Relations from the changed symbols: ``{relation: {symbol key}}`` and the docs units that mention each.

    ``member_of``: owners and members by MEMBER_OF edges as they are (#212), instead of by line containment."""
    by_file: dict[str, list[str]] = {}
    for i, n in nodes.items():
        by_file.setdefault(n["file_path"], []).append(i)
    hit, decl = seeds(nodes, by_file, files, side)
    out, into = {}, {}
    for kind, es in edges.items():
        o, t = out.setdefault(kind, {}), into.setdefault(kind, {})
        for a, b in es:
            o.setdefault(a, []).append(b)
            t.setdefault(b, []).append(a)

    def step(ids, table, kind):
        return {j for i in ids for j in table.get(kind, {}).get(i, ()) if j in nodes}

    # Owners and members by line containment in a file: MEMBER_OF edges of a Java class with a
    # constructor point to the constructor, not to the class (27,395 of 106,850 on Spring).
    owner_of: dict[str, str] = {}
    members_of: dict[str, set[str]] = {}

    def types_around(n: dict) -> list[dict]:
        ts = [nodes[i] for i in by_file[n["file_path"]] if nodes[i]["kind"] in TYPES and i != n["id"]]
        return sorted((t for t in ts if t["start_line"] <= n["start_line"] and n["end_line"] <= t["end_line"]),
                      key=lambda t: (t["end_line"] - t["start_line"], t["id"]))

    def owner(i: str) -> str | None:
        if member_of:
            return next((j for j in sorted(step({i}, out, "MEMBER_OF")) if nodes[j]["kind"] in TYPES), None)
        if i not in owner_of:
            around = types_around(nodes[i])
            owner_of[i] = around[0]["id"] if around else ""
        return owner_of[i] or None

    def members(i: str) -> set[str]:
        if member_of:
            return step({i}, into, "MEMBER_OF")
        if i not in members_of:
            n = nodes[i]
            members_of[i] = {j for j in by_file[n["file_path"]] if j != i and is_code(nodes[j])
                             and n["start_line"] <= nodes[j]["start_line"] and nodes[j]["end_line"] <= n["end_line"]
                             and owner(j) == i}
        return members_of[i]

    def is_constructor(n: dict) -> bool:
        o = owner(n["id"])
        return n["kind"] == "Method" and o is not None and nodes[o]["name"] == n["name"]

    def is_local(n: dict) -> bool:
        """A variable inside a method; it names nothing in the docs."""
        return n["kind"] == "Variable" and any(
            nodes[j]["kind"] in ("Method", "Function") and nodes[j]["start_line"] <= n["start_line"]
            and n["end_line"] <= nodes[j]["end_line"] for j in by_file[n["file_path"]] if j != n["id"])

    def owners_of(ids) -> set[str]:
        res, todo = set(), set(ids)
        while todo:
            todo = {o for o in map(owner, todo) if o} - res
            res |= todo
        return res

    types = {i for i in hit if nodes[i]["kind"] in TYPES}
    methods = {i for i in hit if nodes[i]["kind"] in ("Method", "Function", "Variable", "Constant")
               and not is_constructor(nodes[i]) and not is_local(nodes[i])}
    owners = owners_of(hit)
    dispatch = step(methods, out, "OVERRIDES") | step(methods, into, "OVERRIDES")
    dispatch |= owners_of(dispatch)
    rel = {
        "touched": types | methods,
        "owner": owners,
        "member": {j for i in types | owners for j in members(i) if nodes[j]["kind"] != "Variable"},
        "dispatch": dispatch,
        "docref": {j for j in step(types | methods, out, "MENTIONS") | step(types | methods, into, "MENTIONS")},
        "nameref": step(types | methods, out, "REFERENCES") | step(types | methods, into, "REFERENCES"),
        "calls": step(methods, out, "CALLS") | step(methods, into, "CALLS"),
    }
    units: dict[str, dict[str, int]] = {}
    docs_in: dict[str, dict[str, int]] = {}
    for a, b in edges.get("MENTIONS", ()):
        src = nodes.get(a)
        if src and b in nodes and is_unit(src["file_path"]):
            d = docs_in.setdefault(b, {})
            d[src["file_path"]] = d.get(src["file_path"], 0) + 1
    relations: dict[str, list[str]] = {}
    for r, ids in rel.items():
        ids = {i for i in ids if is_code(nodes[i])}
        relations[r] = sorted({key(nodes[i]) for i in ids})
        for i in ids:
            if i in docs_in:
                u = units.setdefault(key(nodes[i]), {})
                for p, n in docs_in[i].items():
                    u[p] = max(u.get(p, 0), n)
    sample: dict[str, int] = {}
    direct = types | methods | owners
    for kind in ("CALLS", "OVERRIDES", "REFERENCES", "MENTIONS"):
        for j in step(direct, into, kind):
            p = nodes[j]["file_path"]
            if is_unit(p) and not p.endswith(".adoc"):
                sample[p] = sample.get(p, 0) + 1
    return {"relations": relations, "units": units, "sample": sample,
            "declared": sorted({key(nodes[i]) for i in decl}), "symbols": len(nodes),
            "mentions": sum(map(len, docs_in.values()))}


def merge(a: dict, b: dict) -> dict:
    """The old and the new side as one neighbourhood."""
    rel = {r: sorted(set(a["relations"].get(r, [])) | set(b["relations"].get(r, [])))
           for r in {**a["relations"], **b["relations"]}}
    units = {k: dict(v) for k, v in a["units"].items()}
    for k, v in b["units"].items():
        for p, n in v.items():
            units.setdefault(k, {})[p] = max(units.get(k, {}).get(p, 0), n)
    sample = {p: max(a["sample"].get(p, 0), b["sample"].get(p, 0)) for p in {**a["sample"], **b["sample"]}}
    return {"relations": rel, "units": units, "sample": sample, "declared": sorted(set(a["declared"]) | set(b["declared"]))}


def mentions(c: Corpus, g: dict, cfg: dict) -> dict[str, float]:
    """Docs units by the mentions of the changed symbols; a symbol in several relations has the largest weight."""
    w = PROFILES[cfg["profile"]]
    weight: dict[str, float] = {}
    for r, x in w.items():
        for k in g["relations"].get(r, ()):
            weight[k] = max(weight.get(k, 0.0), x)
    scores: dict[str, float] = {}
    for k, x in weight.items():
        units = {p: n for p, n in g["units"].get(k, {}).items() if p in c.units}
        if not units:
            continue
        idf = math.log((c.n + 1) / (len(units) + 0.5)) ** cfg["idf"]
        for p, n in units.items():
            tf = 1 if cfg["tf"] == "binary" else 1 + math.log(n)
            scores[p] = scores.get(p, 0.0) + x * idf * tf
    if w.get("sample"):
        for p, n in g["sample"].items():
            if p in c.units:
                scores[p] = scores.get(p, 0.0) + w["sample"] * (1 + math.log(n)) * math.log(c.n)
    return {p: s / (0.5 + 0.5 * c.units[p].length / c.avg_ident) ** cfg["norm"] for p, s in scores.items()}
