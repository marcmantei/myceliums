"""Gold sets for the hard multi-hop tasks (#125), computed from call graphs.

The call graphs come from compiler tooling, never from myceliums:

* fzf (Go): ``gotool <repo> edges`` (go/packages + SSA; every function,
  closures included), run for GOOS=darwin, linux and windows, with and
  without test packages (``WITH_TESTS=1``);
* hono (TypeScript): ``node ts_edges.js <repo>`` (the type checker resolves
  each call through imports, re-exports and receiver types);
* FastAPI (Python): ``python3 -I py_edges.py <repo> fastapi`` (static name
  resolution through imports, ``self``/``cls``/``super()`` and class names;
  every gold set built from it was checked against grep of each name).

    python3 -I hard_tasks.py <edges-dir> <repos-dir> v1 > ../tasks/tasks-hard-v1.json   # calibrated, at the ceiling
    python3 -I hard_tasks.py <edges-dir> <repos-dir> v2 > ../tasks/tasks-hard.json      # the matrix's set

``<edges-dir>`` holds ``fzf/edges-{darwin,linux,windows}.tsv``,
``fzf/edges-tests-{...}.tsv``, ``hono/edges.tsv`` and ``fastapi/edges.tsv``.
For every gold item the builder records evidence lines (a call site on the
chain, or the definition of an end point) that ``agent_eval.py check-tasks``
re-verifies against the pinned checkouts.

Only edges of kind ``static`` (Go), ``direct`` (TypeScript) and ``direct``
(Python) are followed: calls through interface values, function values and
callbacks are not, and every question says so.
"""

from __future__ import annotations

import collections
import json
import os
import re
import sys

# --------------------------------------------------------------------------
# Graph loading: nodes are language-specific keys, `name` gives the answer
# spelling (`Type.method` / `function`).
# --------------------------------------------------------------------------


class Graph:
    def __init__(self):
        self.out = collections.defaultdict(set)
        self.inc = collections.defaultdict(set)
        self.sites = collections.defaultdict(set)  # (a, b) -> {"file:line"}

    def add(self, a, b, site):
        if a == b:
            return
        self.out[a].add(b)
        self.inc[b].add(a)
        self.sites[(a, b)].add(site)

    def callers_upto(self, target, depth):
        seen, frontier = {}, {target}
        for d in range(1, depth + 1):
            nxt = set()
            for t in frontier:
                for c in self.inc.get(t, ()):
                    if c not in seen and c != target:
                        seen[c] = d
                        nxt.add(c)
            frontier = nxt
        return seen

    def reach(self, start, forward=True):
        g = self.out if forward else self.inc
        seen, stack = {start}, [start]
        while stack:
            x = stack.pop()
            for y in g.get(x, ()):
                if y not in seen:
                    seen.add(y)
                    stack.append(y)
        return seen


def go_norm(fn: str) -> str:
    s = re.sub(r"\$(bound|thunk)$", "", fn)
    s = re.sub(r"(\$\d+)+$", "", s)  # closures belong to the enclosing function
    s = re.sub(r"\[.*?\]", "", s)
    m = re.match(r"^\(\*?([\w./-]+)\.(\w+)\)\.(\w+)$", s)
    if m:
        return f"{m.group(1).split('/')[-1]}:{m.group(2)}.{m.group(3)}"
    m = re.match(r"^([\w./-]+)\.(\w+)$", s)
    if m:
        return f"{m.group(1).split('/')[-1]}:{m.group(2)}"
    return s


def load_go(paths):
    g = Graph()
    for p in paths:
        for line in open(p):
            a, b, site, kind = line.rstrip("\n").split("\t")
            if kind == "static":
                g.add(go_norm(a), go_norm(b), site)
    return g


def load_ts(path, tests):
    g = Graph()
    for line in open(path):
        a, b, site, kind, bfile = line.rstrip("\n").split("\t")
        afile = site.rsplit(":", 1)[0]
        if kind != "direct" or (not tests and ".test." in afile):
            continue
        g.add((afile, a), (bfile, b), site)
    return g


def load_py(path):
    g = Graph()
    for line in open(path):
        a, b, site, kind = line.rstrip("\n").split("\t")
        if kind == "direct" and site.startswith("fastapi/"):
            g.add(a, b, site)
    return g


def name_of(lang, key):
    if lang == "go":
        return key.split(":", 1)[1]
    if lang == "ts":
        return key[1]
    return key.split(":", 1)[1]


def short(lang, key):
    n = name_of(lang, key).split(".")[-1]
    if lang == "ts" and n == "constructor":
        return name_of(lang, key).split(".")[0]
    if lang == "py" and n == "__init__":
        return name_of(lang, key).split(".")[0]
    return n.lstrip("#")


# --------------------------------------------------------------------------
# Evidence
# --------------------------------------------------------------------------


def line_at(repos, repo, file, line):
    with open(os.path.join(repos, repo, file), encoding="utf-8", errors="replace") as fh:
        return fh.read().splitlines()[line - 1]


def call_evidence(repos, repo, lang, g, a, b):
    """One call site from a to b whose line names b."""
    want = short(lang, b)
    for site in sorted(g.sites[(a, b)], key=lambda s: (s.rsplit(":", 1)[0], int(s.rsplit(":", 1)[1]))):
        file, ln = site.rsplit(":", 1)
        text = line_at(repos, repo, file, int(ln))
        if want in text:
            return {"file": file, "line": int(ln), "contains": want}
    # multi-line call expressions: the callee name sits on a later line
    file, ln = sorted(g.sites[(a, b)])[0].rsplit(":", 1)
    for k in range(int(ln), int(ln) + 4):
        if want in line_at(repos, repo, file, k):
            return {"file": file, "line": k, "contains": want}
    # aliased imports (`serveStatic as baseServeStatic`): quote the call line
    text = line_at(repos, repo, file, int(ln)).strip()
    return {"file": file, "line": int(ln), "contains": text[:60]}


DEF_PATTERNS = {
    "go": lambda n: [rf"^func (\([^)]*\) )?{re.escape(n)}\b"],
    "ts": lambda n: [rf"\b(function|const|let)\s+{re.escape(n)}\b", rf"^\s*(async\s+)?#?{re.escape(n)}\s*[=(<]"],
    "py": lambda n: [rf"^\s*(async\s+)?def {re.escape(n)}\("],
}


def def_evidence(repos, repo, lang, file, key):
    n = name_of(lang, key).split(".")[-1]
    with open(os.path.join(repos, repo, file), encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    for pat in DEF_PATTERNS[lang](n.lstrip("#")):
        for i, text in enumerate(lines, 1):
            if re.search(pat, text):
                return {"file": file, "line": i, "contains": n.lstrip("#")}
    raise SystemExit(f"no definition line for {key} in {file}")


# --------------------------------------------------------------------------
# Gold set builders
# --------------------------------------------------------------------------


def gold_callers(repos, repo, lang, g, target, depth, keep=lambda k: True):
    c = {k: d for k, d in g.callers_upto(target, depth).items() if keep(k)}
    gold, seen = [], set()
    for k, d in sorted(c.items(), key=lambda x: (x[1], name_of(lang, x[0]), str(x[0]))):
        if name_of(lang, k) in seen:
            # Answers are names: two functions of one name (say the `serveStatic`
            # of several adapters) are one answer item, at the lowest level.
            continue
        seen.add(name_of(lang, k))
        hops = sorted((x for x in g.out[k] if x == target or c.get(x) == d - 1), key=str)
        gold.append({"name": name_of(lang, k), "level": d,
                     "evidence": [call_evidence(repos, repo, lang, g, k, hops[0])],
                     "via": name_of(lang, hops[0])})
    return gold


def gold_caller_files(repos, repo, lang, g, target, depth, keep=lambda k: True):
    c = {k: d for k, d in g.callers_upto(target, depth).items() if keep(k)}
    files = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], str(x[0]))):
        for hop in sorted((x for x in g.out[k] if x == target or c.get(x) == d - 1), key=str):
            for site in sorted(g.sites[(k, hop)]):
                f = site.rsplit(":", 1)[0]
                if f not in files:
                    ev = call_evidence(repos, repo, lang, g, k, hop)
                    if ev["file"] != f:
                        continue
                    files[f] = {"name": f, "level": d, "evidence": [ev],
                                "via": f"{name_of(lang, k)} -> {name_of(lang, hop)}"}
    return sorted(files.values(), key=lambda x: (x["level"], x["name"]))


def gold_flow(repos, repo, lang, g, a, b, file_of):
    between = g.reach(a, True) & g.reach(b, False)
    gold = []
    for k in sorted(between, key=lambda x: name_of(lang, x)):
        nxt = sorted((x for x in g.out[k] if x in between), key=str)
        if nxt:
            ev = call_evidence(repos, repo, lang, g, k, nxt[0])
            via = name_of(lang, nxt[0])
        else:
            ev = def_evidence(repos, repo, lang, file_of(k), k)
            via = None
        gold.append({"name": name_of(lang, k), "evidence": [ev], "via": via})
    return gold


def gold_go_tests(repos, g, target):
    c = g.callers_upto(target, 50)
    gold = []
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0])):
        if not name_of("go", k).startswith("Test"):
            continue
        hops = sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1)
        gold.append({"name": name_of("go", k), "level": d,
                     "evidence": [call_evidence(repos, "fzf", "go", g, k, hops[0])],
                     "via": name_of("go", hops[0])})
    return gold


def gold_ts_test_files(repos, g, target):
    c = g.callers_upto(target, 50)
    files = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], str(x[0]))):
        if ".test." not in k[0] or k[0] in files:
            continue
        hops = sorted((x for x in g.out[k] if x == target or c.get(x) == d - 1), key=str)
        files[k[0]] = {"name": k[0], "level": d,
                       "evidence": [call_evidence(repos, "hono", "ts", g, k, hops[0])],
                       "via": name_of("ts", hops[0])}
    return sorted(files.values(), key=lambda x: (x["level"], x["name"]))


def gold_starlette_http_exception(repos, py, depth=3):
    """Functions that construct Starlette's HTTPException (not FastAPI's
    subclass), directly or through at most two intermediate calls.

    The class is third-party, so py_edges has no node for it: level 1 is
    every ``HTTPException(...)`` call in a module that imports the name from
    ``starlette.exceptions``; higher levels follow the py_edges graph.
    """
    import ast
    root = os.path.join(repos, "fastapi")
    level1 = {}
    for dp, dn, fn in os.walk(os.path.join(root, "fastapi")):
        dn.sort()
        for f in sorted(fn):
            if not f.endswith(".py"):
                continue
            path = os.path.join(dp, f)
            rel = os.path.relpath(path, root)
            tree = ast.parse(open(path, encoding="utf-8").read())
            if not any(isinstance(n, ast.ImportFrom) and n.module == "starlette.exceptions"
                       and any(a.name == "HTTPException" and a.asname is None for a in n.names) for n in tree.body):
                continue
            mod = rel[:-3].replace(os.sep, ".")

            def visit(node, owner, cls):
                for ch in ast.iter_child_nodes(node):
                    o = owner
                    if isinstance(ch, ast.ClassDef) and owner == "<module>":
                        visit(ch, owner, ch.name)
                        continue
                    if isinstance(ch, (ast.FunctionDef, ast.AsyncFunctionDef)) and owner == "<module>":
                        o = f"{cls}.{ch.name}" if cls else ch.name
                    if isinstance(ch, ast.Call) and isinstance(ch.func, ast.Name) and ch.func.id == "HTTPException":
                        level1.setdefault(f"{mod}:{o}", (rel, ch.lineno))
                    visit(ch, o, cls)
            visit(tree, "<module>", None)
    levels = {k: 1 for k in level1}
    frontier = set(level1)
    for d in range(2, depth + 1):
        nxt = set()
        for t in frontier:
            for c in py.inc.get(t, ()):
                if c not in levels:
                    levels[c] = d
                    nxt.add(c)
        frontier = nxt
    gold = []
    for k, d in sorted(levels.items(), key=lambda x: (x[1], x[0])):
        if d == 1:
            rel, ln = level1[k]
            ev, via = {"file": rel, "line": ln, "contains": "HTTPException("}, "starlette HTTPException"
        else:
            hop = sorted(x for x in py.out[k] if levels.get(x) == d - 1)[0]
            ev, via = call_evidence(repos, "fastapi", "py", py, k, hop), name_of("py", hop)
        gold.append({"name": name_of("py", k), "level": d, "evidence": [ev], "via": via})
    return gold


def main(edges_dir, repos, version="v2"):
    fz = lambda pre: [os.path.join(edges_dir, "fzf", f"{pre}{goos}.tsv") for goos in ("darwin", "linux", "windows")]
    go = load_go(fz("edges-"))
    go_t = load_go(fz("edges-tests-"))
    ts = load_ts(os.path.join(edges_dir, "hono", "edges.tsv"), tests=False)
    ts_t = load_ts(os.path.join(edges_dir, "hono", "edges.tsv"), tests=True)
    py = load_py(os.path.join(edges_dir, "fastapi", "edges.tsv"))
    py_file = lambda k: k.split(":")[0].replace(".", "/") + ".py"
    no_mod = lambda k: k[1] != "<module>"

    out = {}
    out["fzf-tc-matchitem"] = gold_callers(repos, "fzf", "go", go, "src:Pattern.MatchItem", 3)
    out["fzf-blast-istty"] = gold_caller_files(repos, "fzf", "go", go, "util:IsTty", 2)
    out["fzf-blast-stringbytes"] = gold_caller_files(repos, "fzf", "go", go, "src:stringBytes", 2)
    out["fzf-amb-chars-length"] = gold_callers(repos, "fzf", "go", go, "util:Chars.Length", 1)
    out["fzf-amb-noseparatorline"] = gold_callers(repos, "fzf", "go", go, "src:Terminal.noSeparatorLine", 1)
    out["fzf-tests-awktokenizer"] = gold_go_tests(repos, go_t, "src:awkTokenizer")
    out["fzf-tests-splitnth"] = gold_go_tests(repos, go_t, "src:splitNth")
    out["fzf-flow-ansi"] = gold_flow(repos, "fzf", "go", go, "src:NewTerminal", "src:matchControlSequence",
                                     lambda k: "src/ansi.go")

    out["hono-tc-decodebase64"] = gold_callers(repos, "hono", "ts", ts, ("src/utils/encode.ts", "decodeBase64"), 2, no_mod)
    out["hono-blast-newresponse"] = gold_caller_files(repos, "hono", "ts", ts, ("src/context.ts", "Context.newResponse"), 2)
    out["hono-blast-httpexception"] = gold_caller_files(repos, "hono", "ts", ts, ("src/http-exception.ts", "HTTPException.constructor"), 2)
    out["hono-amb-req-header"] = gold_callers(repos, "hono", "ts", ts, ("src/request.ts", "HonoRequest.header"), 1,
                                              lambda k: not k[0].startswith("src/adapter/"))
    out["hono-tests-buffertoformdata"] = gold_ts_test_files(repos, ts_t, ("src/utils/buffer.ts", "bufferToFormData"))
    out["hono-tests-serialize"] = gold_ts_test_files(repos, ts_t, ("src/utils/cookie.ts", "_serialize"))
    out["hono-flow-validator"] = gold_flow(repos, "hono", "ts", ts, ("src/validator/validator.ts", "validator"),
                                           ("src/utils/url.ts", "tryDecodeURIComponent"), lambda k: k[0])

    out["fastapi-tc-oauth-scopes"] = gold_callers(repos, "fastapi", "py", py, "fastapi.dependencies.models:_get_oauth_scopes", 3)
    out["fastapi-tc-annotation-sequence"] = gold_callers(repos, "fastapi", "py", py, "fastapi._compat.shared:field_annotation_is_sequence", 3)
    out["fastapi-flow-openapi-path"] = gold_flow(repos, "fastapi", "py", py, "fastapi.openapi.utils:get_openapi_path",
                                                 "fastapi._compat.shared:lenient_issubclass", py_file)
    out["fastapi-flow-openapi-cache-key"] = gold_flow(repos, "fastapi", "py", py, "fastapi.applications:FastAPI.openapi",
                                                      "fastapi.dependencies.models:_get_cache_key", py_file)
    # FastAPI's own HTTPException (fastapi/exceptions.py), not Starlette's: the
    # two security modules that import it construct it in
    # make_not_authenticated_error; every __call__ that runs that
    # implementation (inherited, not overridden) is one more level.
    http_exc = "fastapi.exceptions:HTTPException.__init__"
    makers = [k for k in py.inc[http_exc]]
    gold = []
    for m in sorted(makers):
        gold.append({"name": name_of("py", m), "level": 1,
                     "evidence": [call_evidence(repos, "fastapi", "py", py, m, http_exc)], "via": "HTTPException"})
    for m in sorted(makers):
        for c in sorted(py.inc[m]):
            gold.append({"name": name_of("py", c), "level": 2,
                         "evidence": [call_evidence(repos, "fastapi", "py", py, c, m)], "via": name_of("py", m)})
    out["fastapi-amb-httpexception"] = gold
    # v2 (#125 matrix): the calibration found every v1 task at the ceiling, so
    # the same questions run deeper / wider.
    out["fzf-tc-tochars-d4"] = gold_callers(repos, "fzf", "go", go, "util:ToChars", 4)
    out["fzf-blast-istty-d3"] = gold_caller_files(repos, "fzf", "go", go, "util:IsTty", 3)
    out["fzf-blast-mayfold-d4"] = gold_caller_files(repos, "fzf", "go", go, "util:MayFoldToAscii", 4)
    out["fzf-amb-chars-length-d3"] = gold_callers(repos, "fzf", "go", go, "util:Chars.Length", 3)
    out["fzf-amb-merger-get-d3"] = gold_callers(repos, "fzf", "go", go, "src:Merger.Get", 3)
    out["fzf-tests-stringbytes"] = gold_go_tests(repos, go_t, "src:stringBytes")
    out["fzf-tests-chars-tostring"] = gold_go_tests(repos, go_t, "util:Chars.ToString")
    out["fzf-flow-newterminal-stringbytes"] = gold_flow(repos, "fzf", "go", go, "src:NewTerminal", "src:stringBytes",
                                                        lambda k: "src/functions.go")
    out["hono-tc-trydecode-d3"] = gold_callers(repos, "hono", "ts", ts, ("src/utils/url.ts", "tryDecode"), 3, no_mod)
    out["hono-blast-createresponse-d3"] = gold_caller_files(repos, "hono", "ts", ts, ("src/context.ts", "createResponseInstance"), 3)
    out["hono-blast-httpexception-d3"] = gold_caller_files(repos, "hono", "ts", ts, ("src/http-exception.ts", "HTTPException.constructor"), 3)
    out["hono-amb-ctx-header-d2"] = gold_callers(repos, "hono", "ts", ts, ("src/context.ts", "Context.header"), 2, no_mod)
    out["hono-tests-req-header"] = gold_ts_test_files(repos, ts_t, ("src/request.ts", "HonoRequest.header"))
    out["hono-tests-trydecode"] = gold_ts_test_files(repos, ts_t, ("src/utils/url.ts", "tryDecode"))
    out["fastapi-tc-oauth-scopes-d5"] = gold_callers(repos, "fastapi", "py", py, "fastapi.dependencies.models:_get_oauth_scopes", 5)
    out["fastapi-tc-annotation-sequence-d5"] = gold_callers(repos, "fastapi", "py", py, "fastapi._compat.shared:field_annotation_is_sequence", 5)
    out["fastapi-flow-openapi-lenient"] = gold_flow(repos, "fastapi", "py", py, "fastapi.applications:FastAPI.openapi",
                                                    "fastapi._compat.shared:lenient_issubclass", py_file)
    out["fastapi-flow-route-state"] = gold_flow(repos, "fastapi", "py", py, "fastapi.routing:_populate_api_route_state",
                                                "fastapi._compat.shared:_annotation_is_sequence", py_file)
    out["fastapi-flow-websocket-app"] = gold_flow(repos, "fastapi", "py", py, "fastapi.routing:get_websocket_app",
                                                  "fastapi._compat.shared:field_annotation_is_sequence", py_file)
    out["fastapi-amb-starlette-httpexception"] = gold_starlette_http_exception(repos, py)

    specs = SPECS_V1 if version == "v1" else SPECS_V2
    tasks = []
    for spec in specs:
        gold = out[spec["id"]]
        items = [{"name": g["name"], "evidence": g["evidence"],
                  "note": (f"level {g['level']}" if "level" in g else "on the chain")
                  + (f", via {g['via']}" if g.get("via") else "")} for g in gold]
        tasks.append({
            "id": spec["id"], "repo": spec["repo"], "type": spec["type"], "answer_kind": spec["kind"],
            "question": spec["question"], "gold": items,
            "rationale": spec["rationale"].format(n=len(items)),
            "verified_with": VERIFIED[spec["repo"]],
        })
    json.dump({"description": DESCRIPTION, "tasks": tasks}, sys.stdout, indent=1, ensure_ascii=False)
    print()


DESCRIPTION = (
    "Hard multi-hop tasks for the variant matrix (issue #125): transitive callers, blast radius, "
    "call-chain flows, same-name symbols and tests that reach a function. Gold sets were computed by "
    "evals/agent/groundtruth/hard_tasks.py from call graphs built with compiler tooling (Go: go/packages + "
    "SSA via gotool edges, three GOOS; TypeScript: the type checker via ts_edges.js; Python: import-aware "
    "AST resolution via py_edges.py, cross-checked against grep), never from myceliums output. Each gold "
    "item lists one evidence line (a call site on the chain or the definition of an end point) that "
    "check-tasks re-verifies; its note gives the level or the next hop."
)

VERIFIED = {
    "fzf": ["gotool edges (go/packages + SSA, GOOS darwin/linux/windows)", "gotool refs (type-checked uses)"],
    "hono": ["ts_edges.js (TypeScript 5.9 type checker)", "grep"],
    "fastapi": ["py_edges.py (import-aware AST)", "py_refs.py", "grep"],
}

GO = ("Count only direct calls of named functions and methods: a call made inside a closure or function "
      "literal counts for the named function that contains it; calls through interface values or function "
      "variables do not count. Include platform-specific files (such as `*_windows.go`) and exclude "
      "`*_test.go` files.")
GO_TESTS = ("Count only direct calls of named functions and methods, whatever the arguments or branches: a call "
            "made inside a closure counts for the named function that contains it; calls through interface "
            "values or function variables do not count. Consider every package's test files, platform-specific "
            "ones included.")
TS = ("Count only direct calls (`new` included) that resolve to the declaration, following imports, re-exports "
      "and the static type of the receiver; a call inside a nested function, arrow function or callback counts "
      "for the outermost named function, method or `const`-bound function that contains it (for example a "
      "middleware factory such as `cors`). Calls through callback parameters, interface-typed values or function "
      "values do not count. Only non-test files under `src/` (exclude `*.test.ts` and `*.test.tsx`).")
TS_TESTS = ("Code anywhere in a test file counts, including inside `describe`/`it` callbacks and handlers the file "
            "defines. Count only direct calls (`new` included) that resolve to a declaration, following imports, "
            "re-exports and the static type of the receiver (so `c.req.parseBody()` calls `HonoRequest.parseBody`), "
            "whatever the arguments or branches; inside non-test code, a call in a nested function or callback "
            "counts for the outermost named function that contains it. Calls through callback parameters, "
            "interface-typed values or function values do not count, and sending a request to an app "
            "(`app.request(...)`, `fetch`) does not count as calling its handlers.")
PY = ("Consider only the `fastapi/` package. Count direct calls by name (imported names and re-exports included), "
      "calls through `self`, `cls` or `super()`, and class instantiation; a call inside a nested function counts "
      "for the outermost enclosing function or method. Calls on other objects' attributes or through function "
      "values do not count. Ignore docstrings and comments.")

# v2: the same rule, with examples of what does not count.
PY2 = ("Consider only the `fastapi/` package. Count direct calls by name (imported names and re-exports included), "
       "calls through `self`, `cls` or `super()`, and class instantiation by the class's name; a call inside a nested "
       "function counts for the outermost enclosing function or method. Calls on other objects' attributes (for "
       "example `route.handle(...)`, `self.router.add_api_route(...)`, `original_route.get_route_handler()`), "
       "instantiation through a variable (`route_class(...)`) and calls through function values do not count. "
       "Ignore docstrings and comments.")

LEVELS = ("Computed as all callers up to {depth} levels up the {tool} call graph; {{n}} functions. Each item's "
          "note gives its level and the function it calls on the way to the target.")
FILES = ("Computed from the {tool} call graph: the files holding a call to the target (level 1) or to one of "
         "its direct callers (level 2); {{n}} files. Each note names the calling function and its callee.")
FLOW = ("Computed as the functions reachable from the start and reaching the end in the {tool} call graph "
        "(every function on some chain, both ends included); {{n}} functions.")
GO_TOOL = "gotool edges (static edges, union of GOOS darwin, linux and windows)"
TS_TOOL = "ts_edges.js (type-checker resolved, `direct` edges)"
PY_TOOL = "py_edges.py (`direct` edges), every name grepped for missed callers"

SPECS_V1 = [
    {"id": "fzf-tc-matchitem", "repo": "fzf", "type": "transitive_callers", "kind": "symbols",
     "question": "Which functions or methods call `Pattern.MatchItem` (src/pattern.go) directly, call such a caller, "
                 "or call one of those - that is, every caller up to three levels up the call chain? " + GO +
                 " Do not list `MatchItem` itself.",
     "rationale": LEVELS.format(depth=3, tool=GO_TOOL) + " Cross-checked with type-checked uses (gotool refs) of "
                  "every level-1 and level-2 function: no use outside the set."},
    {"id": "fzf-blast-istty", "repo": "fzf", "type": "blast_radius", "kind": "files",
     "question": "Suppose the behaviour of `IsTty` (src/util/util.go) changes. Which Go files contain a call to "
                 "`IsTty`, or a call to a function or method that calls `IsTty` directly (two levels)? Name the "
                 "file that contains each such call. " + GO,
     "rationale": FILES.format(tool=GO_TOOL) + " src/winpty_windows.go and src/tui/light_unix.go come from "
                  "platform-specific builds."},
    {"id": "fzf-blast-stringbytes", "repo": "fzf", "type": "blast_radius", "kind": "files",
     "question": "Suppose the behaviour of `stringBytes` (src/functions.go) changes. Which Go files contain a call "
                 "to `stringBytes`, or a call to a function or method that calls `stringBytes` directly (two "
                 "levels)? Name the file that contains each such call. " + GO,
     "rationale": FILES.format(tool=GO_TOOL)},
    {"id": "fzf-amb-chars-length", "repo": "fzf", "type": "ambiguous", "kind": "symbols",
     "question": "Several types in this repository have a method named `Length`. Which functions or methods call "
                 "the `Length` method of the `Chars` type defined in src/util/chars.go (not `Merger.Length` or any "
                 "other `Length`)? " + GO,
     "rationale": "The direct callers of (*util.Chars).Length in the " + GO_TOOL + "; {n} functions. "
                  "`grep '.Length()'` finds 67 call sites outside tests, most of them on Merger. Cross-checked "
                  "with type-checked uses (gotool refs)."},
    {"id": "fzf-amb-noseparatorline", "repo": "fzf", "type": "ambiguous", "kind": "symbols",
     "question": "More than one function or method in src/ is named `noSeparatorLine`. Which functions or methods "
                 "call the `noSeparatorLine` method whose receiver is `*Terminal` (src/terminal.go)? " + GO,
     "rationale": "The direct callers of (*Terminal).noSeparatorLine in the " + GO_TOOL + "; {n} functions. "
                  "The plain function noSeparatorLine (src/options.go) and Options.noSeparatorLine have other "
                  "callers. Cross-checked with type-checked uses (gotool refs)."},
    {"id": "fzf-tests-awktokenizer", "repo": "fzf", "type": "tests", "kind": "symbols",
     "question": "Which `Test...` functions in the `*_test.go` files exercise `awkTokenizer` (src/tokenizer.go), "
                 "that is, reach it through a chain of direct calls of any length (directly, or through test "
                 "helpers and non-test functions)? " + GO_TESTS,
     "rationale": "The Test functions among all transitive callers in the " + GO_TOOL.replace("static edges",
                  "static edges with test packages") + "; {n} tests. Notes give the chain length (level) and the "
                  "first call on the chain."},
    {"id": "fzf-tests-splitnth", "repo": "fzf", "type": "tests", "kind": "symbols",
     "question": "Which `Test...` functions in the `*_test.go` files exercise `splitNth` (src/options.go), that is, "
                 "reach it through a chain of direct calls of any length (directly, or through test helpers and "
                 "non-test functions)? " + GO_TESTS,
     "rationale": "The Test functions among all transitive callers in the " + GO_TOOL.replace("static edges",
                  "static edges with test packages") + "; {n} tests. Notes give the chain length (level) and the "
                  "first call on the chain."},
    {"id": "fzf-flow-ansi", "repo": "fzf", "type": "flow", "kind": "symbols",
     "question": "List every function or method that lies on at least one chain of direct calls from `NewTerminal` "
                 "(src/terminal.go) to `matchControlSequence` (src/ansi.go), including both ends. " + GO,
     "rationale": FLOW.format(tool=GO_TOOL) + " Chains run through the prompt parser, the label printer and the "
                  "tmux/zellij border-label setters to extractColor."},
    {"id": "hono-tc-decodebase64", "repo": "hono", "type": "transitive_callers", "kind": "symbols",
     "question": "Which functions or methods call `decodeBase64` (src/utils/encode.ts) directly, or call one of its "
                 "direct callers (two levels up the call chain)? " + TS + " Do not list `decodeBase64` itself.",
     "rationale": LEVELS.format(depth=2, tool=TS_TOOL) + " `handle` is the AWS Lambda adapter's (the Lambda@Edge "
                  "`handle` is three levels up)."},
    {"id": "hono-blast-newresponse", "repo": "hono", "type": "blast_radius", "kind": "files",
     "question": "Suppose the public `newResponse` method of `Context` (src/context.ts; not the private "
                 "`#newResponse`) changes its signature. Which non-test source files under src/ contain a call to "
                 "it, or a call to a function or method that calls it directly (two levels)? Name the file that "
                 "contains each such call. " + TS,
     "rationale": FILES.format(tool=TS_TOOL) + " Context.redirect calls this.newResponse, so its callers count."},
    {"id": "hono-blast-httpexception", "repo": "hono", "type": "blast_radius", "kind": "files",
     "question": "Suppose the constructor of `HTTPException` (src/http-exception.ts) gains a required parameter. "
                 "Which non-test source files under src/ contain code that constructs an `HTTPException` "
                 "(`new HTTPException(...)`, inside a function or at module level), or a call to a function or "
                 "method that does (two levels)? Name the file that contains each such call. " + TS,
     "rationale": FILES.format(tool=TS_TOOL) + " src/http-exception.ts itself only mentions the constructor in a "
                  "doc comment; src/middleware/timeout/index.ts constructs one at module level."},
    {"id": "hono-amb-req-header", "repo": "hono", "type": "ambiguous", "kind": "symbols",
     "question": "Both `Context` (src/context.ts) and `HonoRequest` (src/request.ts) have a method named `header`. "
                 "Which functions or methods outside src/adapter/ call the `header` method of `HonoRequest` (for "
                 "example `c.req.header('X')`), as opposed to `Context.header`? " + TS,
     "rationale": "The direct callers of HonoRequest.header in the " + TS_TOOL + " outside src/adapter/; {n} "
                  "functions, confirmed with grep for `.header(`. compress calls both (ctx.header and "
                  "ctx.req.header)."},
    {"id": "hono-tests-buffertoformdata", "repo": "hono", "type": "tests", "kind": "files",
     "question": "Which test files under src/ (`*.test.ts`, `*.test.tsx`) exercise `bufferToFormData` "
                 "(src/utils/buffer.ts) through a chain of direct calls: code in the test file calls it, or calls "
                 "a non-test function from which it is reachable through direct calls? " + TS_TESTS +
                 " Answer with file paths.",
     "rationale": "The test files among all transitive callers in the " + TS_TOOL.replace("`direct` edges",
                  "`direct` edges, test files included") + "; {n} files. Chains run through validator, "
                  "HonoRequest.parseBody, parseBody, parseFormData and methodOverride."},
    {"id": "hono-tests-serialize", "repo": "hono", "type": "tests", "kind": "files",
     "question": "Which test files under src/ (`*.test.ts`, `*.test.tsx`) exercise `_serialize` (src/utils/cookie.ts) "
                 "through a chain of direct calls: code in the test file calls it, or calls a non-test function "
                 "from which it is reachable through direct calls? " + TS_TESTS + " Answer with file paths.",
     "rationale": "The test files among all transitive callers in the " + TS_TOOL.replace("`direct` edges",
                  "`direct` edges, test files included") + "; {n} files. Chains run through serialize / "
                  "serializeSigned, the cookie helpers (setCookie, setSignedCookie, deleteCookie, generateCookie), "
                  "the RPC client (hc -> ClientRequestImpl.fetch serialises cookies), testClient and the language "
                  "middleware's cookie cache."},
    {"id": "hono-flow-validator", "repo": "hono", "type": "flow", "kind": "symbols",
     "question": "List every function or method that lies on at least one chain of direct calls from `validator` "
                 "(src/validator/validator.ts) to `tryDecodeURIComponent` (src/utils/url.ts), including both ends. "
                 + TS,
     "rationale": FLOW.format(tool=TS_TOOL) + " Three branches: query parsing (HonoRequest.queries), path params "
                  "(HonoRequest.param and its private decoders) and cookies (getCookie -> parse)."},
    {"id": "fastapi-tc-oauth-scopes", "repo": "fastapi", "type": "transitive_callers", "kind": "symbols",
     "question": "Which functions or methods in the `fastapi/` package call `_get_oauth_scopes` "
                 "(fastapi/dependencies/models.py) directly, call such a caller, or call one of those (every caller "
                 "up to three levels up)? " + PY + " Do not list `_get_oauth_scopes` itself.",
     "rationale": LEVELS.format(depth=3, tool=PY_TOOL)},
    {"id": "fastapi-tc-annotation-sequence", "repo": "fastapi", "type": "transitive_callers", "kind": "symbols",
     "question": "Which functions or methods in the `fastapi/` package call `field_annotation_is_sequence` "
                 "(fastapi/_compat/shared.py) directly, call such a caller, or call one of those (every caller up to "
                 "three levels up)? " + PY + " Do not list `field_annotation_is_sequence` itself.",
     "rationale": LEVELS.format(depth=3, tool=PY_TOOL) + " Callers import it through the fastapi._compat "
                  "re-export."},
    {"id": "fastapi-flow-openapi-path", "repo": "fastapi", "type": "flow", "kind": "symbols",
     "question": "List every function or method in the `fastapi/` package that lies on at least one chain of direct "
                 "calls from `get_openapi_path` (fastapi/openapi/utils.py) to `lenient_issubclass` "
                 "(fastapi/_compat/shared.py), including both ends. " + PY,
     "rationale": FLOW.format(tool=PY_TOOL) + " get_openapi_path also calls lenient_issubclass directly."},
    {"id": "fastapi-flow-openapi-cache-key", "repo": "fastapi", "type": "flow", "kind": "symbols",
     "question": "List every function or method in the `fastapi/` package that lies on at least one chain of direct "
                 "calls from the `openapi` method of `FastAPI` (fastapi/applications.py) to `_get_cache_key` "
                 "(fastapi/dependencies/models.py), including both ends. " + PY,
     "rationale": FLOW.format(tool=PY_TOOL) + " Two branches below get_openapi: get_fields_from_routes -> "
                  "get_flat_params and get_openapi_path -> _get_openapi_dependency_data."},
    {"id": "fastapi-amb-httpexception", "repo": "fastapi", "type": "ambiguous", "kind": "symbols",
     "question": "FastAPI defines its own `HTTPException` class in fastapi/exceptions.py, a subclass of Starlette's "
                 "class of the same name, and modules of the package import one or the other. Which functions or "
                 "methods in the `fastapi/` package construct an instance of FastAPI's `HTTPException` "
                 "(fastapi/exceptions.py) - not Starlette's - either directly or by calling a method that "
                 "constructs one? A call through `self` runs the implementation that the instance's class uses "
                 "(inherited unless overridden). Ignore docstrings and comments.",
     "rationale": "Only fastapi/security/http.py and fastapi/security/oauth2.py import fastapi.exceptions."
                  "HTTPException; routing.py, api_key.py, open_id_connect_url.py and telemetry/_api.py construct "
                  "Starlette's. The two make_not_authenticated_error methods construct it (level 1); every __call__ "
                  "that runs one of them through self, inherited and not overridden, is level 2; {n} methods. "
                  "Derived with py_edges.py and checked by reading every `HTTPException(` and "
                  "`make_not_authenticated_error(` site in fastapi/."},
]



def _callers_q(what, depth, clause, itself):
    levels = {2: "or call one of its direct callers (two levels up the call chain)",
              3: "call such a caller, or call one of those (every caller up to three levels up the call chain)",
              4: "or call a function that does, and so on up to four levels up the call chain",
              5: "or call a function that does, and so on up to five levels up the call chain"}[depth]
    return f"Which functions or methods call {what} directly, {levels}? {clause} Do not list `{itself}` itself."


def _files_q(what, depth, clause, extra=""):
    chain = {2: "or a call to a function or method that calls it directly (two levels)",
             3: "or a call to a function or method on a chain of at most three calls ending in it (three levels)",
             4: "or a call to a function or method on a chain of at most four calls ending in it (four levels)"}[depth]
    return (f"Suppose {what}. Which {extra}files contain a call to it, {chain}? Name the file that contains each "
            f"such call. {clause}")


SPECS_V2 = [
    {"id": "fzf-tc-tochars-d4", "repo": "fzf", "type": "transitive_callers", "kind": "symbols",
     "question": _callers_q("`ToChars` (src/util/chars.go)", 4, GO, "ToChars"),
     "rationale": LEVELS.format(depth=4, tool=GO_TOOL)},
    {"id": "fzf-blast-istty-d3", "repo": "fzf", "type": "blast_radius", "kind": "files",
     "question": _files_q("the behaviour of `IsTty` (src/util/util.go) changes", 3, GO, "Go "),
     "rationale": FILES.format(tool=GO_TOOL).replace("(level 1) or to one of its direct callers (level 2)",
                                                     "or to a function up to two levels above it")},
    {"id": "fzf-blast-mayfold-d4", "repo": "fzf", "type": "blast_radius", "kind": "files",
     "question": _files_q("the behaviour of the function `MayFoldToAscii` in package util (src/util/chars.go; not "
                          "the method `Chars.MayFoldToAscii`) changes", 4, GO, "Go "),
     "rationale": FILES.format(tool=GO_TOOL).replace("(level 1) or to one of its direct callers (level 2)",
                                                     "or to a function up to three levels above it")},
    {"id": "fzf-amb-chars-length-d3", "repo": "fzf", "type": "ambiguous", "kind": "symbols",
     "question": "Several types in this repository have a method named `Length`. Which functions or methods call "
                 "the `Length` method of the `Chars` type defined in src/util/chars.go (not `Merger.Length` or any "
                 "other `Length`) directly, call such a caller, or call one of those (every caller up to three "
                 "levels up the call chain)? " + GO + " Do not list `Chars.Length` itself.",
     "rationale": LEVELS.format(depth=3, tool=GO_TOOL) + " `grep '.Length()'` finds 67 call sites outside tests, "
                  "most of them on Merger."},
    {"id": "fzf-amb-merger-get-d3", "repo": "fzf", "type": "ambiguous", "kind": "symbols",
     "question": "Several types in this repository have a method named `Get` (`Merger`, `Chars`, `AtomicBool`). Which "
                 "functions or methods call the `Get` method of `Merger` (src/merger.go) directly, call such a "
                 "caller, or call one of those (every caller up to three levels up the call chain)? " + GO +
                 " Do not list `Merger.Get` itself.",
     "rationale": LEVELS.format(depth=3, tool=GO_TOOL)},
    {"id": "fzf-tests-stringbytes", "repo": "fzf", "type": "tests", "kind": "symbols",
     "question": "Which `Test...` functions in the `*_test.go` files exercise `stringBytes` (src/functions.go), that "
                 "is, reach it through a chain of direct calls of any length (directly, or through test helpers and "
                 "non-test functions)? " + GO_TESTS,
     "rationale": "The Test functions among all transitive callers in the " + GO_TOOL.replace("static edges",
                  "static edges with test packages") + "; {n} tests, none of which calls it directly. Notes give "
                  "the chain length (level) and the first call on the chain."},
    {"id": "fzf-tests-chars-tostring", "repo": "fzf", "type": "tests", "kind": "symbols",
     "question": "Which `Test...` functions in the `*_test.go` files exercise the `ToString` method of `Chars` "
                 "(src/util/chars.go), that is, reach it through a chain of direct calls of any length (directly, or "
                 "through test helpers and non-test functions)? " + GO_TESTS,
     "rationale": "The Test functions among all transitive callers in the " + GO_TOOL.replace("static edges",
                  "static edges with test packages") + "; {n} tests. Notes give the chain length (level) and the "
                  "first call on the chain."},
    {"id": "fzf-flow-newterminal-stringbytes", "repo": "fzf", "type": "flow", "kind": "symbols",
     "question": "List every function or method that lies on at least one chain of direct calls from `NewTerminal` "
                 "(src/terminal.go) to `stringBytes` (src/functions.go), including both ends. " + GO,
     "rationale": FLOW.format(tool=GO_TOOL) + " Branches run through the prompt and label parsers (extractColor, "
                  "ANSI sequence parsing), the tmux/zellij border-label setters and printHighlighted -> MatchItem "
                  "-> transformInput -> Tokenize/Transform."},
    {"id": "hono-tc-trydecode-d3", "repo": "hono", "type": "transitive_callers", "kind": "symbols",
     "question": _callers_q("`tryDecode` (src/utils/url.ts)", 3, TS, "tryDecode"),
     "rationale": LEVELS.format(depth=3, tool=TS_TOOL) + " Same-named functions in different files (the adapters' "
                  "`serveStatic`) are one answer item."},
    {"id": "hono-blast-createresponse-d3", "repo": "hono", "type": "blast_radius", "kind": "files",
     "question": _files_q("the function `createResponseInstance` (src/context.ts) changes its signature", 3, TS,
                          "non-test source "),
     "rationale": FILES.format(tool=TS_TOOL).replace("(level 1) or to one of its direct callers (level 2)",
                                                    "or to a function up to two levels above it")},
    {"id": "hono-blast-httpexception-d3", "repo": "hono", "type": "blast_radius", "kind": "files",
     "question": _files_q("the constructor of `HTTPException` (src/http-exception.ts) gains a required parameter "
                          "(code that constructs one, `new HTTPException(...)`, inside a function or at module level, "
                          "counts as a call)", 3, TS, "non-test source "),
     "rationale": FILES.format(tool=TS_TOOL).replace("(level 1) or to one of its direct callers (level 2)",
                                                    "or to a function up to two levels above it")
                  + " src/http-exception.ts itself only mentions the constructor in a doc comment."},
    {"id": "hono-amb-ctx-header-d2", "repo": "hono", "type": "ambiguous", "kind": "symbols",
     "question": "Both `Context` (src/context.ts) and `HonoRequest` (src/request.ts) have a method named `header`. "
                 "Which functions or methods call the `header` method of `Context` (for example `c.header('X', 'v')`, "
                 "as opposed to `c.req.header(...)`) directly, or call one of its direct callers (two levels up the "
                 "call chain)? " + TS + " Do not list `Context.header` itself.",
     "rationale": LEVELS.format(depth=2, tool=TS_TOOL) + " Same-named functions in different files (the adapters' "
                  "`serveStatic`) are one answer item."},
    {"id": "hono-tests-req-header", "repo": "hono", "type": "tests", "kind": "files",
     "question": "Which test files under src/ (`*.test.ts`, `*.test.tsx`) exercise the `header` method of `HonoRequest` "
                 "(src/request.ts; not `Context.header`) through a chain of direct calls: code in the test file calls "
                 "it, or calls a non-test function from which it is reachable through direct calls? " + TS_TESTS +
                 " Answer with file paths.",
     "rationale": "The test files among all transitive callers in the " + TS_TOOL.replace("`direct` edges",
                  "`direct` edges, test files included") + "; {n} files."},
    {"id": "hono-tests-trydecode", "repo": "hono", "type": "tests", "kind": "files",
     "question": "Which test files under src/ (`*.test.ts`, `*.test.tsx`) exercise `tryDecode` (src/utils/url.ts) "
                 "through a chain of direct calls: code in the test file calls it, or calls a non-test function from "
                 "which it is reachable through direct calls? " + TS_TESTS + " Answer with file paths.",
     "rationale": "The test files among all transitive callers in the " + TS_TOOL.replace("`direct` edges",
                  "`direct` edges, test files included") + "; {n} files, none of which calls it directly."},
    {"id": "fastapi-tc-oauth-scopes-d5", "repo": "fastapi", "type": "transitive_callers", "kind": "symbols",
     "question": _callers_q("`_get_oauth_scopes` (fastapi/dependencies/models.py)", 5, PY2, "_get_oauth_scopes"),
     "rationale": LEVELS.format(depth=5, tool=PY_TOOL)},
    {"id": "fastapi-tc-annotation-sequence-d5", "repo": "fastapi", "type": "transitive_callers", "kind": "symbols",
     "question": _callers_q("`field_annotation_is_sequence` (fastapi/_compat/shared.py)", 5, PY2,
                            "field_annotation_is_sequence"),
     "rationale": LEVELS.format(depth=5, tool=PY_TOOL) + " Callers import it through the fastapi._compat re-export."},
    {"id": "fastapi-flow-openapi-lenient", "repo": "fastapi", "type": "flow", "kind": "symbols",
     "question": "List every function or method in the `fastapi/` package that lies on at least one chain of direct "
                 "calls from the `openapi` method of `FastAPI` (fastapi/applications.py) to `lenient_issubclass` "
                 "(fastapi/_compat/shared.py), including both ends. " + PY2,
     "rationale": FLOW.format(tool=PY_TOOL)},
    {"id": "fastapi-flow-route-state", "repo": "fastapi", "type": "flow", "kind": "symbols",
     "question": "List every function or method in the `fastapi/` package that lies on at least one chain of direct "
                 "calls from `_populate_api_route_state` (fastapi/routing.py) to `_annotation_is_sequence` "
                 "(fastapi/_compat/shared.py), including both ends. " + PY2,
     "rationale": FLOW.format(tool=PY_TOOL)},
    {"id": "fastapi-flow-websocket-app", "repo": "fastapi", "type": "flow", "kind": "symbols",
     "question": "List every function or method in the `fastapi/` package that lies on at least one chain of direct "
                 "calls from `get_websocket_app` (fastapi/routing.py) to `field_annotation_is_sequence` "
                 "(fastapi/_compat/shared.py), including both ends. " + PY2,
     "rationale": FLOW.format(tool=PY_TOOL)},
    {"id": "fastapi-amb-starlette-httpexception", "repo": "fastapi", "type": "ambiguous", "kind": "symbols",
     "question": "Starlette's `HTTPException` (starlette.exceptions) and FastAPI's subclass of the same name "
                 "(fastapi/exceptions.py) are both imported by modules of the package. Which functions or methods in "
                 "the `fastapi/` package call Starlette's `HTTPException` class itself (imported from "
                 "starlette.exceptions; calls of FastAPI's subclass do not count), directly or through at most two "
                 "intermediate calls (three levels)? " + PY2,
     "rationale": "Level 1: every `HTTPException(...)` call in a module that imports the name from "
                  "starlette.exceptions (fastapi/routing.py, fastapi/security/api_key.py, "
                  "fastapi/security/open_id_connect_url.py); fastapi/security/http.py and oauth2.py construct "
                  "FastAPI's subclass. Levels 2-3 follow " + PY_TOOL + "; {n} functions."},
]


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "v2")
