"""Groundtruth helper for #182 (not myceliums): a crude Java call-site finder. Prints the occurrences of
`name(` and `::name` in */src/main/java with the enclosing `Type.method`, for a reader to check one by one.

usage: JAVA_ROOT=<checkout> python3 -I jcalls.py NAME [NAME ...]
"""
import re, subprocess, sys, os
ROOT = os.environ.get("JAVA_ROOT", os.path.expanduser("~/.cache/myceliums-agent-eval-182/repos/spring-framework"))
KW = {"if","for","while","switch","catch","synchronized","try","return","new","else","do","assert","throw","super","this"}

def strip(src):
    out = []; i = 0; n = len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i); j = n if j < 0 else j
            out.append(" " * (j - i)); i = j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2); j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", src[i:j])); i = j
        elif src.startswith('"""', i):
            j = src.find('"""', i + 3); j = n if j < 0 else j + 3
            out.append('"' + re.sub(r"[^\n]", " ", src[i+1:j-1]) + '"'); i = j
        elif c == '"' or c == "'":
            j = i + 1
            while j < n and src[j] != c:
                if src[j] == "\\": j += 1
                if j < n and src[j] == "\n": break
                j += 1
            out.append(c + " " * (j - i - 1) + (c if j < n else "")); i = j + 1
        else:
            out.append(c); i += 1
    return "".join(out)

def frames(code):
    """returns list of (pos, stack snapshot) events; we compute enclosing per offset lazily."""
    stack = []; events = []  # (start, end, kind, name)
    hdr_start = 0
    opens = []
    for i, c in enumerate(code):
        if c in ";":
            hdr_start = i + 1
        elif c == "{":
            hdr = code[hdr_start:i].strip()
            hdr1 = re.sub(r"\s+", " ", hdr)
            kind, name = "block", None
            m = re.search(r"(?:^|[\s>)])(class|interface|enum|record|@interface)\s+(\w+)", " " + hdr1)
            if m and not re.search(r"\bnew\s", hdr1[:m.start()]):
                kind, name = "type", m.group(2)
            elif re.search(r"\bnew\s+[\w.<>\[\], ?]+\s*\(.*\)\s*$", hdr1):
                kind = "anon"
            else:
                h = re.sub(r"\s*throws\s+[\w.,\s<>]+$", "", hdr1)
                if h.endswith(")") and stack and stack[-1][0] in ("type", "anon"):
                    d = 0
                    for k in range(len(h) - 1, -1, -1):
                        if h[k] == ")": d += 1
                        elif h[k] == "(":
                            d -= 1
                            if d == 0: break
                    mm = re.search(r"(\w+)\s*$", h[:k])
                    if mm and mm.group(1) not in KW and "->" not in h:
                        kind, name = "method", mm.group(1)
                elif stack and stack[-1][0] in ("type","anon") and re.fullmatch(r"(static)?", h):
                    kind, name = "method", "<init-block>"
            stack.append((kind, name, i))
            hdr_start = i + 1
        elif c == "}":
            if stack:
                k = stack.pop(); events.append((k[2], i, k[0], k[1]))
            hdr_start = i + 1
    return events

def enclosing(events, pos):
    encl = sorted([e for e in events if e[0] <= pos <= e[1]], key=lambda e: e[0])
    meth = None; owner = None; types = []
    for (s, e, kind, name) in encl:
        if kind == "type" and meth is None: types.append(name)
        if kind == "method" and meth is None:
            meth = name; owner = types[-1] if types else None
    return owner, meth, types

_cache = {}
def load(path):
    if path not in _cache:
        src = open(os.path.join(ROOT, path), encoding="utf-8", errors="replace").read()
        code = strip(src)
        _cache[path] = (src, code, frames(code))
    return _cache[path]

def files(scope="main"):
    out = subprocess.run(["git", "-C", ROOT, "ls-files", "*.java"], capture_output=True, text=True).stdout.split()
    if scope == "main":
        out = [f for f in out if "/src/main/java/" in f and not f.startswith(("integration-tests/","buildSrc/","framework-docs/"))]
    return out

def find(name, scope="main"):
    g = subprocess.run(["git", "-C", ROOT, "grep", "-l", "-w", name, "--", "*.java"], capture_output=True, text=True).stdout.split()
    fs = set(files(scope))
    res = []
    for f in g:
        if f not in fs: continue
        src, code, ev = load(f)
        for m in re.finditer(r"(?:(::)\s*%s\b|\b%s\s*\()" % (name, name), code):
            pos = m.start()
            # declaration? a method frame starting right after this header
            line = code.count("\n", 0, pos) + 1
            owner, meth, types = enclosing(ev, pos)
            # skip declaration: the frame whose header contains pos and kind method named name
            is_decl = any(k == "method" and nm == name and s > pos and code[pos:s].count(";") == 0 and code[pos:s].count("}") == 0 for (s, e, k, nm) in ev)
            if is_decl: continue
            text = src.splitlines()[line - 1].strip()
            res.append((f, line, f"{owner}.{meth}", text, ".".join(types)))
    return res

if __name__ == "__main__":
    for name in sys.argv[1:]:
        r = find(name)
        print(f"== {name}: {len(r)} sites")
        for f, l, enc, t, ty in r:
            print(f"  {f}:{l}  [{enc}]  {t[:140]}")
