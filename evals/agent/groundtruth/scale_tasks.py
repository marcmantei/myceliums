"""Gold sets for the outcome eval at scale (#163): tasks over Django 5.1.4 and
Vue 3.5.46 (vuejs/core), six question types, never derived from myceliums.

    # 1. call edges (Python's ast module; the TypeScript type checker)
    python3 -I scale_tasks.py edges <repos> <edges>        # needs node and TS_PATH (typescript 6.0)
    # 2. the task file
    python3 -I scale_tasks.py build <edges> <repos> > ../tasks/tasks-scale.json
    python3 -I scale_tasks.py build <edges> <repos> --calibration > ../tasks/tasks-scale-candidates.json

``edges`` writes

* ``<edges>/django/edges.tsv``: ``py_edges.py --local-imports --star-imports``
  over ``django/`` (the call graph of #153);
* ``<edges>/django/edges-tests.tsv``: the same over ``django/`` and every test
  app under ``tests/`` (each app is importable top level, as Django's
  ``runtests.py`` arranges; call sites keep their ``tests/...`` paths);
* ``<edges>/vue/edges.tsv``: ``vue_edges.js`` over ``packages/*/src`` and
  ``packages/*/__tests__``;
* ``<edges>/vue/app-config.tsv``: ``vue_config.js`` (accesses of ``AppConfig``
  members, typed by the checker).

Question types, per repository (``CANDIDATES`` lists every candidate; the
calibration picked the frozen set, see evals/agent/README.md):

* ``transitive_callers``: every caller up to two or three levels up;
* ``blast_radius``: the files holding a call to X or to a direct caller of X;
* ``ambiguous``: callers of one of several same-named functions;
* ``flow``: every function on at least one call chain from A to B;
* ``tests``: the test files that reach X through direct calls;
* ``config``: questions that are not about calls: which checks run only with
  ``check --deploy``, which template filters are marked safe, which model
  fields disallow empty strings (Django); which files read ``app.config``
  options, which keys are always set as HTML attributes, which component
  options are merged into arrays (Vue).

Each gold item carries one evidence line that ``check-tasks`` re-verifies and a
note (the level and next hop for call-graph items).
"""

from __future__ import annotations

import ast
import collections
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from hard_tasks import Graph  # noqa: E402
import outcome_tasks as ot  # noqa: E402

# --------------------------------------------------------------------------
# Edges
# --------------------------------------------------------------------------


def make_edges(repos: str, edges: str) -> None:
    dj = os.path.join(repos, "django")
    os.makedirs(os.path.join(edges, "django"), exist_ok=True)
    os.makedirs(os.path.join(edges, "vue"), exist_ok=True)
    py_edges = [sys.executable, "-I", os.path.join(HERE, "py_edges.py"), "--local-imports", "--star-imports"]
    with open(os.path.join(edges, "django", "edges.tsv"), "w") as fh:
        subprocess.run(py_edges + [dj, "django"], stdout=fh, check=True)
    # Test apps import each other top level (`from forms_tests.models import ...`):
    # a root with `django` and every test app side by side resolves that.
    root = tempfile.mkdtemp(prefix="scale-djroot-")
    try:
        os.symlink(os.path.join(dj, "django"), os.path.join(root, "django"))
        apps = sorted(d for d in os.listdir(os.path.join(dj, "tests"))
                      if os.path.isdir(os.path.join(dj, "tests", d)))
        for a in apps:
            os.symlink(os.path.join(dj, "tests", a), os.path.join(root, a))
        res = subprocess.run(py_edges + [root, "django", *apps], capture_output=True, text=True, check=True)
    finally:
        shutil.rmtree(root)
    with open(os.path.join(edges, "django", "edges-tests.tsv"), "w") as fh:
        for line in res.stdout.splitlines():
            a, b, site, kind = line.split("\t")
            if not site.startswith("django/"):
                site = "tests/" + site
            fh.write("\t".join((a, b, site, kind)) + "\n")
    vue = os.path.join(repos, "vue")
    env = dict(os.environ)
    for script, args, out in (("vue_edges.js", [], "edges.tsv"), ("vue_config.js", [], "app-config.tsv"),
                              ("vue_hierarchy.js", ["packages/compiler-core/src/ast.ts", "Node"], "ast-node.tsv")):
        with open(os.path.join(edges, "vue", out), "w") as fh:
            subprocess.run(["node", os.path.join(HERE, script), vue, *args], stdout=fh, check=True, env=env)


# --------------------------------------------------------------------------
# Shared helpers
# --------------------------------------------------------------------------


def lines_of(repos: str, repo: str, file: str) -> list[str]:
    with open(os.path.join(repos, repo, file), encoding="utf-8", errors="replace") as fh:
        return fh.read().splitlines()


def find_line(repos: str, repo: str, file: str, want: str, start: int = 1) -> dict:
    """The first line at or after ``start`` that contains ``want``."""
    ls = lines_of(repos, repo, file)
    for i in range(max(1, start), len(ls) + 1):
        if want in ls[i - 1]:
            return {"file": file, "line": i, "contains": want}
    raise SystemExit(f"{file}: no line containing {want!r} from line {start}")


def with_aliases(item: dict) -> dict:
    """A constructor may be answered by its class's name."""
    n = item["name"]
    for suffix in (".constructor", ".__init__"):
        if n.endswith(suffix) and n.count(".") >= 1:
            item["aliases"] = [n[: -len(suffix)]]
    return item


# --------------------------------------------------------------------------
# Django: call graphs
# --------------------------------------------------------------------------

DJ = "django"


def dj_graph(edges: str, tests: bool = False) -> Graph:
    g = Graph()
    path = os.path.join(edges, DJ, "edges-tests.tsv" if tests else "edges.tsv")
    for line in open(path):
        a, b, site, kind = line.rstrip("\n").split("\t")
        if kind != "direct":
            continue
        if not tests and not site.startswith("django/"):
            continue
        g.add(a, b, site)
    return g


def dj_file_of(key: str, repos: str) -> str:
    mod = key.split(":", 1)[0].replace(".", "/")
    for cand in (mod + ".py", mod + "/__init__.py"):
        if os.path.exists(os.path.join(repos, DJ, cand)):
            return cand
    raise SystemExit(f"no file for {key}")


def dj_def_evidence(repos: str, key: str) -> dict:
    file = dj_file_of(key, repos)
    qual = key.split(":", 1)[1].split(".")
    ls = lines_of(repos, DJ, file)
    start = 1
    if len(qual) > 1:
        start = next(i for i, t in enumerate(ls, 1) if t.lstrip().startswith(f"class {qual[0]}"))
    want = f"def {qual[-1]}("
    for i in range(start, len(ls) + 1):
        if want in ls[i - 1]:
            return {"file": file, "line": i, "contains": want}
    raise SystemExit(f"no definition line for {key}")


def dj_symbols(repos, g, target, depth):
    return [with_aliases(x) for x in ot.gold_symbols(repos, g, target, depth)]


def dj_files(repos, g, target, depth):
    return ot.gold_files(repos, g, target, depth)


def dj_flow(repos, g, a, b):
    between = g.reach(a, True) & g.reach(b, False)
    gold, seen = [], set()
    for k in sorted(between, key=lambda x: (ot.name_of(x), x)):
        name = ot.name_of(k)
        if name in seen:  # two functions of one name are one answer item
            continue
        seen.add(name)
        nxt = sorted(x for x in g.out[k] if x in between)
        if nxt:
            ev, note = ot.evidence(repos, g, k, nxt[0]), f"calls {ot.name_of(nxt[0])} ({k.split(':')[0]})"
        else:
            ev, note = dj_def_evidence(repos, k), f"the end of the chain ({k.split(':')[0]})"
        gold.append(with_aliases({"name": name, "evidence": [ev], "note": note}))
    return gold


def dj_test_files(repos, gt, target, depth, optional=()):
    """Test files (under tests/) holding a call to ``target`` or to a function
    or method that calls it directly (``depth`` 2)."""
    c = gt.callers_upto(target, depth)
    files: dict[str, dict] = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0])):
        for hop in sorted(x for x in gt.out[k] if x == target or c.get(x) == d - 1):
            for site in sorted(gt.sites[(k, hop)]):
                f = site.rsplit(":", 1)[0]
                if not f.startswith("tests/") or f in files:
                    continue
                files[f] = {"name": f, "evidence": [ot.evidence(repos, gt, k, hop)],
                            "note": f"level {d}: {ot.name_of(k)} -> {ot.name_of(hop)}", "_level": d}
    gold = [{k: v for k, v in x.items() if k != "_level"}
            for x in sorted(files.values(), key=lambda x: (x["_level"], x["name"]))]
    return [x for x in gold if x["name"] not in optional]


# --------------------------------------------------------------------------
# Django: questions that are not about calls
# --------------------------------------------------------------------------


def _py_files(repos: str, sub: str):
    root = os.path.join(repos, DJ)
    for dp, dn, fn in os.walk(os.path.join(root, sub)):
        dn.sort()
        for f in sorted(fn):
            if f.endswith(".py"):
                p = os.path.join(dp, f)
                yield os.path.relpath(p, root), ast.parse(open(p, encoding="utf-8").read())


def dj_deploy_checks(repos: str) -> list[dict]:
    """Check functions registered with ``deploy=True`` (``@register(..., deploy=True)``)."""
    gold = []
    for rel, tree in _py_files(repos, "django/core/checks"):
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for dec in node.decorator_list:
                if (isinstance(dec, ast.Call) and getattr(dec.func, "id", getattr(dec.func, "attr", "")) == "register"
                        and any(k.arg == "deploy" and getattr(k.value, "value", None) is True for k in dec.keywords)):
                    gold.append({"name": node.name,
                                 "evidence": [find_line(repos, DJ, rel, f"def {node.name}(", node.lineno)],
                                 "note": f"registered with deploy=True at {rel}:{dec.lineno}"})
    return sorted(gold, key=lambda x: x["name"])


def dj_safe_filters(repos: str) -> list[dict]:
    """Functions in defaultfilters.py registered as filters with ``is_safe=True``."""
    rel = "django/template/defaultfilters.py"
    tree = ast.parse(open(os.path.join(repos, DJ, rel), encoding="utf-8").read())
    gold = []
    for node in tree.body:
        if not isinstance(node, ast.FunctionDef):
            continue
        for dec in node.decorator_list:
            if (isinstance(dec, ast.Call) and isinstance(dec.func, ast.Attribute) and dec.func.attr == "filter"
                    and any(k.arg == "is_safe" and getattr(k.value, "value", None) is True for k in dec.keywords)):
                name_arg = dec.args[0].value if dec.args and isinstance(dec.args[0], ast.Constant) else None
                item = {"name": node.name, "evidence": [find_line(repos, DJ, rel, f"def {node.name}(", node.lineno)],
                        "note": f"@register.filter({'%r, ' % name_arg if name_arg else ''}is_safe=True) "
                                f"at line {dec.lineno}"}
                if name_arg and name_arg != node.name:
                    item["aliases"] = [name_arg]  # the filter's template name
                gold.append(item)
    return gold


def dj_empty_strings(repos: str) -> tuple[list[dict], list[dict]]:
    """Model field classes in django/db/models/fields/ whose effective
    ``empty_strings_allowed`` is False (set by the class or inherited)."""
    classes: dict[str, tuple[str, ast.ClassDef]] = {}
    for rel, tree in _py_files(repos, "django/db/models/fields"):
        for node in tree.body:
            if isinstance(node, ast.ClassDef):
                classes.setdefault(node.name, (rel, node))  # names are unique in the package

    def own_value(cls: ast.ClassDef):
        for st in cls.body:
            if (isinstance(st, ast.Assign) and any(getattr(t, "id", None) == "empty_strings_allowed"
                                                   for t in st.targets)):
                return st.value.value, st.lineno
        return None

    def bases(cls: ast.ClassDef) -> list[str]:
        out = []
        for b in cls.bases:
            n = b.id if isinstance(b, ast.Name) else b.attr if isinstance(b, ast.Attribute) else None
            if n:
                out.append(n)
        return out

    def mro(name: str, seen=None) -> list[str]:
        # Depth-first, left to right: the package has no diamond that changes
        # the answer (checked: every class's linearisation reaches one value).
        seen = seen or set()
        if name in seen or name not in classes:
            return []
        seen.add(name)
        out = [name]
        for b in bases(classes[name][1]):
            out += [x for x in mro(b, seen) if x not in out]
        return out

    def is_field(name: str) -> bool:
        return "Field" in mro(name)

    gold, optional = [], []
    for name, (rel, cls) in sorted(classes.items()):
        if not is_field(name):
            continue
        chain = mro(name)
        value, origin = None, None
        for c in chain:
            v = own_value(classes[c][1])
            if v is not None:
                value, origin = v[0], c
                break
        if value is False:
            ev = find_line(repos, DJ, rel, f"class {name}(", cls.lineno)
            gold.append({"name": name, "evidence": [ev],
                         "note": "sets empty_strings_allowed = False" if origin == name
                         else f"inherits empty_strings_allowed = False from {origin}"})
    return gold, optional


def dj_subclasses(repos: str, base: str) -> list[dict]:
    """Classes in django/ whose bases reach ``base`` by name (transitively)."""
    classes = collections.defaultdict(list)
    for rel, tree in _py_files(repos, "django"):
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                bs = [b.id if isinstance(b, ast.Name) else b.attr if isinstance(b, ast.Attribute) else None
                      for b in node.bases]
                classes[node.name].append((rel, node, bs))
    sub, frontier = {}, {base}
    while frontier:
        nxt = set()
        for name, defs in sorted(classes.items()):
            for rel, node, bs in defs:
                hit = sorted(set(bs) & frontier)
                if hit and name not in sub and name != base:
                    sub[name] = (rel, node, hit[0])
                    nxt.add(name)
        frontier = nxt
    return [{"name": n, "evidence": [find_line(repos, DJ, rel, f"class {n}(", node.lineno)],
             "note": f"subclass of {via} ({rel})"} for n, (rel, node, via) in sorted(sub.items())]


def dj_optional_settings(repos: str) -> tuple[list[dict], list[dict]]:
    gs = ast.parse(open(os.path.join(repos, DJ, "django/conf/global_settings.py"), encoding="utf-8").read())
    defined = {t.id for n in gs.body if isinstance(n, ast.Assign) for t in n.targets if isinstance(t, ast.Name)}
    found: dict[str, tuple[str, int, str]] = {}
    for rel, tree in _py_files(repos, "django"):
        if rel.startswith("django/conf/"):
            continue
        for n in ast.walk(tree):
            name = None
            if (isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name) and n.value.id == "settings"
                    and n.attr.isupper()):
                name, how = n.attr, f"settings.{n.attr}"
            elif (isinstance(n, ast.Call) and getattr(n.func, "id", None) in ("getattr", "hasattr")
                  and len(n.args) >= 2 and isinstance(n.args[0], ast.Name) and n.args[0].id == "settings"
                  and isinstance(n.args[1], ast.Constant) and isinstance(n.args[1].value, str)):
                name, how = n.args[1].value, f"{n.func.id}(settings, ...)"
            if name and name not in defined:
                prev = found.get(name)
                if prev is None or (rel, n.lineno) < (prev[0], prev[1]):
                    found[name] = (rel, n.lineno, how)
    gold, optional = [], []
    for name, (rel, ln, how) in sorted(found.items()):
        item = {"name": name, "evidence": [find_line(repos, DJ, rel, name, ln)],
                "note": f"read as {how} at {rel}:{ln}; not assigned in global_settings.py"}
        (optional if name == "SETTINGS_MODULE" else gold).append(item)
    return gold, optional


def dj_suspicious(repos: str) -> list[dict]:
    """Classes in django/ that subclass SuspiciousOperation, directly or not."""
    classes = {}
    for rel, tree in _py_files(repos, "django"):
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                classes.setdefault(node.name, []).append((rel, node))
    sub = {}
    frontier = {"SuspiciousOperation"}
    while frontier:
        nxt = set()
        for name, defs in classes.items():
            for rel, node in defs:
                bs = {b.id if isinstance(b, ast.Name) else getattr(b, "attr", None) for b in node.bases}
                hit = bs & frontier
                if hit and name not in sub and name != "SuspiciousOperation":
                    sub[name] = (rel, node, sorted(hit)[0])
                    nxt.add(name)
        frontier = nxt
    return [{"name": n, "evidence": [find_line(repos, DJ, rel, f"class {n}(", node.lineno)],
             "note": f"subclass of {base}"} for n, (rel, node, base) in sorted(sub.items())]


# --------------------------------------------------------------------------
# Vue: call graphs
# --------------------------------------------------------------------------

VUE = "vue"


def is_test(f: str) -> bool:
    return "/__tests__/" in f


def vue_graph(edges: str, tests: bool) -> Graph:
    g = Graph()
    for line in open(os.path.join(edges, VUE, "edges.tsv")):
        a, b, site, kind, bfile = line.rstrip("\n").split("\t")
        afile = site.rsplit(":", 1)[0]
        if kind != "direct" or (not tests and is_test(afile)):
            continue
        g.add((afile, a), (bfile, b), site)
    return g


def ts_short(key) -> str:
    n = key[1].split(".")
    return n[0] if n[-1] == "constructor" else n[-1]


def ts_evidence(repos, g, a, b) -> dict:
    want = ts_short(b)
    sites = sorted(g.sites[(a, b)], key=lambda s: (s.rsplit(":", 1)[0], int(s.rsplit(":", 1)[1])))
    for site in sites:
        file, ln = site.rsplit(":", 1)
        ls = lines_of(repos, VUE, file)
        for k in range(int(ln), min(int(ln) + 4, len(ls) + 1)):
            if want in ls[k - 1]:
                return {"file": file, "line": k, "contains": want}
    file, ln = sites[0].rsplit(":", 1)
    return {"file": file, "line": int(ln), "contains": lines_of(repos, VUE, file)[int(ln) - 1].strip()[:60]}


def ts_def_evidence(repos, key) -> dict:
    file, name = key
    n = name.split(".")[-1]
    ls = lines_of(repos, VUE, file)
    import re
    for pat in (rf"\b(function|const|let)\s+{re.escape(n)}\b", rf"^\s*(async\s+)?(get\s+|set\s+)?{re.escape(n)}\s*[(<]"):
        for i, t in enumerate(ls, 1):
            if re.search(pat, t):
                return {"file": file, "line": i, "contains": n}
    raise SystemExit(f"no definition line for {key}")


def vue_symbols(repos, g, target, depth):
    c = {k: d for k, d in g.callers_upto(target, depth).items() if k[1] != "<module>"}
    gold, seen = [], set()
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0][1], x[0][0])):
        if k[1] in seen:
            continue
        seen.add(k[1])
        hop = sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1)[0]
        gold.append(with_aliases({"name": k[1], "evidence": [ts_evidence(repos, g, k, hop)],
                                  "note": f"level {d}, via {hop[1]} ({k[0]})"}))
    return gold


def self_call_files(edges: str, repo: str, target) -> dict[str, int]:
    """Files holding a recursive call of ``target`` inside itself (the call
    graph drops self-edges): file -> first line."""
    out: dict[str, int] = {}
    if repo == VUE:
        for line in open(os.path.join(edges, VUE, "edges.tsv")):
            a, b, site, kind, bfile = line.rstrip("\n").split("\t")
            f, ln = site.rsplit(":", 1)
            if kind == "direct" and (f, a) == target and (bfile, b) == target:
                out.setdefault(f, int(ln))
    else:
        for line in open(os.path.join(edges, DJ, "edges.tsv")):
            a, b, site, kind = line.rstrip("\n").split("\t")
            f, ln = site.rsplit(":", 1)
            if kind == "direct" and a == b == target:
                out.setdefault(f, int(ln))
    return out


def vue_files(repos, g, target, depth):
    c = g.callers_upto(target, depth)
    files = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0])):
        for hop in sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1):
            f = k[0]
            if f in files:
                continue
            files[f] = {"name": f, "evidence": [ts_evidence(repos, g, k, hop)],
                        "note": f"level {d}: {k[1]} -> {hop[1]}", "_level": d}
    return [{k: v for k, v in x.items() if k != "_level"}
            for x in sorted(files.values(), key=lambda x: (x["_level"], x["name"]))]


def vue_flow(repos, g, a, b):
    between = g.reach(a, True) & g.reach(b, False)
    gold, seen = [], set()
    for k in sorted(between, key=lambda x: (x[1], x[0])):
        if k[1] in seen:
            continue
        seen.add(k[1])
        nxt = sorted(x for x in g.out[k] if x in between)
        if nxt:
            ev, note = ts_evidence(repos, g, k, nxt[0]), f"calls {nxt[0][1]} ({k[0]})"
        else:
            ev, note = ts_def_evidence(repos, k), f"the end of the chain ({k[0]})"
        gold.append(with_aliases({"name": k[1], "evidence": [ev], "note": note}))
    return gold


def vue_test_files(repos, g, target):
    c = g.callers_upto(target, 50)
    files = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0])):
        if not is_test(k[0]) or k[0] in files:
            continue
        hop = sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1)[0]
        files[k[0]] = {"name": k[0], "evidence": [ts_evidence(repos, g, k, hop)],
                       "note": f"chain length {d}: {k[1]} -> {hop[1]}", "_level": d}
    return [{k: v for k, v in x.items() if k != "_level"}
            for x in sorted(files.values(), key=lambda x: (x["_level"], x["name"]))]


# --------------------------------------------------------------------------
# Vue: questions that are not about calls
# --------------------------------------------------------------------------


def vue_app_config_files(repos: str, edges: str) -> tuple[list[dict], list[dict]]:
    reads: dict[str, tuple[str, int]] = {}
    writes: dict[str, tuple[str, int]] = {}
    for line in open(os.path.join(edges, VUE, "app-config.tsv")):
        if not line.strip():
            continue
        member, site, how = line.rstrip("\n").split("\t")
        f, ln = site.rsplit(":", 1)
        if f.endswith("runtime-core/src/apiCreateApp.ts"):
            continue
        (writes if how == "write" else reads).setdefault(f, (member, int(ln)))
    gold = [{"name": f, "evidence": [find_line(repos, VUE, f, m, ln)], "note": f"reads app.config.{m}"}
            for f, (m, ln) in sorted(reads.items())]
    optional = [{"name": f, "evidence": [find_line(repos, VUE, f, m, ln)],
                 "note": f"only assigns app.config.{m} (no read)"}
                for f, (m, ln) in sorted(writes.items()) if f not in reads]
    return gold, optional


PATCH_PROP = "packages/runtime-dom/src/patchProp.ts"
# Keys `shouldSetAsProp` sends to setAttribute on HTML (non-SVG) elements even
# when the element has a DOM property of that name (read from the function).
FORCE_ATTRS = [
    ("spellcheck", "key === 'spellcheck'", "enumerated attribute whose DOM property is boolean"),
    ("draggable", "key === 'draggable'", "enumerated attribute whose DOM property is boolean"),
    ("translate", "key === 'translate'", "enumerated attribute whose DOM property is boolean"),
    ("autocorrect", "key === 'autocorrect'", "enumerated attribute whose DOM property is boolean"),
    ("sandbox", "key === 'sandbox' && el.tagName === 'IFRAME'", "on <iframe>"),
    ("form", "if (key === 'form')", "read-only DOM property on form elements"),
    ("list", "key === 'list' && el.tagName === 'INPUT'", "on <input>"),
    ("type", "key === 'type' && el.tagName === 'TEXTAREA'", "on <textarea>"),
    ("width", "key === 'width' || key === 'height'", "on <img>, <video>, <canvas>, <source>"),
    ("height", "key === 'width' || key === 'height'", "on <img>, <video>, <canvas>, <source>"),
]

COMPONENT_OPTIONS = "packages/runtime-core/src/componentOptions.ts"
ARRAY_MERGED = ["beforeCreate", "created", "beforeMount", "mounted", "beforeUpdate", "updated", "beforeDestroy",
                "beforeUnmount", "destroyed", "unmounted", "activated", "deactivated", "errorCaptured",
                "serverPrefetch"]


def vue_node_interfaces(repos: str, edges: str) -> list[dict]:
    gold = []
    for line in open(os.path.join(edges, VUE, "ast-node.tsv")):
        if not line.strip():
            continue
        key, ln, how = line.rstrip("\n").split("\t")
        file, name = key.split("#")
        gold.append({"name": name, "evidence": [find_line(repos, VUE, file, f"interface {name}", int(ln))],
                     "note": f"{how}ly extends Node ({file})"})
    return sorted(gold, key=lambda x: x["name"])


def vue_unused_error_codes(repos: str) -> list[dict]:
    import re
    errors = "packages/compiler-core/src/errors.ts"
    ls = lines_of(repos, VUE, errors)
    start = next(i for i, t in enumerate(ls, 1) if t.startswith("export enum ErrorCodes"))
    members = []
    for i in range(start + 1, len(ls) + 1):
        if ls[i - 1].startswith("}"):
            break
        m = re.match(r"\s+([A-Z][A-Z0-9_]*)\b", ls[i - 1])
        if m:
            members.append((m.group(1), i))
    used = set()
    pk = os.path.join(repos, VUE, "packages")
    for p in sorted(os.listdir(pk)):
        for dp, dn, fn in os.walk(os.path.join(pk, p, "src")):
            for f in fn:
                path = os.path.join(dp, f)
                if not f.endswith(".ts") or os.path.relpath(path, os.path.join(repos, VUE)) == errors:
                    continue
                used |= set(re.findall(r"\bErrorCodes\.([A-Z][A-Z0-9_]*)", open(path, encoding="utf-8").read()))
    return [{"name": n, "evidence": [{"file": errors, "line": i, "contains": n}],
             "note": "declared in the enum; no `ErrorCodes." + n + "` outside errors.ts"}
            for n, i in members if n not in used]


def vue_force_attrs(repos: str) -> list[dict]:
    start = find_line(repos, VUE, PATCH_PROP, "function shouldSetAsProp(")["line"]
    return [{"name": k, "evidence": [find_line(repos, VUE, PATCH_PROP, want, start)], "note": note}
            for k, want, note in FORCE_ATTRS]


def vue_array_merged(repos: str) -> tuple[list[dict], list[dict]]:
    start = find_line(repos, VUE, COMPONENT_OPTIONS, "export const internalOptionMergeStrats")["line"]
    gold = [{"name": k, "evidence": [find_line(repos, VUE, COMPONENT_OPTIONS, f"{k}: mergeAsArray", start)],
             "note": "lifecycle hook, merged with mergeAsArray"} for k in ARRAY_MERGED]
    optional = [
        {"name": "watch", "evidence": [find_line(repos, VUE, COMPONENT_OPTIONS, "watch: mergeWatchOptions", start)],
         "note": "an object whose per-key handlers mergeWatchOptions concatenates with mergeAsArray"},
        {"name": "props", "evidence": [find_line(repos, VUE, COMPONENT_OPTIONS, "props: mergeEmitsOrPropsOptions", start)],
         "note": "array form only: mergeEmitsOrPropsOptions unions two arrays"},
        {"name": "emits", "evidence": [find_line(repos, VUE, COMPONENT_OPTIONS, "emits: mergeEmitsOrPropsOptions", start)],
         "note": "array form only: mergeEmitsOrPropsOptions unions two arrays"},
    ]
    return gold, optional


# --------------------------------------------------------------------------
# Questions
# --------------------------------------------------------------------------

DJ_RULES = ot.RULES
DJ_TESTS = (
    "A test file counts if code anywhere in it calls the function directly, or calls a function or method in the "
    "`django/` package that calls it directly. Count calls by name (imported names and re-exports included, also "
    "imports inside a function body), calls through `self`, `cls` or `super()` that resolve to a method the "
    "enclosing class defines or inherits (for example a test case calling an assertion method it inherits from "
    "`django.test.TestCase`), and class instantiation by the class's name. Calls on other objects' attributes (for "
    "example `request.get_host()`, `self.client.get(...)`) and passing a function as a value do not count. Answer "
    "with paths of files under `tests/`."
)
VUE_RULES = (
    "Consider only the non-test source files in `packages/*/src/` (not `__tests__/`, `packages-private/` or build "
    "scripts). Count only direct calls (`new` included) that resolve to the function's declaration, following "
    "imports, re-exports (`@vue/*` imports resolve to `packages/*/src`) and the static type of the receiver (so "
    "`this.method()` and `obj.method()` on a typed object count). A call inside a nested function, arrow function, "
    "callback or object-literal method counts for the outermost named function, method or `const`-bound function "
    "that contains it (for example, the functions defined inside `baseCreateRenderer` count as "
    "`baseCreateRenderer`), and code inside an object literal or callback that initialises a `const` counts for "
    "that constant (`const handlers = { get() { ... } }` counts as `handlers`). Calls through callback parameters, "
    "interface-typed values (such as `context.onError(...)` on a `TransformContext`) or other function values do "
    "not count."
)
VUE_TESTS = (
    "A test file is any file under a `packages/*/__tests__/` directory. It counts if code anywhere in it (inside "
    "`describe`/`test` callbacks, component definitions and helpers included) calls the function directly, or calls "
    "a function in `packages/*/src` from which the function is reachable through a chain of direct calls of any "
    "length. Count only direct calls (`new` included) that resolve to a declaration, following imports, re-exports "
    "(`@vue/*` and `vue` resolve to `packages/*/src`) and the static type of the receiver; inside non-test code a "
    "call in a nested function or callback counts for the outermost named function, method or `const`-bound "
    "function that contains it. Calls through callback parameters, interface-typed values or function values do not "
    "count (in particular, the `render`/`createApp` that `createRenderer` returns are function values). Answer with "
    "paths of test files."
)


def tc_q(name, path, depth, rules, scope):
    words = {2: "two", 3: "three"}[depth]
    return (f"Which functions or methods in {scope} call `{name}` ({path}) directly, call such a caller"
            + (", or call one of those" if depth == 3 else "")
            + f" - that is, every caller up to {words} levels up the call chain? {rules} Do not list `{name}` itself.")


def blast_q(subject, name, rules, scope):
    return (f"Suppose {subject}. Which files in {scope} contain a call to `{name}`, or a call to a function or "
            "method that calls it directly (two levels)? Name the file that contains each such call; code at module "
            f"level counts too. {rules}")


def flow_q(a, a_path, b, b_path, rules, scope):
    return (f"Which functions or methods in {scope} lie on at least one chain of direct calls that starts at `{a}` "
            f"({a_path}) and ends at `{b}` ({b_path})? List every function on any such chain, `{a}` and `{b}` "
            f"included. {rules}")


DJS = "the `django/` package"
VUES = "Vue's source (`packages/*/src`)"

CANDIDATES = [
    # ======================= Django =======================
    {"id": "django-tc-conditional-escape", "repo": DJ, "type": "transitive_callers", "kind": "symbols",
     "gold": ("dj_symbols", "django.utils.html:conditional_escape", 2),
     "question": tc_q("conditional_escape", "django/utils/html.py", 2, DJ_RULES, DJS),
     "rationale": "HTML escaping: level 1 spans template nodes, filters and `format_html`; level 2 reaches the "
                  "admin helpers and templatetags, forms rendering and the i18n template tags."},
    {"id": "django-tc-docs-version", "repo": DJ, "type": "transitive_callers", "kind": "symbols",
     "gold": ("dj_symbols", "django.utils.version:get_docs_version", 3),
     "question": tc_q("get_docs_version", "django/utils/version.py", 3, DJ_RULES, DJS),
     "rationale": "Documentation links in messages: management commands, the MySQL validation checks, the "
                  "migrations questioner, serializer and state, the CSRF and debug views; levels 2 and 3 run "
                  "through the migration serializers and writer and the exception responses."},
    {"id": "django-tc-default-renderer", "repo": DJ, "type": "transitive_callers", "kind": "symbols",
     "gold": ("dj_symbols", "django.forms.renderers:get_default_renderer", 3),
     "question": tc_q("get_default_renderer", "django/forms/renderers.py", 3, DJ_RULES, DJS),
     "rationale": "Form rendering: the `__init__` of forms, formsets and error lists and the template autoreloader; "
                  "levels 2 and 3 are the `__init__` methods of their subclasses (calling `super().__init__`)."},
    {"id": "django-blast-protected-type", "repo": DJ, "type": "blast_radius", "kind": "files",
     "gold": ("dj_files", "django.utils.encoding:is_protected_type", 2),
     "question": blast_q("the behaviour of `is_protected_type` (django/utils/encoding.py) changes",
                         "is_protected_type", DJ_RULES, DJS),
     "rationale": "An obscure helper: level 1 is `force_str` and `force_bytes` and the python serializer, so every "
                  "file calling `force_str` or `force_bytes` is level 2."},
    {"id": "django-blast-tree-node", "repo": DJ, "type": "blast_radius", "kind": "files",
     "gold": ("dj_files", "django.utils.tree:Node.__init__", 2),
     "question": ("Suppose the constructor of `Node` (django/utils/tree.py) gains a required parameter. Which files "
                  "in the `django/` package contain code that calls it - a call that resolves to `Node.__init__`, "
                  "including `super().__init__(...)` and instantiating a class that inherits this `__init__` "
                  "without defining its own - or a call to a function or method that does so directly (two "
                  "levels)? Name the file that contains each such call; code at module level counts too. "
                  + DJ_RULES),
     "rationale": "Level 1: `Q.__init__` (super), `WhereNode()` instantiations in `Query` and the related lookups, "
                  "`Node.create`; level 2: every file instantiating `Q(...)` or calling those `Query` methods."},
    {"id": "django-amb-color-make-style", "repo": DJ, "type": "ambiguous", "kind": "symbols",
     "gold": ("dj_symbols", "django.core.management.color:make_style", 3),
     "question": ("Two modules define a function named `make_style`. "
                  + tc_q("make_style", "django/core/management/color.py", 3, DJ_RULES, DJS).replace(
                      "directly,", "directly (not the `make_style` in django/utils/termcolors.py),", 1)),
     "rationale": "The management color helper wraps the termcolors one; its callers are `color_style`, "
                  "`BaseCommand` and the server log formatter, the termcolors function has its own."},
    {"id": "django-amb-loader-get-template", "repo": DJ, "type": "ambiguous", "kind": "symbols",
     "gold": ("dj_symbols", "django.template.loader:get_template", 2),
     "question": ("Django defines many functions and methods named `get_template`. "
                  + tc_q("get_template", "django/template/loader.py", 2, DJ_RULES, DJS).replace(
                      "directly,", "directly (not `Engine.get_template`, a loader's or template backend's "
                      "`get_template`, `BaseRenderer.get_template` or any other `get_template`),", 1)),
     "rationale": "The module-level loader function used by views, the admin, syndication and form renderers."},
    {"id": "django-amb-numberformat-format", "repo": DJ, "type": "ambiguous", "kind": "symbols",
     "gold": ("dj_symbols", "django.utils.numberformat:format", 3),
     "question": ("Several functions and methods in Django are named `format`. "
                  + tc_q("format", "django/utils/numberformat.py", 3, DJ_RULES, DJS).replace(
                      "directly,", "directly (not `format` in django/utils/dateformat.py, `ServerFormatter.format` "
                      "or Python's built-in `format`/`str.format`),", 1)),
     "rationale": "Number formatting: `number_format` in utils/formats.py, then localisation helpers and filters."},
    {"id": "django-flow-runserver-import-string", "repo": DJ, "type": "flow", "kind": "symbols",
     "gold": ("dj_flow", "django.contrib.staticfiles.management.commands.runserver:Command.get_handler",
              "django.utils.module_loading:import_string"),
     "question": flow_q("Command.get_handler", "django/contrib/staticfiles/management/commands/runserver.py",
                        "import_string", "django/utils/module_loading.py", DJ_RULES, DJS),
     "rationale": "From the static-files runserver through the core runserver, the WSGI application loading, "
                  "`django.setup`, middleware loading and the debug exception reporter to `import_string`."},
    {"id": "django-flow-translate-is-naive", "repo": DJ, "type": "flow", "kind": "symbols",
     "gold": ("dj_flow", "django.templatetags.i18n:TranslateNode.render", "django.utils.timezone:is_naive"),
     "question": flow_q("TranslateNode.render", "django/templatetags/i18n.py", "is_naive",
                        "django/utils/timezone.py", DJ_RULES, DJS),
     "rationale": "Template rendering of a translated value through localisation and date formatting to the "
                  "timezone check."},
    {"id": "django-tests-import-string", "repo": DJ, "type": "tests", "kind": "files",
     "gold": ("dj_test_files", "django.utils.module_loading:import_string", 2),
     "question": ("Which test files under `tests/` exercise `import_string` (django/utils/module_loading.py), "
                  "directly or through one function in `django/`? " + DJ_TESTS),
     "rationale": "Tests of module loading, plus tests calling the mail `get_connection`, static-files finders, "
                  "the WSGI loader and other functions that import a dotted path."},
    {"id": "django-tests-split-domain-port", "repo": DJ, "type": "tests", "kind": "files",
     "gold": ("dj_test_files", "django.http.request:split_domain_port", 2),
     "optional_files": ["tests/messages_tests/base.py"],
     "question": ("Which test files under `tests/` exercise `split_domain_port` (django/http/request.py), "
                  "directly or through one function or method in `django/`? " + DJ_TESTS),
     "rationale": "The host-validation tests call it directly; every test file that calls the inherited "
                  "`assertRedirects` (which calls it) is level 2. tests/messages_tests/base.py calls "
                  "`self.assertRedirects` in a mixin without a test-case base class (subclass dispatch, which "
                  "the rule excludes), so it is optional."},
    {"id": "django-cfg-deploy-checks", "repo": DJ, "type": "config", "kind": "symbols",
     "gold": ("dj_deploy_checks",),
     "question": ("Which of Django's built-in system checks run only when `manage.py check --deploy` is used? Name "
                  "the check functions in the `django/` package (not `tests/`)."),
     "rationale": "The functions registered with `@register(..., deploy=True)` in django/core/checks/ (security, "
                  "CSRF, sessions, caches, async)."},
    {"id": "django-cfg-safe-filters", "repo": DJ, "type": "config", "kind": "symbols",
     "gold": ("dj_safe_filters",),
     "question": ("Which built-in template filters in django/template/defaultfilters.py are registered with "
                  "`is_safe=True`? Name the Python functions."),
     "rationale": "Every function decorated with `@register.filter(..., is_safe=True)`; a filter registered under "
                  "another name (`escape`, `linebreaks`, ...) may also be answered by that name."},
    {"id": "django-cfg-empty-strings", "repo": DJ, "type": "config", "kind": "symbols",
     "gold": ("dj_empty_strings",),
     "question": ("Which model field classes defined in the django/db/models/fields/ package have "
                  "`empty_strings_allowed` equal to `False`, whether they set it themselves or inherit it from a "
                  "base class? Consider only classes defined in that package that are subclasses of "
                  "`django.db.models.Field`; name the classes."),
     "rationale": "Python's attribute lookup along each class's bases; `Field` sets it to True and a few classes "
                  "set False (their subclasses inherit it unless they set it back)."},
    {"id": "django-tests-number-format", "repo": DJ, "type": "tests", "kind": "files",
     "gold": ("dj_test_files", "django.utils.formats:number_format", 2),
     "question": ("Which test files under `tests/` exercise `number_format` (django/utils/formats.py), directly or "
                  "through one function in `django/`? " + DJ_TESTS),
     "rationale": "Formatting and localisation tests, plus tests of filters and admin display helpers."},
    {"id": "django-cfg-suspicious-subclasses", "repo": DJ, "type": "config", "kind": "symbols",
     "gold": ("dj_suspicious",),
     "question": ("Which classes in the `django/` package are subclasses of `SuspiciousOperation` "
                  "(django/core/exceptions.py), directly or indirectly? Name the classes."),
     "rationale": "Class bases by name across django/."},
    # Added after the calibration: replacements for the five questions that are
    # not about calls, all of which the calibration showed trivially solved.
    {"id": "django-cfg-widget-subclasses", "repo": DJ, "type": "config", "kind": "names", "after_calibration": True,
     "gold": ("dj_subclasses", "Widget"),
     "question": ("Which classes in the `django/` package are form widgets, that is subclasses of `django.forms.Widget` "
                  "(django/forms/widgets.py), directly or through other classes? Look at every app, contrib apps "
                  "included, and name the classes."),
     "rationale": "Class bases by name across django/ (Python's ast module), closed transitively: core widgets, the "
                  "admin's, auth's, GIS's and postgres's."},
    {"id": "django-cfg-optional-settings", "repo": DJ, "type": "config", "kind": "names", "after_calibration": True,
     "gold": ("dj_optional_settings",),
     "question": ("Which settings does code in the `django/` package read (as `settings.NAME`, or with `getattr` or "
                  "`hasattr` on `settings`) that have no default in django/conf/global_settings.py, so that they only "
                  "take effect when a project defines them? Ignore code under django/conf/ itself; name the settings."),
     "rationale": "Every `settings.NAME` attribute and `getattr`/`hasattr(settings, \"NAME\")` in django/ outside "
                  "django/conf/ (Python's ast module) whose NAME global_settings.py does not assign. `SETTINGS_MODULE`, "
                  "an attribute of the settings object rather than a project setting, is optional."},
    # ======================= Vue =======================
    {"id": "vue-tc-to-raw-type", "repo": VUE, "type": "transitive_callers", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/shared/src/general.ts", "toRawType"), 3),
     "question": tc_q("toRawType", "packages/shared/src/general.ts", 3, VUE_RULES, VUES),
     "rationale": "A shared type helper: reactivity's `createReactiveObject` and collection handlers and "
                  "runtime-core's prop validation; levels 2 and 3 reach the reactive APIs, component setup "
                  "and the compat layer."},
    {"id": "vue-tc-parse-string-style", "repo": VUE, "type": "transitive_callers", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/shared/src/normalizeProp.ts", "parseStringStyle"), 3),
     "question": tc_q("parseStringStyle", "packages/shared/src/normalizeProp.ts", 3, VUE_RULES, VUES),
     "rationale": "Inline-style parsing used by `normalizeStyle` and the DOM/SSR compilers; levels 2 and 3 reach "
                  "vnode creation, hydration, SSR attribute rendering and static stringification."},
    {"id": "vue-tc-code-frame", "repo": VUE, "type": "transitive_callers", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/shared/src/codeframe.ts", "generateCodeFrame"), 3),
     "question": tc_q("generateCodeFrame", "packages/shared/src/codeframe.ts", 3, VUE_RULES, VUES),
     "rationale": "Error code frames: the SFC compilers and `ScriptCompileContext`, the runtime compilers; level 3 "
                  "reaches every `define*` macro processor through `ctx.error`."},
    {"id": "vue-blast-handle-error", "repo": VUE, "type": "blast_radius", "kind": "files",
     "gold": ("vue_files", ("packages/runtime-core/src/errorHandling.ts", "handleError"), 2),
     "question": blast_q("the signature of `handleError` (packages/runtime-core/src/errorHandling.ts) changes",
                         "handleError", VUE_RULES, VUES),
     "rationale": "Error handling: level 1 is `callWithErrorHandling` and friends and a few direct callers; level 2 "
                  "every file that calls those."},
    {"id": "vue-blast-to-raw", "repo": VUE, "type": "blast_radius", "kind": "files",
     "gold": ("vue_files", ("packages/reactivity/src/reactive.ts", "toRaw"), 2),
     "question": blast_q("the behaviour of `toRaw` (packages/reactivity/src/reactive.ts) changes", "toRaw",
                         VUE_RULES, VUES),
     "rationale": "Reactivity's unwrap helper: level 1 spans reactivity's handlers and refs and runtime-core; level 2 "
                  "their callers across runtime-core and runtime-dom."},
    {"id": "vue-amb-reactivity-warn", "repo": VUE, "type": "ambiguous", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/reactivity/src/warning.ts", "warn"), 2),
     "question": ("Vue defines several functions named `warn`. "
                  + tc_q("warn", "packages/reactivity/src/warning.ts", 2, VUE_RULES, VUES).replace(
                      "directly,", "directly (not the `warn` of packages/runtime-core/src/warning.ts, of "
                      "packages/compiler-sfc/src/warn.ts or any other `warn`),", 1)),
     "rationale": "Reactivity's own warning helper, used inside the reactivity package; level 2 adds the reactive "
                  "APIs and the two runtime functions that call reactivity functions which warn."},
    {"id": "vue-amb-compiler-warn-deprecation", "repo": VUE, "type": "ambiguous", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/compiler-core/src/compat/compatConfig.ts", "warnDeprecation"), 3),
     "question": ("Two packages define a function named `warnDeprecation`. "
                  + tc_q("warnDeprecation", "packages/compiler-core/src/compat/compatConfig.ts", 3, VUE_RULES,
                         VUES).replace("directly,", "directly (not the `warnDeprecation` of "
                                       "packages/runtime-core/src/compat/compatConfig.ts),", 1)),
     "rationale": "The compiler's compat warning: the parser, filters and element transforms, then compiler-dom's "
                  "and compiler-ssr's transforms; the runtime function has unrelated callers."},
    {"id": "vue-amb-reactivity-watch", "repo": VUE, "type": "ambiguous", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/reactivity/src/watch.ts", "watch"), 3),
     "question": ("Two packages define a function named `watch`. "
                  + tc_q("watch", "packages/reactivity/src/watch.ts", 3, VUE_RULES, VUES).replace(
                      "directly,", "directly (not the `watch` of packages/runtime-core/src/apiWatch.ts),", 1)),
     "rationale": "Reactivity's base watcher is called by runtime-core's `doWatch`; above it the runtime watch "
                  "APIs and their users."},
    {"id": "vue-flow-ssr-escape-html", "repo": VUE, "type": "flow", "kind": "symbols",
     "gold": ("vue_flow", ("packages/server-renderer/src/renderToString.ts", "renderToString"),
              ("packages/shared/src/escapeHtml.ts", "escapeHtml")),
     "question": flow_q("renderToString", "packages/server-renderer/src/renderToString.ts", "escapeHtml",
                        "packages/shared/src/escapeHtml.ts", VUE_RULES, VUES),
     "rationale": "Server rendering reaches HTML escaping both at render time (vnode and attribute rendering) and "
                  "through the on-the-fly template compilation of `ssrCompile` into compiler-ssr."},
    {"id": "vue-flow-sfc-type-reference", "repo": VUE, "type": "flow", "kind": "symbols",
     "gold": ("vue_flow", ("packages/compiler-sfc/src/compileScript.ts", "compileScript"),
              ("packages/compiler-sfc/src/script/resolveType.ts", "resolveTypeReference")),
     "question": flow_q("compileScript", "packages/compiler-sfc/src/compileScript.ts", "resolveTypeReference",
                        "packages/compiler-sfc/src/script/resolveType.ts", VUE_RULES, VUES),
     "rationale": "`<script setup>` compilation: props, emits and model macros resolve TypeScript types through "
                  "the type resolver's many helpers."},
    {"id": "vue-tests-queue-job", "repo": VUE, "type": "tests", "kind": "files",
     "gold": ("vue_test_files", ("packages/runtime-core/src/scheduler.ts", "queueJob")),
     "question": ("Which test files exercise `queueJob` (packages/runtime-core/src/scheduler.ts), directly or "
                  "through functions in `packages/*/src`? " + VUE_TESTS),
     "rationale": "The scheduler tests call it directly; tests calling runtime-core's `watch`, `watchEffect`, "
                  "`useModel` or `useCssVars` reach it through `doWatch`."},
    {"id": "vue-tests-transform-preset", "repo": VUE, "type": "tests", "kind": "files",
     "gold": ("vue_test_files", ("packages/compiler-core/src/compile.ts", "getBaseTransformPreset")),
     "question": ("Which test files exercise `getBaseTransformPreset` (packages/compiler-core/src/compile.ts), "
                  "directly or through functions in `packages/*/src`? " + VUE_TESTS),
     "rationale": "Compiler tests that call it or `baseCompile`, and tests that compile templates through "
                  "compiler-dom's `compile` (runtime and DOM tests included)."},
    {"id": "vue-cfg-app-config-files", "repo": VUE, "type": "config", "kind": "files",
     "gold": ("vue_app_config_files",),
     "question": ("Which source files in `packages/*/src` read an option of the application config - a member of "
                  "the `AppConfig` interface (packages/runtime-core/src/apiCreateApp.ts), whether through a property "
                  "access or a destructuring? Do not count apiCreateApp.ts itself, comments or strings, or members "
                  "of other objects that happen to have the same name (such as compiler options). Answer with file "
                  "paths."),
     "rationale": "Accesses typed by the TypeScript checker as `AppConfig` (vue_config.js), outside apiCreateApp.ts. "
                  "A file that only assigns an option is optional."},
    {"id": "vue-cfg-force-attrs", "repo": VUE, "type": "config", "kind": "symbols",
     "gold": ("vue_force_attrs",),
     "question": ("When Vue's DOM renderer (runtime-dom) patches a prop on an HTML element (not SVG), it normally "
                  "sets a DOM property if the element has one. For which attribute names does it always use an "
                  "attribute instead, either on every element or on specific element types? List the attribute "
                  "names (keys)."),
     "rationale": "The special cases of `shouldSetAsProp` in packages/runtime-dom/src/patchProp.ts."},
    {"id": "vue-cfg-array-merged", "repo": VUE, "type": "config", "kind": "symbols",
     "gold": ("vue_array_merged",),
     "question": ("When Vue merges component options from mixins, `extends` and global mixins, which options are "
                  "concatenated into an array (so that every version runs) rather than overwritten or merged as "
                  "objects? List the option names."),
     "rationale": "The keys of `internalOptionMergeStrats` (packages/runtime-core/src/componentOptions.ts) whose "
                  "strategy is `mergeAsArray`; `watch` (per-key arrays) and array-form `props`/`emits` are optional."},
    {"id": "vue-cfg-unused-error-codes", "repo": VUE, "type": "config", "kind": "symbols",
     "gold": ("vue_unused_error_codes",),
     "question": ("Which members of the `ErrorCodes` enum of the template compiler (packages/compiler-core/src/errors.ts) "
                  "are never referenced in `packages/*/src` outside errors.ts itself, so the compiler can never "
                  "report them? List the member names."),
     "rationale": "Every `ErrorCodes.<member>` reference in packages/*/src outside errors.ts (whose message table "
                  "names every member), matched per member."},
    {"id": "vue-cfg-ast-node-interfaces", "repo": VUE, "type": "config", "kind": "names", "after_calibration": True,
     "gold": ("vue_node_interfaces",),
     "question": ("Which interfaces in `packages/*/src` extend the template compiler's AST `Node` interface "
                  "(packages/compiler-core/src/ast.ts), directly or through other interfaces? Not the DOM's `Node`; "
                  "name the interfaces."),
     "rationale": "Heritage clauses resolved by the TypeScript checker (vue_hierarchy.js), closed transitively."},
    {"id": "vue-tests-scope-dispose", "repo": VUE, "type": "tests", "kind": "files",
     "gold": ("vue_test_files", ("packages/reactivity/src/effectScope.ts", "onScopeDispose")),
     "question": ("Which test files exercise `onScopeDispose` (packages/reactivity/src/effectScope.ts), directly or "
                  "through functions in `packages/*/src`? " + VUE_TESTS),
     "rationale": "Effect-scope tests call it; component, SSR and async-component tests reach it through the "
                  "runtime helpers that register cleanups."},
    {"id": "vue-tc-queue-job", "repo": VUE, "type": "transitive_callers", "kind": "symbols",
     "gold": ("vue_symbols", ("packages/runtime-core/src/scheduler.ts", "queueJob"), 3),
     "question": tc_q("queueJob", "packages/runtime-core/src/scheduler.ts", 3, VUE_RULES, VUES),
     "rationale": "The scheduler's entry point: `doWatch`, the public instance's `$forceUpdate`, the renderer, "
                  "transitions and HMR; above them the watch APIs and renderer factories."},
]

TOOLS = {
    DJ: "py_edges.py --local-imports --star-imports (import-aware AST, `direct` edges)",
    VUE: "vue_edges.js (TypeScript 6.0 type checker, `direct` edges)",
}

DESCRIPTION = (
    "Outcome eval at scale (issue #163): candidate tasks over Django 5.1.4 (the django/ package and its tests/) and "
    "Vue 3.5.46 (vuejs/core, packages/*/src and __tests__) - transitive callers, blast radius, same-name "
    "disambiguation, call-chain flows, which tests reach a function, and questions that are not about calls. "
    "Gold sets were computed by evals/agent/groundtruth/scale_tasks.py from Python's ast module and the "
    "TypeScript type checker, never from myceliums output. Each gold item lists one evidence line that check-tasks "
    "re-verifies; its note gives the level and the next hop, or why it counts."
)


# The frozen set (#163): the ten calibrated candidates that were not trivially
# solved, the uncalibrated call-graph candidates, and the replacements added
# after the calibration for the questions that are not about calls.
FINAL = [
    "django-tc-conditional-escape", "django-tc-docs-version", "django-tc-default-renderer",
    "django-blast-tree-node", "django-blast-protected-type",
    "django-amb-numberformat-format", "django-amb-color-make-style", "django-amb-loader-get-template",
    "django-flow-runserver-import-string", "django-flow-translate-is-naive",
    "django-tests-split-domain-port", "django-tests-import-string", "django-tests-number-format",
    "django-cfg-widget-subclasses", "django-cfg-optional-settings",
    "vue-tc-code-frame", "vue-tc-to-raw-type", "vue-tc-parse-string-style",
    "vue-blast-handle-error", "vue-blast-to-raw",
    "vue-amb-reactivity-warn", "vue-amb-compiler-warn-deprecation", "vue-amb-reactivity-watch",
    "vue-flow-ssr-escape-html", "vue-flow-sfc-type-reference",
    "vue-tests-queue-job", "vue-tests-transform-preset", "vue-tests-scope-dispose",
    "vue-cfg-app-config-files", "vue-cfg-ast-node-interfaces",
]
# Calibrated (2026-11-22-scale-calibration, condition none, 10 turns).
CALIBRATED = {
    "django-tc-conditional-escape": "F1 1.00, 6 tool calls", "django-blast-tree-node": "F1 0.85, 5 tool calls",
    "django-amb-numberformat-format": "F1 1.00, 6 tool calls",
    "django-flow-runserver-import-string": "F1 0.69, 2 tool calls",
    "django-tests-split-domain-port": "F1 1.00, 5 tool calls", "vue-tc-code-frame": "F1 0.83, 7 tool calls",
    "vue-blast-handle-error": "F1 1.00, 6 tool calls", "vue-amb-reactivity-warn": "F1 1.00, 6 tool calls",
    "vue-flow-ssr-escape-html": "F1 1.00, 9 tool calls", "vue-tests-queue-job": "F1 0.52, 8 tool calls",
}

FINAL_DESCRIPTION = (
    "Outcome eval at scale (issue #163): 30 tasks, 15 over Django 5.1.4 (the django/ package and its tests/) and 15 "
    "over Vue 3.5.46 (vuejs/core, packages/*/src and __tests__): transitive callers, blast radius, same-name "
    "disambiguation, call-chain flows, which tests reach a function (6, 4, 6, 4, 6 tasks) and 4 questions that are "
    "not about calls (class and interface hierarchies, settings without a default, files that read app.config). "
    "Gold sets were computed by evals/agent/groundtruth/scale_tasks.py from Python's ast module and the TypeScript "
    "type checker, never from myceliums output; the task set was chosen with a 15-session calibration "
    "(results/2026-11-22-scale-calibration) and frozen before any myceliums index of Vue existed or any session "
    "with myceliums ran. Each gold item lists one evidence line that check-tasks re-verifies; its note gives the "
    "level and next hop, or why it counts. `calibration` records the calibration result where there is one."
)


def build(edges: str, repos: str, only: list[str] | None = None, mode: str = "final") -> dict:
    gdj = dj_graph(edges)
    gdj_t = dj_graph(edges, tests=True)
    gvue = vue_graph(edges, tests=False)
    gvue_t = vue_graph(edges, tests=True)
    tasks = []
    specs = {s["id"]: s for s in CANDIDATES}
    if mode == "final":
        order = [specs[i] for i in FINAL]
    elif mode == "calibration":  # exactly the candidate file the calibration ran on
        order = [s for s in CANDIDATES if not s.get("after_calibration")]
    else:
        order = list(CANDIDATES)
    for spec in order:
        if only and spec["id"] not in only:
            continue
        fn, *args = spec["gold"]
        optional: list[dict] = []
        if fn == "dj_symbols":
            gold = dj_symbols(repos, gdj, *args)
        elif fn == "dj_files":
            gold = dj_files(repos, gdj, *args)
        elif fn == "dj_flow":
            gold = dj_flow(repos, gdj, *args)
        elif fn == "dj_test_files":
            opt = spec.get("optional_files", [])
            gold = dj_test_files(repos, gdj_t, *args, optional=opt)
            optional = [{"name": f, "evidence": [find_line(repos, DJ, f, "self.assertRedirects(")],
                         "note": "calls assertRedirects in a mixin without a TestCase base (subclass dispatch)"}
                        for f in opt]
        elif fn == "dj_deploy_checks":
            gold = dj_deploy_checks(repos)
        elif fn == "dj_safe_filters":
            gold = dj_safe_filters(repos)
        elif fn == "dj_empty_strings":
            gold, optional = dj_empty_strings(repos)
        elif fn == "dj_subclasses":
            gold = dj_subclasses(repos, *args)
        elif fn == "dj_optional_settings":
            gold, optional = dj_optional_settings(repos)
        elif fn == "vue_node_interfaces":
            gold = vue_node_interfaces(repos, edges)
        elif fn == "dj_suspicious":
            gold = dj_suspicious(repos)
        elif fn == "vue_symbols":
            gold = vue_symbols(repos, gvue, *args)
        elif fn == "vue_files":
            gold = vue_files(repos, gvue, *args)
        elif fn == "vue_flow":
            gold = vue_flow(repos, gvue, *args)
        elif fn == "vue_test_files":
            gold = vue_test_files(repos, gvue_t, *args)
        elif fn == "vue_app_config_files":
            gold, optional = vue_app_config_files(repos, edges)
        elif fn == "vue_force_attrs":
            gold = vue_force_attrs(repos)
        elif fn == "vue_unused_error_codes":
            gold = vue_unused_error_codes(repos)
        elif fn == "vue_array_merged":
            gold, optional = vue_array_merged(repos)
        else:
            raise SystemExit(f"unknown gold builder {fn}")
        how = ("Computed from the " + TOOLS[spec["repo"]] + " call graph" if spec["type"] != "config"
               else "Derived by " + ("Python's ast module" if spec["repo"] == DJ else
                                     "the TypeScript checker / reading the code"))
        task = {"id": spec["id"], "repo": spec["repo"], "type": spec["type"], "answer_kind": spec["kind"],
                "question": spec["question"], "gold": gold,
                "rationale": f"{spec['rationale']} {how}; {len(gold)} items.",
                "verified_with": [TOOLS[spec["repo"]] if spec["type"] != "config" else how, "reading the code"]}
        if mode == "final" and spec["type"] == "blast_radius":
            # Amended after the run (#163 review): under the owner policy of
            # #164/#168, recursion and self-calls are optional, and the call
            # graph drops self-edges, so a file whose only call is the target
            # calling itself was missing from both lists.
            names = {x["name"] for x in gold} | {x["name"] for x in optional}
            for f, ln in sorted(self_call_files(edges, spec["repo"], args[0]).items()):
                if f not in names:
                    want = args[0][1] if spec["repo"] == VUE else args[0].split(":", 1)[1].split(".")[-1]
                    optional.append({"name": f, "evidence": [find_line(repos, spec["repo"], f, want, ln)],
                                     "note": "only a recursive call of the target inside itself (optional: "
                                             "recursion and self-calls, owner policy of #164/#168)"})
                    task["rationale"] += (f" Amended after the run (#163 review): {f} is optional, its only call "
                                          "is the target's recursive call of itself.")
        if optional:
            task["optional"] = optional
        if mode == "final":
            task["calibration"] = CALIBRATED.get(spec["id"], "not calibrated" + (
                " (added after the calibration)" if spec.get("after_calibration") else ""))
        tasks.append(task)
    return {"description": FINAL_DESCRIPTION if mode == "final" else DESCRIPTION, "tasks": tasks}


if __name__ == "__main__":
    if len(sys.argv) >= 4 and sys.argv[1] == "edges":
        make_edges(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 4 and sys.argv[1] == "build":
        mode = "final"
        for flag in ("--calibration", "--all"):
            if flag in sys.argv:
                mode = flag[2:]
        json.dump(build(sys.argv[2], sys.argv[3], mode=mode), sys.stdout, indent=1, ensure_ascii=False)
        print()
    else:
        sys.exit(__doc__)
