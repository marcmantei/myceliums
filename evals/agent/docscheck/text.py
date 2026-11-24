"""Text-search baselines (#210): grep of changed identifiers, BM25 over diff terms, their grids."""

from __future__ import annotations

import itertools
import math

from .corpus import STOP, Corpus, path_words, words_of
from .change import IDENT

# Weights per term source. `declared`: the declaration line itself changes.
PROFILES = {
    "declared": {"type": 3, "member": 2, "declared": 1, "qualified": 1, "key": 3, "attribute": 2, "code": 0, "comment": 0, "test": 0},
    "balanced": {"type": 3, "member": 2, "declared": 1, "qualified": 1, "key": 3, "attribute": 2, "code": 0.5, "comment": 0.5, "test": 0},
    "types": {"type": 5, "member": 1, "declared": 0, "qualified": 2, "key": 3, "attribute": 1, "code": 0.5, "comment": 0.25, "test": 0},
    "wide": {"type": 3, "member": 2, "declared": 1, "qualified": 1, "key": 3, "attribute": 2, "code": 1, "comment": 1, "test": 0.5},
}
GREP_GRID = [{"profile": p, "idf": i, "tf": t, "b": b} for p, i, t, b in
             itertools.product(PROFILES, (1, 2), ("log", "bm25"), (0.0, 0.5, 1.0))]
QUERIES = {
    "declared": {"decl": 3, "code": 0.5, "comment": 0.5, "test": 0, "path": 1},
    "flat": {"decl": 1, "code": 1, "comment": 1, "test": 0, "path": 1},
    "wide": {"decl": 2, "code": 1, "comment": 1, "test": 0.5, "path": 1},
}
BM25_GRID = [{"query": q, "k1": k, "b": b, "unit": u, "path": w} for q, k, b, u, w in
             itertools.product(("declared", "flat"), (1.2, 2.0, 3.0), (0.6, 0.85), ("section", "both"), (0, 3))]


def grep_terms(f: dict, profile: str) -> dict[tuple[str, str], float]:
    """``{(table, term): weight}``; an identifier from several sources has its largest weight."""
    w = PROFILES[profile]
    out: dict[tuple[str, str], float] = {}

    def put(table: str, term: str, weight: float) -> None:
        if weight > 0 and len(term) > 1:
            out[table, term] = max(out.get((table, term), 0.0), weight)

    decl = set(f["declared"])
    for t in f["types"]:
        put("idents", t, w["type"] + (w["declared"] if t in decl else 0))
    for m in f["members"]:
        put("member", m, w["member"] + (w["declared"] if m in decl else 0))
        # An accessor is documented as its property, and the reverse.
        for pre in ("set", "get", "is"):
            if m.startswith(pre) and len(m) > len(pre) + 2 and m[len(pre)].isupper():
                put("member", m[len(pre)].lower() + m[len(pre) + 1:], w["member"] * 0.75)
    for owner, m in f["pairs"]:
        for sep in (".",):
            put("keys", f"{owner}{sep}{m}", w["qualified"])
    for q in f["qualified"]:
        put("keys", q, w["qualified"])
    for k in f["keys"]:
        put("keys", k, w["key"])
    for a in f["attributes"]:
        put("member", a, w["attribute"])
    for source in ("code", "comment", "test"):
        for t in f["idents"][source]:
            put("member", t, w[source])
    return out


def grep(c: Corpus, f: dict, cfg: dict) -> dict[str, float]:
    """Whole-word, case-sensitive matches of the diff's identifiers and keys, by role, rarity and frequency."""
    plan = []
    for (table, term), weight in grep_terms(f, cfg["profile"]).items():
        tab = "idents" if table == "member" else table
        idf = c.idf(tab, term)
        if idf > 0:
            plan.append((tab, term, weight * idf ** cfg["idf"]))
    scores: dict[str, float] = {}
    for path, u in c.units.items():
        s = 0.0
        norm = 1 - cfg["b"] + cfg["b"] * u.length / c.avg_ident
        for tab, term, weight in plan:
            tf = getattr(u, tab).get(term)
            if tf:
                s += weight * ((1 + math.log(tf)) / norm ** 0.5 if cfg["tf"] == "log" else 2.2 * tf / (tf + 1.2 * norm))
        if s > 0:
            scores[path] = s
    return scores


def bm25_query(c: Corpus, f: dict, query: str) -> dict[str, float]:
    w = QUERIES[query]
    q: dict[str, float] = {}

    def put(ws: dict[str, float], weight: float) -> None:
        for t, n in ws.items():
            q[t] = q.get(t, 0) + weight * (1 + math.log(n))

    def tally(idents) -> dict[str, float]:
        out: dict[str, float] = {}
        for i in idents:
            for t in words_of(i):
                out[t] = out.get(t, 0) + 1
        return out

    put(tally(list(f["types"]) + list(f["members"]) + [k for key in f["keys"] for k in IDENT.findall(key)]), w["decl"])
    for source in ("code", "comment", "test"):
        if w[source]:
            put(tally(t for ln in f["lines"][source] for t in IDENT.findall(ln)), w[source])
    put({t: n for p in f["paths"] for t, n in path_words(p).items()}, w["path"])
    return {t: x for t, x in q.items() if x > 0 and t not in STOP}


def bm25(c: Corpus, f: dict, cfg: dict) -> dict[str, float]:
    """BM25 over the words of the diff (camel-case parts, prose of comments), per section or section plus page.

    ``path`` adds the words of the page's path to every section that many times."""
    q = bm25_query(c, f, cfg["query"])
    k1, b = cfg["k1"], cfg["b"]

    def score(weights, words: dict[str, int], length: int, avg: float, boost: dict[str, int]) -> float:
        norm = k1 * (1 - b + b * length / avg)
        s = 0.0
        terms = weights if len(weights) < len(words) else words
        for t in terms:
            w = weights.get(t)
            if w:
                tf = words.get(t, 0) + cfg["path"] * boost.get(t, 0)
                if tf:
                    s += w * tf * (k1 + 1) / (tf + norm)
        for t, n in boost.items():
            if cfg["path"] and t in weights and (t not in words):
                tf = cfg["path"] * n
                if terms is words:
                    s += weights[t] * tf * (k1 + 1) / (tf + norm)
        return s

    ws = {t: w * c.section_idf(t) for t, w in q.items() if t in c.sdf}
    wp = {t: w * c.word_idf(t) for t, w in q.items() if t in c.sdf}
    scores: dict[str, float] = {}
    for path, u in c.units.items():
        boost = c.paths[path]
        s = max(score(ws, x, n, c.avg_sec, boost) for x, n in zip(u.sections, u.sec_len))
        if cfg["unit"] == "both":
            s += score(wp, u.words, u.wlen, c.avg_words, boost)
        if s > 0:
            scores[path] = s
    return scores


def plain_grep(c: Corpus, f: dict) -> dict[str, float]:
    """Untuned reference: how often the names of the touched types occur."""
    out = {}
    for path, u in c.units.items():
        n = sum(u.idents.get(t, 0) for t in f["types"])
        if n:
            out[path] = float(n)
    return out
