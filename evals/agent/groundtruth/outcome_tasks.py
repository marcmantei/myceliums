"""Gold sets for the outcome eval (#153): twelve multi-hop tasks over Django 5.1.4.

The call graph comes from Python's ``ast`` module, never from myceliums:

    python3 -I py_edges.py --local-imports --star-imports <repos>/django django > <edges>/django/edges.tsv
    python3 -I outcome_tasks.py <edges> <repos> > ../tasks/tasks-outcome.json
    python3 -I outcome_tasks.py <edges> <repos> --audit      # grep audit, see below

``py_edges.py`` resolves calls by name through imports (function-local ones
and ``from x import *`` re-exports included), ``self``/``cls``/``super()``
through the enclosing class and its bases, and class instantiation by name
(an edge to the ``__init__`` the class defines or inherits). Only ``direct``
edges whose call site lies in ``django/`` are followed; every question spells
out that rule.

Task types, four tasks each:

* ``transitive_callers``: every caller up to three levels up the call chain;
* ``blast_radius``: the files holding a call to X or to a direct caller of X
  (15 to 24 files);
* ``ambiguous``: callers (two or three levels) of one of several same-named
  functions in different apps, the others named in the question.

``--audit`` greps every name on each chain (the target and every gold
function below the last level) for call sites in ``django/`` and prints the
ones the graph does not account for: neither an edge into that node, nor an
attribute call on another object (excluded by the rule), nor a call through
``self`` that resolves to another class. Each printed site was read; the
verdicts are in the task rationales.
"""

from __future__ import annotations

import collections
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hard_tasks import Graph  # noqa: E402

REPO = "django"


def load(edges_dir: str) -> tuple[Graph, dict]:
    g = Graph()
    unresolved = collections.defaultdict(list)  # attr -> [site]
    for line in open(os.path.join(edges_dir, REPO, "edges.tsv")):
        a, b, site, kind = line.rstrip("\n").split("\t")
        if not site.startswith("django/"):
            continue
        if kind == "direct":
            g.add(a, b, site)
        else:
            unresolved[b[2:]].append((a, site))
    return g, unresolved


def name_of(key: str) -> str:
    return key.split(":", 1)[1]


def short(key: str) -> str:
    n = name_of(key).split(".")
    return n[-2] if n[-1] == "__init__" and len(n) > 1 else n[-1]


def file_lines(repos: str, file: str) -> list[str]:
    with open(os.path.join(repos, REPO, file), encoding="utf-8", errors="replace") as fh:
        return fh.read().splitlines()


def evidence(repos: str, g: Graph, a: str, b: str) -> dict:
    """One call site from ``a`` to ``b`` whose line names ``b`` (a constructor
    reached through ``super().__init__`` is quoted as such)."""
    wants = [short(b)]
    if name_of(b).endswith("__init__"):
        wants.append("__init__(")
    sites = sorted(g.sites[(a, b)], key=lambda s: (s.rsplit(":", 1)[0], int(s.rsplit(":", 1)[1])))
    for want in wants:
        for site in sites:
            file, ln = site.rsplit(":", 1)
            lines = file_lines(repos, file)
            for k in range(int(ln), min(int(ln) + 4, len(lines) + 1)):  # multi-line calls
                if want in lines[k - 1]:
                    return {"file": file, "line": k, "contains": want}
    file, ln = sites[0].rsplit(":", 1)
    return {"file": file, "line": int(ln), "contains": file_lines(repos, file)[int(ln) - 1].strip()[:60]}


def callers(g: Graph, target: str, depth: int) -> dict[str, int]:
    return {k: d for k, d in g.callers_upto(target, depth).items() if not k.endswith(":<module>")}


def gold_symbols(repos: str, g: Graph, target: str, depth: int) -> list[dict]:
    """Every caller up to ``depth`` levels, as answer names. Two functions of
    one name (the ``serve`` of django/views/static.py and of
    django/contrib/staticfiles/views.py) are one answer item, at the lower level."""
    c = callers(g, target, depth)
    gold, seen = [], set()
    for k, d in sorted(c.items(), key=lambda x: (x[1], name_of(x[0]), x[0])):
        if name_of(k) in seen:
            continue
        seen.add(name_of(k))
        hop = sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1)[0]
        gold.append({"name": name_of(k), "evidence": [evidence(repos, g, k, hop)],
                     "note": f"level {d}, via {name_of(hop)} ({k.split(':')[0]})"})
    return gold


def gold_files(repos: str, g: Graph, target: str, depth: int) -> list[dict]:
    """The files holding a call to ``target`` (level 1) or to one of its
    callers up to ``depth - 1`` levels; module-level code counts."""
    c = g.callers_upto(target, depth)
    files: dict[str, dict] = {}
    for k, d in sorted(c.items(), key=lambda x: (x[1], x[0])):
        for hop in sorted(x for x in g.out[k] if x == target or c.get(x) == d - 1):
            f = sorted(g.sites[(k, hop)])[0].rsplit(":", 1)[0]
            if f in files:
                continue
            files[f] = {"name": f, "evidence": [evidence(repos, g, k, hop)],
                        "note": f"level {d}: {name_of(k)} -> {name_of(hop)}", "_level": d}
    return [{k: v for k, v in x.items() if k != "_level"}
            for x in sorted(files.values(), key=lambda x: (x["_level"], x["name"]))]


# --------------------------------------------------------------------------
# Questions
# --------------------------------------------------------------------------

RULES = (
    "Consider only the `django/` package (not `tests/`, `docs/` or other top-level directories). Count direct "
    "calls by name: imported names and re-exports count (also imports inside a function body and "
    "`module.function(...)` after `from django.utils import formats`), and so do calls through `self`, `cls` or "
    "`super()`, which count for the method the enclosing class defines or inherits from its base classes (not for "
    "overrides in subclasses), and class instantiation by the class's name, which counts as a call of the "
    "`__init__` the class defines or inherits. A call inside a nested function, lambda or decorator wrapper counts "
    "for the outermost enclosing function or method. Calls on other objects' attributes (for example "
    "`model_admin.get_queryset(request)`, `self.storage.path(name)`, `request.user.has_perm(...)`), instantiation "
    "through a variable (`redirect_class(url)`) and passing a function as a value (decorators applied with `@`, "
    "callbacks) do not count. Ignore docstrings and comments."
)

TOOL = "py_edges.py --local-imports --star-imports (import-aware AST, `direct` edges)"
VERIFIED = [TOOL, "grep audit of every name on each chain (outcome_tasks.py --audit)", "reading the call sites"]

LEVELS = ("Computed as all callers up to {depth} levels up the {tool} call graph; {{n}} functions. Each note "
          "gives the level, the function called on the way to the target and the caller's module.")
FILES = ("Computed from the {tool} call graph: the files holding a call to the target (level 1) or to one of "
         "its direct callers (level 2); {{n}} files. Each note names the calling function and its callee.")


def tc(depth: int, name: str, path: str) -> str:
    words = {2: "two", 3: "three"}[depth]
    return (f"Which functions or methods in the `django/` package call `{name}` ({path}) directly, call such a "
            f"caller, or call one of those - that is, every caller up to {words} levels up the call chain? "
            f"{RULES} Do not list `{name}` itself.")


def blast(subject: str, name: str) -> str:
    return (f"Suppose {subject}. Which files in the `django/` package contain a call to `{name}`, or a call to a "
            "function or method that calls it directly (two levels)? Name the file that contains each such call; "
            f"code at module level counts too. {RULES}")


SPECS = [
    # --- transitive callers, three levels --------------------------------
    {"id": "django-tc-permission-codename", "type": "transitive_callers", "kind": "symbols", "depth": 3,
     "target": "django.contrib.auth:get_permission_codename",
     "question": tc(3, "get_permission_codename", "django/contrib/auth/__init__.py"),
     "rationale": "Admin permission checks: level 1 is the four `has_*_permission` methods of `BaseModelAdmin`, "
                  "`InlineModelAdmin._has_any_perms_for_target_model` and the permission builder of "
                  "contrib/auth/management; the levels above run through `self.has_*_permission(...)` in "
                  "ModelAdmin, InlineModelAdmin and UserAdmin views. "},
    {"id": "django-tc-get-format", "type": "transitive_callers", "kind": "symbols", "depth": 3,
     "target": "django.utils.formats:get_format",
     "question": tc(3, "get_format", "django/utils/formats.py"),
     "rationale": "Locale formats: level 1 spans utils/formats.py, forms widgets and fields and the JavaScript "
                  "catalog; levels 2 and 3 reach template filters (`date`, `floatformat`), humanize, the admin "
                  "and form field `to_python` methods. `localize` names two functions (utils/formats.py and "
                  "templatetags/l10n.py), one answer item. "},
    {"id": "django-tc-safe-join", "type": "transitive_callers", "kind": "symbols", "depth": 3,
     "target": "django.utils._os:safe_join",
     "question": tc(3, "safe_join", "django/utils/_os.py"),
     "rationale": "Path joining for storage, template loaders and static files: most of the levels above are "
                  "`self.path(name)` in FileSystemStorage and `self._resolve(...)` in InMemoryStorage. `serve` "
                  "names two functions (views/static.py and contrib/staticfiles/views.py), one answer item. "},
    {"id": "django-tc-resolve-url", "type": "transitive_callers", "kind": "symbols", "depth": 3,
     "target": "django.shortcuts:resolve_url",
     "question": tc(3, "resolve_url", "django/shortcuts.py"),
     "rationale": "Redirect targets: level 1 is contrib/auth's views, mixins, middleware and decorators and "
                  "`redirect`; `user_passes_test` calls it inside its nested wrapper, so the decorator factories "
                  "built on it (`login_required`, `permission_required`, `staff_member_required`) are level 2. "},
    # --- blast radius, files, two levels ----------------------------------
    {"id": "django-blast-iri-to-uri", "type": "blast_radius", "kind": "files", "depth": 2,
     "target": "django.utils.encoding:iri_to_uri",
     "question": blast("the behaviour of `iri_to_uri` (django/utils/encoding.py) changes", "iri_to_uri"),
     "rationale": "Level 1 includes `HttpResponseRedirectBase.__init__`, so every file that instantiates "
                  "`HttpResponseRedirect` or `HttpResponsePermanentRedirect` by name is level 2 (inherited "
                  "`__init__`). django/shortcuts.py is not: `redirect` instantiates through a variable. "},
    {"id": "django-blast-current-timezone", "type": "blast_radius", "kind": "files", "depth": 2,
     "target": "django.utils.timezone:get_current_timezone",
     "question": blast("the behaviour of `get_current_timezone` (django/utils/timezone.py) changes",
                       "get_current_timezone"),
     "rationale": "Level 1 includes the helpers in utils/timezone.py (`localtime`, `make_aware`, `make_naive`, "
                  "`get_current_timezone_name`) that call it without an argument, so their callers across "
                  "the admin, database backends, forms, HTTP and views are level 2. "},
    {"id": "django-blast-template-does-not-exist", "type": "blast_radius", "kind": "files", "depth": 2,
     "target": "django.template.exceptions:TemplateDoesNotExist.__init__",
     "question": ("Suppose the constructor of `TemplateDoesNotExist` (django/template/exceptions.py) gains a "
                  "required parameter. Which files in the `django/` package contain code that instantiates "
                  "`TemplateDoesNotExist` (`raise TemplateDoesNotExist(...)` included), or a call to a function or "
                  "method that does so directly (two levels)? Name the file that contains each such call; code at "
                  "module level counts too. " + RULES),
     "rationale": "Level 1: the template backends, engine, loaders and `get_template` in template/loader.py; "
                  "level 2 adds the callers of those (`get_template` and `select_template` users in admin, "
                  "flatpages, syndication, forms renderers and views). "},
    {"id": "django-blast-script-prefix", "type": "blast_radius", "kind": "files", "depth": 2,
     "target": "django.urls.base:get_script_prefix",
     "question": blast("the behaviour of the function `get_script_prefix` in django/urls/base.py changes (not the "
                       "`get_script_prefix` method of `ASGIHandler` in django/core/handlers/asgi.py)",
                       "get_script_prefix"),
     "rationale": "Level 1 includes `reverse` in urls/base.py, so every file that calls `reverse` is level 2. "},
    # --- ambiguous names ---------------------------------------------------
    {"id": "django-amb-transaction-get-connection", "type": "ambiguous", "kind": "symbols", "depth": 3,
     "target": "django.db.transaction:get_connection",
     "question": ("Django defines several functions and methods named `get_connection`. "
                  + tc(3, "get_connection", "django/db/transaction.py").replace(
                      "directly,", "directly (not `get_connection` in django/core/mail/__init__.py, "
                      "`EmailMessage.get_connection` or any other `get_connection`),", 1)),
     "rationale": "Level 1 is the module-level API of db/transaction.py and `Atomic`; levels 2 and 3 are its "
                  "callers through `transaction.<function>(...)`. The mail `get_connection` has its own, "
                  "unrelated callers. "},
    {"id": "django-amb-migrations-resolve-relation", "type": "ambiguous", "kind": "symbols", "depth": 2,
     "target": "django.db.migrations.utils:resolve_relation",
     "question": ("Two modules define a function named `resolve_relation`. "
                  + tc(2, "resolve_relation", "django/db/migrations/utils.py").replace(
                      "directly,", "directly (not the `resolve_relation` in django/db/models/fields/related.py),", 1)),
     "rationale": "The migrations helper is used by the autodetector, operations and project state; the "
                  "same-named function in db/models/fields/related.py serves lazy model relations. "},
    {"id": "django-amb-urls-resolve", "type": "ambiguous", "kind": "symbols", "depth": 2,
     "target": "django.urls.base:resolve",
     "question": ("Many functions and methods in Django are named `resolve`. "
                  + tc(2, "resolve", "django/urls/base.py").replace(
                      "directly,", "directly (not `URLResolver.resolve`, `URLPattern.resolve`, `Variable.resolve`, "
                      "`FilterExpression.resolve` or any other `resolve`),", 1)),
     "rationale": "The module-level URL resolver function; its level-2 callers are mostly admin views through "
                  "`add_preserved_filters` and middleware through `is_valid_path`. "},
    {"id": "django-amb-auth-load-backend", "type": "ambiguous", "kind": "symbols", "depth": 3,
     "target": "django.contrib.auth:load_backend",
     "question": ("Two modules define a function named `load_backend`. "
                  + tc(3, "load_backend", "django/contrib/auth/__init__.py").replace(
                      "directly,", "directly (not the database `load_backend` in django/db/utils.py),", 1)),
     "rationale": "The authentication backend loader: level 1 is contrib/auth (`_get_backends`, `get_user`), the "
                  "remote-user middleware, `UserManager.with_perm` and the test client; levels 2 and 3 reach "
                  "`authenticate`, `login` and their callers. `get_user` names two functions (contrib/auth/"
                  "__init__.py and contrib/auth/middleware.py), one answer item. "},
]


DESCRIPTION = (
    "Outcome eval (issue #153): twelve multi-hop tasks over Django 5.1.4 (the django/ package) - transitive "
    "callers three levels up, blast radius over 16 to 24 files, and callers of one of several same-named "
    "functions. Gold sets were computed by evals/agent/groundtruth/outcome_tasks.py from the call graph of "
    "py_edges.py (Python's ast module, import-aware, function-local and star imports included), never from "
    "myceliums output, and audited by grepping every name on each chain. Each gold item lists one evidence line "
    "(a call site on the chain) that check-tasks re-verifies; its note gives the level and the next hop."
)


def build(edges_dir: str, repos: str) -> dict:
    g, _ = load(edges_dir)
    tasks = []
    for spec in SPECS:
        gold = (gold_symbols if spec["kind"] == "symbols" else gold_files)(repos, g, spec["target"], spec["depth"])
        rationale = spec["rationale"] + (LEVELS if spec["kind"] == "symbols" else FILES).format(
            depth=spec["depth"], tool=TOOL).format(n=len(gold))
        tasks.append({"id": spec["id"], "repo": REPO, "type": spec["type"], "answer_kind": spec["kind"],
                      "question": spec["question"], "gold": gold, "rationale": rationale,
                      "verified_with": VERIFIED})
    return {"description": DESCRIPTION, "tasks": tasks}


# --------------------------------------------------------------------------
# Audit: grep every name on each chain for call sites the graph misses
# --------------------------------------------------------------------------

def audit(edges_dir: str, repos: str) -> int:
    """Call sites in django/ (from the AST) of every name on each chain that
    the graph does not link to that node, minus the ones the rule explains:

    * the site links to another function or method of the same name (a
      different class's method through ``self``, another module's function);
    * an attribute call on another object (``obj.name(...)``);
    * a bare name the calling module binds to something else (``path(...)``
      is ``django.urls.path``), or a local variable holding a function value.

    What is left is printed for reading; the verdicts are in the rationales.
    """
    import ast
    import py_edges
    py_edges.LOCAL_IMPORTS = py_edges.STAR_IMPORTS = True
    root = os.path.join(repos, REPO)
    py_edges.ROOT = root
    mods = py_edges.load(root, ["django"])
    mod_of_file = {os.path.relpath(m.path, root): m for m in mods.values()}
    g, _ = load(edges_dir)
    calls = collections.defaultdict(list)  # short name -> [(site, receiver, module)]
    for rel, m in sorted(mod_of_file.items()):
        for node in ast.walk(m.tree):
            if not isinstance(node, ast.Call):
                continue
            fn_ = node.func
            if isinstance(fn_, ast.Name):
                calls[fn_.id].append((f"{rel}:{node.lineno}", None, m))
            elif isinstance(fn_, ast.Attribute):
                v = fn_.value
                recv = (v.id if isinstance(v, ast.Name) else "super()" if isinstance(v, ast.Call)
                        and isinstance(v.func, ast.Name) and v.func.id == "super" else "<expr>")
                calls[fn_.attr].append((f"{rel}:{node.lineno}", recv, m))
    into = collections.defaultdict(set)  # callee -> sites with an edge into it
    by_short = collections.defaultdict(set)  # short name -> sites with an edge into a node of that name
    for (a, b), sites in g.sites.items():
        into[b] |= sites
        by_short[short(b)] |= sites
    flagged = 0
    for spec in SPECS:
        c = g.callers_upto(spec["target"], spec["depth"])
        chain = [spec["target"]] + [k for k, d in c.items() if d < spec["depth"] and not k.endswith("<module>")]
        print(f"== {spec['id']}: {len(chain)} names on the chain")
        for node in sorted(chain):
            n = short(node)
            node_mod = node.split(":")[0]
            for site, recv, m in calls[n]:
                if site in into[node] or site in by_short[n]:
                    continue
                if recv is None:
                    r = py_edges.lookup(mods, m.name, n)
                    if r is None or r[0] != node_mod:
                        continue  # bound to something else here, or a local variable
                elif recv not in ("self", "cls", "super()"):
                    r = py_edges.lookup(mods, m.name, recv)
                    if r is None or (r[1] and r[0] + ":" + r[1] != node_mod + ":" + name_of(node).split(".")[0]) \
                            or (not r[1] and r[0] != node_mod):
                        continue  # another object's attribute, or another module or class
                flagged += 1
                f, ln = site.rsplit(":", 1)
                print(f"   ? {node} <- {site} [{recv}]: {file_lines(repos, f)[int(ln) - 1].strip()[:100]}")
    print(f"{flagged} call sites to review")
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    if "--audit" in sys.argv[3:]:
        sys.exit(audit(sys.argv[1], sys.argv[2]))
    json.dump(build(sys.argv[1], sys.argv[2]), sys.stdout, indent=1, ensure_ascii=False)
    print()
