"""What a code diff changes, read from the diff and the two versions of its files (#210).

Input of every method: the diff of the files outside ``framework-docs/`` and the
repository at the parent. The commit message and the docs are never read.

:func:`spans` is a small scanner for Java and Kotlin declarations (types and
members with their line ranges, doc comments included). It gives the *text*
baselines the enclosing declarations of a hunk without any index.
"""

from __future__ import annotations

import re
from pathlib import Path

from . import gitdata

IDENT = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
KEY = re.compile(r"[A-Za-z][A-Za-z0-9_]*(?:[.-][A-Za-z0-9_]+)+")
STRING = re.compile(r'"((?:[^"\\\n]|\\.)*)"')
TYPE_DECL = re.compile(r"(?:^|[\s)])(?:(@interface|class|interface|enum|record|object)\s+)([A-Za-z_$][\w$]*)")
FUN_DECL = re.compile(r"\bfun\b[^(]*?([A-Za-z_]\w*|`[^`]+`)\s*\(")
METHOD_DECL = re.compile(r"([A-Za-z_$][\w$]*)\s*\(")
FIELD_DECL = re.compile(r"([A-Za-z_$][\w$]*)\s*(?:=|$|,)")
KEYWORDS = frozenset("""abstract as assert boolean break byte case catch char class companion const continue data default do
double else enum extends false final finally float for fun get goto if implements import in instanceof int interface
internal is long native new null object open operator out override package private protected public record return
sealed set short static strictfp super switch synchronized this throw throws transient true try val var void volatile
when while with yield string object integer list map set collection class override nullable""".split())
ANNOTATION_ARGS = re.compile(r"@(?!interface\b)[\w.]+\s*(?:\((?:[^()]|\((?:[^()]|\([^()]*\))*\))*\))?")


def kind(path: str) -> str:
    """``main``, ``test`` or ``other`` (build files, resources outside source sets)."""
    if "/src/main/" in path:
        return "main"
    if re.search(r"/src/(?:test|testFixtures|jmh|integrationTest)/", path) or path.startswith("integration-tests/"):
        return "test"
    return "other"


def is_docs(path: str) -> bool:
    return path.startswith("framework-docs/")


def blank(source: str) -> str:
    """Source with comments and string contents replaced by spaces (same length and lines)."""
    out, i, n = [], 0, len(source)

    def put(s: str) -> None:
        out.append("".join("\n" if ch == "\n" else " " for ch in s))

    while i < n:
        two = source[i:i + 2]
        if two == "//":
            j = source.find("\n", i)
            j = n if j < 0 else j
        elif two == "/*":
            j = source.find("*/", i + 2)
            j = n if j < 0 else j + 2
        elif source.startswith('"""', i):
            j = source.find('"""', i + 3)
            j = n if j < 0 else j + 3
        elif source[i] in "\"'":
            j = i + 1
            while j < n and source[j] != source[i] and source[j] != "\n":
                j += 2 if source[j] == "\\" else 1
            j = min(j + 1, n)
        else:
            out.append(source[i])
            i += 1
            continue
        put(source[i:j])
        i = j
    return "".join(out)


def spans(source: str) -> list[dict]:
    """Declarations of a Java or Kotlin file: ``{name, kind: type|member, annotation, owner, start, end}``,
    lines from 1; a declaration starts at its doc comment."""
    text = blank(source)
    out: list[dict] = []
    # Each open brace: ("type", span) when a type body, ("member", span) or ("other", None) otherwise.
    stack: list[tuple[str, dict | None]] = []
    head = 0
    paren = 0
    line_at = [0]
    for m in re.finditer("\n", source):
        line_at.append(m.end())

    def line(pos: int) -> int:
        lo, hi = 0, len(line_at) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            lo, hi = (mid, hi) if line_at[mid] <= pos else (lo, mid - 1)
        return lo + 1

    def start_of(a: int, b: int) -> int:
        m = re.search(r"\S", source[a:b])
        return a + (m.start() if m else 0)

    def declare(a: int, b: int) -> dict | None:
        raw = text[a:b]
        h = ANNOTATION_ARGS.sub(" ", raw)
        owner = next((s["name"] for k, s in reversed(stack) if k == "type" and s), None)
        m = TYPE_DECL.search(h)
        if m:
            span = {"name": m.group(2), "kind": "type", "annotation": m.group(1) == "@interface", "owner": owner}
        else:
            f = FUN_DECL.search(h)
            m = f or METHOD_DECL.search(h.split("=")[0])
            if m is None:
                v = re.search(r"\b(?:val|var)\s+([A-Za-z_]\w*)", h) or FIELD_DECL.search(h.strip())
                m = v
            if m is None or m.group(1) in KEYWORDS or h.strip().startswith(("import", "package")):
                return None
            span = {"name": m.group(1).strip("`"), "kind": "member", "annotation": False, "owner": owner}
        # The enclosing doc comment belongs to the declaration.
        s = start_of(a, b)
        span["start"], span["decl"], span["end"] = line(s), line(a + len(raw) - len(raw.lstrip())), line(b)
        span["head_end"] = line(b)
        out.append(span)
        return span

    i = 0
    while i < len(text):
        ch = text[i]
        inside = stack[-1][0] if stack else "file"
        if ch in "([":
            paren += 1
        elif ch in ")]":
            paren = max(0, paren - 1)
        elif paren == 0 or ch in "{}" and inside == "other":
            if ch == "{":
                if inside in ("file", "type"):
                    d = declare(head, i)
                    stack.append((d["kind"] if d else "other", d))
                else:
                    stack.append(("other", None))
                head = i + 1
            elif ch == "}":
                if stack:
                    k, d = stack.pop()
                    if d:
                        d["end"] = line(i)
                head = i + 1
            elif ch == ";":
                if inside in ("file", "type"):
                    declare(head, i)
                head = i + 1
        i += 1
    return out


def parse_diff(text: str) -> list[dict]:
    """Files of a ``git diff -U0`` with their hunks."""
    files: list[dict] = []
    cur: dict | None = None
    hunk: dict | None = None
    for ln in text.split("\n"):
        if ln.startswith("diff --git "):
            cur = {"old": None, "new": None, "hunks": []}
            files.append(cur)
            hunk = None
        elif cur is None:
            continue
        elif hunk is None and ln.startswith("--- "):
            cur["old"] = ln[6:].rstrip("\t") if ln[4:6] == "a/" else None
        elif hunk is None and ln.startswith("+++ "):
            cur["new"] = ln[6:].rstrip("\t") if ln[4:6] == "b/" else None
        elif hunk is None and ln.startswith("rename from "):
            cur["old"] = ln[12:]
        elif hunk is None and ln.startswith("rename to "):
            cur["new"] = ln[10:]
        elif ln.startswith("@@ "):
            m = re.match(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", ln)
            hunk = {"os": int(m[1]), "ol": int(m[2] or 1), "ns": int(m[3]), "nl": int(m[4] or 1),
                    "minus": [], "plus": []}
            cur["hunks"].append(hunk)
        elif hunk is not None and ln.startswith("-"):
            hunk["minus"].append(ln[1:])
        elif hunk is not None and ln.startswith("+"):
            hunk["plus"].append(ln[1:])
    return [f for f in files if f["old"] or f["new"]]


def read_files(hist: Path, parent: str, sha: str) -> list[dict]:
    """The non-docs files of a commit with hunks and both versions' text."""
    diff = gitdata.git(hist, "-c", "core.quotePath=off", "diff", "-U0", "-M50%", "--no-color", "--no-ext-diff",
                       "--no-textconv", parent, sha, "--", ".", ":(exclude)framework-docs").decode(errors="replace")
    files = parse_diff(diff)
    old, new = gitdata.tree(hist, parent), gitdata.tree(hist, sha)
    want = {old[f["old"]] for f in files if f["old"] in old} | {new[f["new"]] for f in files if f["new"] in new}
    text = gitdata.blobs(hist, want)
    for f in files:
        f["old_text"] = text[old[f["old"]]].decode(errors="replace") if f["old"] in old else ""
        f["new_text"] = text[new[f["new"]]].decode(errors="replace") if f["new"] in new else ""
        f["kind"] = kind(f["new"] or f["old"])
    return files


def _overlap(s: dict, start: int, length: int) -> bool:
    return s["start"] <= start + max(length, 1) - 1 and start <= s["end"]


def innermost(decls: list[dict], line: int, member: bool = False) -> list[dict]:
    """Declarations around a line, outermost first."""
    return sorted((s for s in decls if s["start"] <= line <= s["end"]), key=lambda s: (s["start"], -s["end"]))


def touched(decls: list[dict], hunks: list[dict], side: str) -> tuple[list[dict], list[dict]]:
    """``(touched, declared)``: declarations around the changed lines, and those whose
    declaration line itself changes. ``side`` is ``o`` (old) or ``n`` (new)."""
    hit: dict[int, dict] = {}
    declared: dict[int, dict] = {}
    for h in hunks:
        start, length = h[side + "s"], h[side + "l"]
        lines = range(start, start + length) if length else ()
        for ln in lines:
            around = innermost(decls, ln)
            for s in around[-1:] + [s for s in around if s["kind"] == "type"][-1:]:
                hit[id(s)] = s
        for s in decls:
            if length and _overlap({"start": s["decl"], "end": s["head_end"]}, start, length):
                declared[id(s)] = s
                hit[id(s)] = s
    return list(hit.values()), list(declared.values())


def is_comment(line: str) -> bool:
    return line.strip().startswith(("*", "/*", "//", "<!--"))


def package_of(text: str) -> str:
    m = re.search(r"^\s*package\s+([\w.]+)", text, re.M)
    return m.group(1) if m else ""


def features(files: list[dict]) -> dict:
    """Term sources of a diff, each a sorted list or a ``{term: count}`` map."""
    types: dict[str, float] = {}
    members: dict[str, float] = {}
    pairs: set[tuple[str, str]] = set()
    qualified: set[str] = set()
    attributes: set[str] = set()
    declared: set[str] = set()
    keys: dict[str, int] = {}
    counts = {k: {} for k in ("code", "comment", "test", "other")}
    words = {k: [] for k in ("code", "comment", "test", "other")}
    paths: list[str] = []
    for f in files:
        path = f["new"] or f["old"]
        src = path.endswith((".java", ".kt"))
        if f["kind"] == "main":
            paths.append(path)
        for h in f["hunks"]:
            for ln in h["minus"] + h["plus"]:
                if src and ln.strip().startswith(("import ", "package ")):
                    continue
                if f["kind"] == "main":
                    bucket = "comment" if src and is_comment(ln) else "code"
                else:
                    bucket = f["kind"]
                words[bucket].append(ln)
                c = counts[bucket]
                for t in IDENT.findall(ln):
                    if t.lower() not in KEYWORDS and len(t) > 1:
                        c[t] = c.get(t, 0) + 1
                if f["kind"] == "main" and bucket == "code":
                    lits = STRING.findall(ln) if src else [ln]
                    for lit in lits:
                        for k in KEY.findall(lit):
                            keys[k] = keys.get(k, 0) + 1
        if not (src and f["kind"] == "main"):
            continue
        for side, text in (("o", f["old_text"]), ("n", f["new_text"])):
            if not text:
                continue
            decls = spans(text)
            pkg = package_of(text)
            hit, decl = touched(decls, f["hunks"], side)
            annotations = {s["name"] for s in decls if s["annotation"]}
            for s in hit:
                w = 2.0 if any(s is d for d in decl) else 1.0
                if s["kind"] == "type":
                    types[s["name"]] = max(types.get(s["name"], 0), w)
                    if not s["owner"]:
                        qualified.add(f"{pkg}.{s['name']}")
                else:
                    if s["owner"] == s["name"]:
                        continue  # a constructor is its type
                    members[s["name"]] = max(members.get(s["name"], 0), w)
                    if s["owner"]:
                        pairs.add((s["owner"], s["name"]))
                        types.setdefault(s["owner"], 1.0)
                        if s["owner"] in annotations:
                            attributes.add(s["name"])
                if w == 2.0:
                    declared.add(s["name"])
        stem = path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
        types.setdefault(stem, 1.0)
    return {"types": types, "members": members, "pairs": sorted(pairs), "qualified": sorted(qualified),
            "attributes": sorted(attributes), "declared": sorted(declared), "keys": keys, "idents": counts,
            "lines": words, "paths": paths}
