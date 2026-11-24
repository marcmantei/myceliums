"""The documentation units at a commit, tokenised for the text methods (#210)."""

from __future__ import annotations

import math
import re
from functools import lru_cache
from pathlib import Path

from . import gitdata
from .change import IDENT, KEY, without_header
from .select_commits import is_unit

HEADING = re.compile(r"^={1,5} \S", re.M)
CODE_SPAN = re.compile(r"`[^`\n]+`|\[[a-z,]*source[^\]]*\]\s*\n(?:\.[^\n]*\n)?(-{4,})\n.*?\n\1|^(-{4,})\n.*?\n\2",
                       re.S | re.M)
CALL = re.compile(r"([A-Za-z_$][\w$]*)\(")
STOP = frozenset("""a an the and or of to in is are be for on with as by it its this that which can may not from at if when you your
we has have will an into also such than then there these those any all no more use used using see e g i http https www org
java javadoc link code param return throws since author version license licensed apache copyright public private static
void final new import package class interface override default null true false string object int boolean long
""".split())
CAMEL = re.compile(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+|\d+")


def stem(w: str) -> str:
    """A light suffix stripper: plural, -ing and -ed."""
    if len(w) > 4 and w.endswith("ies"):
        return w[:-3] + "y"
    if len(w) > 4 and w.endswith("ses"):
        return w[:-2]
    if len(w) > 3 and w.endswith("s") and not w.endswith(("ss", "us", "is")):
        w = w[:-1]
    if len(w) > 5 and w.endswith("ing"):
        return w[:-3]
    if len(w) > 4 and w.endswith("ed"):
        return w[:-2]
    return w


@lru_cache(maxsize=200_000)
def words_of(ident: str) -> tuple[str, ...]:
    """Lower-case stems of an identifier's camel-case parts, and the whole one when it has several."""
    parts = [p.lower() for p in CAMEL.findall(ident)]
    out = [stem(p) for p in parts if len(p) > 1 and p not in STOP]
    if len(parts) > 1:
        out.append(ident.lower().replace("_", ""))
    return tuple(out)


def count(items) -> dict[str, int]:
    out: dict[str, int] = {}
    for x in items:
        out[x] = out.get(x, 0) + 1
    return out


def bag(text: str) -> dict[str, int]:
    return count(w for t in IDENT.findall(text) for w in words_of(t))


class Unit:
    """Token counts of one file (by blob, so shared between commits)."""

    def __init__(self, text: str, adoc: bool) -> None:
        text = without_header(text)
        self.idents = count(IDENT.findall(text))
        self.keys = count(KEY.findall(text))
        if adoc:
            code = " ".join(m.group(0) for m in CODE_SPAN.finditer(text))
            self.code_idents = count(IDENT.findall(code) + CALL.findall(text))
            cuts = [0] + [m.start() for m in HEADING.finditer(text) if m.start()] + [len(text)]
            parts = [text[a:b] for a, b in zip(cuts, cuts[1:])]
        else:
            self.code_idents = self.idents
            parts = [text]
        self.sections = [b for b in map(bag, parts) if b] or [{}]
        self.length = sum(self.idents.values())
        self.sec_len = [sum(b.values()) for b in self.sections]
        self.words: dict[str, int] = {}
        for b in self.sections:
            for w, n in b.items():
                self.words[w] = self.words.get(w, 0) + n
        self.wlen = sum(self.sec_len)


def path_words(path: str) -> dict[str, int]:
    """Words of a docs path below its fixed root, or of a source path's module and file."""
    p = re.sub(r"^framework-docs/(?:modules/ROOT/(?:pages|partials)/|src/main/[a-z]+/(?:org/springframework/docs/)?)", "", path)
    p = re.sub(r"src/main/[a-z]+/org/springframework/", "", p)
    p = re.sub(r"\.[a-z]+$", "", p)
    return count(w for t in re.findall(r"[A-Za-z]+", p) for w in words_of(t))


class Corpus:
    """All docs units at one commit with document frequencies."""

    _units: dict[str, Unit] = {}

    def __init__(self, hist: Path, rev: str) -> None:
        files = {p: o for p, o in gitdata.tree(hist, rev, "framework-docs").items()
                 if is_unit(p) and not p.endswith((".png", ".svg", ".gif", ".jpg", ".graffle"))}
        new = [o for o in files.values() if o not in self._units]
        paths = {o: p for p, o in files.items()}
        for oid, data in gitdata.blobs(hist, new).items():
            self._units[oid] = Unit(data.decode(errors="replace"), paths[oid].endswith(".adoc"))
        self.units = {p: self._units[o] for p, o in sorted(files.items())}
        self.n = len(self.units)
        self.df: dict[str, dict[str, int]] = {k: {} for k in ("idents", "code_idents", "keys", "words")}
        for u in self.units.values():
            for k, df in self.df.items():
                for t in getattr(u, k):
                    df[t] = df.get(t, 0) + 1
        self.sections = [(p, i) for p, u in self.units.items() for i in range(len(u.sections))]
        self.sdf: dict[str, int] = {}
        for u in self.units.values():
            for b in u.sections:
                for t in b:
                    self.sdf[t] = self.sdf.get(t, 0) + 1
        self.avg_ident = sum(u.length for u in self.units.values()) / max(1, self.n)
        self.avg_words = sum(u.wlen for u in self.units.values()) / max(1, self.n)
        self.avg_sec = sum(sum(u.sec_len) for u in self.units.values()) / max(1, len(self.sections))
        self.paths = {p: path_words(p) for p in self.units}

    def idf(self, table: str, term: str) -> float:
        df = self.df[table].get(term, 0)
        return math.log((self.n + 1) / (df + 0.5)) if df else 0.0

    def section_idf(self, term: str) -> float:
        df = self.sdf.get(term, 0)
        return math.log(1 + (len(self.sections) - df + 0.5) / (df + 0.5)) if df else 0.0

    def word_idf(self, term: str) -> float:
        df = self.df["words"].get(term, 0)
        return math.log(1 + (self.n - df + 0.5) / (df + 0.5)) if df else 0.0
