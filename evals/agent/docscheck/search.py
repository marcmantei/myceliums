"""myceliums' search over the docs units (#210): query formulations and their aggregation.

``myc search`` (keyword, ``--hybrid``) over an index of ``framework-docs/`` alone: the search
has no file filter, and over the whole repository the first hits are mostly code. (The MCP
tool's table shortens the paths, so the CLI is used; both rank with the same library.)
The vectors of unchanged sections are reused because the path stays the same.
"""

from __future__ import annotations

import itertools
import re

from .corpus import Corpus, words_of
from .fuse import ranked
from .text import bm25_query, grep_terms

LIMIT = 50
MAX_SINGLE = 8
QUERIES = ("types", "declared", "idents", "words", "per_type", "per_member")
# One formulation, or two whose ranked lists are fused; "sum" adds the first three sections of a unit.
SETS = [[q] for q in QUERIES] + [["types", "words"], ["declared", "words"], ["idents", "words"], ["idents", "per_member"],
                                 ["declared", "per_member"], ["words", "per_type"]]
GRID = [{"queries": s, "agg": a} for s, a in itertools.product(SETS, ("max", "sum"))]
HIT = re.compile(r"^\s+\d+\. .* \(\w+\) — (?P<path>\S+?):\d+-\d+", re.M)


def queries(c: Corpus, f: dict) -> dict[str, list[str]]:
    """Queries per formulation, from the diff; identifiers are ranked by how rare they are in the docs."""

    def rare(names, table: str, n: int) -> list[str]:
        return sorted(names, key=lambda t: (-c.idf(table, t), t))[:n]

    types = sorted(f["types"], key=lambda t: (-f["types"][t], -c.idf("idents", t), t))
    members = sorted(f["members"], key=lambda t: (-f["members"][t], -c.idf("idents", t), t))
    keys = sorted(k for k in f["keys"] if k in c.df["keys"])
    idents = [t for (tab, t) in grep_terms(f, "balanced") if tab != "keys" and c.df["idents"].get(t)]
    words = bm25_query(c, f, "declared")
    words = sorted(words, key=lambda t: (-words[t] * c.word_idf(t), t))
    owner = {m: o for o, m in f["pairs"]}
    return {
        "types": [" ".join(types[:12])],
        "declared": [" ".join((types[:10] + members[:15] + keys[:5]))],
        "idents": [" ".join(rare(set(idents), "idents", 20))],
        "words": [" ".join(words[:25])],
        "per_type": types[:MAX_SINGLE],
        "per_member": [f"{owner[m]} {m}" if m in owner else m for m in members[:MAX_SINGLE]] or types[:MAX_SINGLE],
    }


def parse(answer: str) -> list[str]:
    """Docs units of an answer, in order, with repetitions (one per section)."""
    return ["framework-docs/" + m["path"] for m in HIT.finditer(answer)]


def aggregate(lists: list[list[str]], agg: str) -> dict[str, float]:
    """Sections of one or more ranked lists to a score per unit, by rank only (the same for both search modes)."""
    out: dict[str, float] = {}
    for hits in lists:
        best: dict[str, list[float]] = {}
        for i, p in enumerate(hits):
            best.setdefault(p, []).append(1 / (10 + i + 1))
        for p, xs in best.items():
            out[p] = out.get(p, 0.0) + (xs[0] if agg == "max" else sum(xs[:3]))
    return out
