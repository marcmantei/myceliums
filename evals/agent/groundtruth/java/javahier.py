"""Groundtruth helper for #182 (not myceliums): Java type hierarchy and method declarations
from a plain-text scan of production sources (*/src/main/java/**/*.java).

usage: python3 -I javahier.py ROOT impls INTERFACE METHOD   -> types declaring METHOD body that are subtypes of INTERFACE
       python3 -I javahier.py ROOT subtypes TYPE
       python3 -I javahier.py ROOT anon TYPE                -> anonymous classes `new TYPE(...) {` (any subtype name)
"""
import json, re, subprocess, sys
from pathlib import Path

KW = {"if","for","while","switch","catch","return","new","throw","synchronized","else","do","try","case","super","this"}

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
            out.append(re.sub(r"[^\n]", " ", src[i:j])); i = j
        elif c == '"' or c == "'":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            j += 1
            out.append(c + " " * max(0, j - i - 2) + c); i = j
        else:
            out.append(c); i += 1
    return "".join(out)

TOK = re.compile(r"[A-Za-z_$][\w$]*|@|\{|\}|\(|\)|;|<|>|,|\.|=|->|::|\S")

def toks(text):
    line = 1; last = 0
    for m in TOK.finditer(text):
        line += text.count("\n", last, m.start()); last = m.start()
        yield m.group(0), line

def split_types(tl):
    """names in an extends/implements list: top-level comma separated, generics dropped, last dotted part"""
    out = []; depth = 0; cur = []
    for t in tl:
        if t == "<": depth += 1; continue
        if t == ">": depth -= 1; continue
        if depth: continue
        if t == ",":
            if cur: out.append(cur[-1])
            cur = []; continue
        if t == ".": continue
        if re.match(r"[A-Za-z_$]", t): cur.append(t)
    if cur: out.append(cur[-1])
    return out

def parse(path, rel):
    raw = path.read_text(encoding="utf-8", errors="replace")
    text = strip(raw)
    pkg = (re.search(r"^\s*package\s+([\w.]+)\s*;", text, re.M) or [None, ""])[1]
    imports = re.findall(r"^\s*import\s+(?!static)([\w.]+(?:\.\*)?)\s*;", text, re.M)
    T = list(toks(text))
    types = []  # dict(name, kind, extends, implements, file, line, outer, methods{name:[(line,has_body,abstract)]}, anon)
    stack = []  # frames: ("type", typeobj) or ("block", None)
    i = 0; n = len(T)
    member = []
    def cur_type():
        for kind, obj in reversed(stack):
            if kind == "type": return obj
        return None
    while i < n:
        t, ln = T[i]
        top = stack[-1] if stack else None
        in_body = top is not None and top[0] == "type"
        # type declaration
        if t in ("class","interface","enum","record") and i + 1 < n and re.match(r"[A-Za-z_$]", T[i+1][0]) and not (i > 0 and T[i-1][0] in (".", "::")):
            name = T[i+1][0]; j = i + 2; ext = []; imp = []; mode = None; depth = 0
            while j < n and not (T[j][0] == "{" and depth == 0):
                tj = T[j][0]
                if tj in ("(", "<"): depth += 1
                elif tj in (")", ">"): depth -= 1
                elif depth == 0 and tj == "extends": mode = "e"; j += 1; continue
                elif depth == 0 and tj == "implements": mode = "i"; j += 1; continue
                elif depth == 0 and tj == "permits": mode = "p"; j += 1; continue
                elif tj == ";": break
                if mode == "e": ext.append(tj)
                elif mode == "i": imp.append(tj)
                elif mode == "p": pass
                if mode and depth and tj in ("<", ">"): pass
                j += 1
            # re-collect with generics markers for split
            def collect(kw):
                out = []; k = i + 2; d = 0; on = False
                while k < j:
                    tk = T[k][0]
                    if tk in ("(",): d += 1
                    if tk in (")",): d -= 1
                    if d == 0 and tk in ("extends","implements","permits") :
                        on = (tk == kw); k += 1; continue
                    if on: out.append(tk)
                    k += 1
                # drop leading generic params of the declaration itself handled since we start after name
                return split_types(out)
            # class generic params: `class X<T extends Y>` -> extends inside <> must be ignored: collect handles d only for (); handle <>
            def collect2(kw):
                out = []; k = i + 2; d = 0; on = False
                while k < j:
                    tk = T[k][0]
                    if tk in ("(", "<"): d += 1
                    elif tk in (")", ">"): d -= 1
                    elif d == 0 and tk in ("extends","implements","permits"):
                        on = (tk == kw); k += 1; continue
                    if on: out.append(tk)
                    k += 1
                return split_types(out)
            if j < n and T[j][0] == "{":
                outer = cur_type()
                ty = {"pkg": pkg, "imports": imports, "outer_fqn": outer["fqn"] if outer else None,
                      "fqn": (outer["fqn"] + "." + name) if outer else (pkg + "." + name),
                      "name": name, "kind": t, "extends": collect2("extends"), "implements": collect2("implements"),
                      "file": rel, "line": ln, "outer": outer["name"] if outer else None, "methods": {}, "anon": False}
                types.append(ty); stack.append(("type", ty)); member = []; i = j + 1; continue
        # anonymous class: new X(...) {
        if t == "new" and i + 1 < n:
            j = i + 1; names = []
            while j < n and (re.match(r"[A-Za-z_$]", T[j][0]) or T[j][0] == "."):
                if T[j][0] != ".": names.append(T[j][0])
                j += 1
            if j < n and T[j][0] == "<":
                d = 0
                while j < n:
                    if T[j][0] == "<": d += 1
                    elif T[j][0] == ">": d -= 1
                    j += 1
                    if d == 0: break
            if names and j < n and T[j][0] == "(":
                d = 0
                while j < n:
                    if T[j][0] == "(": d += 1
                    elif T[j][0] == ")": d -= 1
                    j += 1
                    if d == 0: break
                if j < n and T[j][0] == "{":
                    outer = cur_type()
                    ty = {"pkg": pkg, "imports": imports, "outer_fqn": outer["fqn"] if outer else None,
                          "fqn": f"<anon {names[-1]} in {outer['fqn'] if outer else '?'}:{ln}>",
                          "name": f"<anon {names[-1]} in {outer['name'] if outer else '?'}:{ln}>", "kind": "anon",
                          "extends": [names[-1]], "implements": [], "file": rel, "line": ln,
                          "outer": outer["name"] if outer else None, "methods": {}, "anon": True, "anon_of": names[-1]}
                    types.append(ty); stack.append(("type", ty)); member = []; i = j + 1; continue
        if t == "{":
            if in_body:
                # member with body: method/ctor/initializer/enum-const body
                ty = top[1]
                mname = method_name(member)
                if mname:
                    ty["methods"].setdefault(mname, []).append((member_line(member, mname), True))
            stack.append(("block", None)); member = []; i += 1; continue
        if t == "}":
            if stack: stack.pop()
            member = []; i += 1; continue
        if t == ";" and in_body:
            ty = top[1]
            mname = method_name(member)
            if mname:
                ty["methods"].setdefault(mname, []).append((member_line(member, mname), False))
            member = []; i += 1; continue
        if in_body:
            member.append((t, ln))
        i += 1
    return types

def method_name(member):
    ts = [t for t, _ in member]
    idx = [i for i, t in enumerate(ts) if t == "(" and i > 0 and re.match(r"[A-Za-z_$]", ts[i-1])
           and not (i >= 2 and ts[i-2] in ("@", ".")) and ts[i-1] not in KW]
    if not idx: return None
    k = idx[0]
    if "=" in ts[:k] or "->" in ts[:k]: return None
    return ts[k-1]

def member_line(member, name):
    for t, ln in member:
        if t == name: return ln
    return member[0][1] if member else 0

def load(root):
    root = Path(root)
    files = subprocess.run(["git", "-C", str(root), "ls-files", "*/src/main/java/*.java"], capture_output=True, text=True).stdout.split()
    types = []
    for f in files:
        if f.startswith(("framework-docs/", "buildSrc/", "integration-tests/")): continue
        types.extend(parse(root / f, f))
    return types

def main():
    root, cmd = sys.argv[1], sys.argv[2]
    types = load(root)
    by = {}
    for t in types:
        by.setdefault(t["name"], []).append(t)
    fq = {t["fqn"]: t for t in types}
    pkgs = {}
    for t in types:
        if not t["anon"] and t["outer_fqn"] is None: pkgs.setdefault(t["pkg"], set()).add(t["name"])
    def resolve(name, t):
        # nested in enclosing chain
        o = t.get("outer_fqn") if not t["anon"] else t.get("outer_fqn")
        chain = []
        x = t["fqn"] if not t["anon"] else t.get("outer_fqn")
        while x:
            chain.append(x); x = fq[x]["outer_fqn"] if x in fq else None
        for c in chain:
            if c + "." + name in fq: return c + "." + name
            # inherited nested types ignored
        for imp in t["imports"]:
            if imp.endswith("." + name): return imp if imp in fq else imp
            if imp.endswith("." + name.split(".")[0]): return imp
        if t["pkg"] + "." + name in fq: return t["pkg"] + "." + name
        for imp in t["imports"]:
            if imp.endswith(".*") and imp[:-2] + "." + name in fq: return imp[:-2] + "." + name
        return "?" + name
    sup = {}
    for t in types:
        sup[t["fqn"]] = [resolve(n, t) for n in t["extends"] + t["implements"]]
    def is_sub(t, target, seen=None):
        seen = seen if seen is not None else set()
        for s in sup[t["fqn"]]:
            if s == target: return True
            if s in seen: continue
            seen.add(s)
            if s in fq and is_sub(fq[s], target, seen): return True
        return False
    if cmd == "subtypes":
        target = sys.argv[3]
        for t in types:
            if is_sub(t, target):
                print(t["kind"], t["fqn"], t["file"], t["line"], "->", sup[t["fqn"]], sorted(t["methods"]) if len(sys.argv) > 4 else "")
    elif cmd in ("impls", "batch"):
      jobs = [l.split() for l in sys.stdin if l.strip()] if cmd == "batch" else [(sys.argv[3], sys.argv[4])]
      for target, meth in jobs:
        print("===", target, meth)
        for t in types:
            if (is_sub(t, target) or t["fqn"] == target) and meth in t["methods"]:
                body = [x for x in t["methods"][meth]]
                print(t["kind"], t["fqn"], t["file"], body, "dup" if len(by.get(t["name"], [])) > 1 else "")
    elif cmd == "dups":
        for k, v in by.items():
            if len(v) > 1 and not k.startswith("<"): print(k, [x["file"] for x in v])

main()
