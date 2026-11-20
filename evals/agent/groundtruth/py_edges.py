"""Ground-truth helper for the hard Python (FastAPI) tasks (#125): call edges
resolved through imports, ``self``/``cls`` and class names.

    python3 -I py_edges.py <repoDir> [<package> ...] > edges.tsv

One line per call site::

    caller <TAB> callee <TAB> file:line <TAB> kind

Callers and callees are ``module:qualname``; ``qualname`` is ``func`` or
``Class.method``, and code in a nested function or lambda belongs to the
outermost enclosing named function (``<module>`` at top level). ``kind``:

* ``direct`` - a name the module defines or imports (``from .x import f``,
  ``import a.b as m; m.f()``), ``self.m()`` / ``cls.m()`` / ``super().m()``
  inside a class (looked up through base classes defined in the scanned
  packages), ``Class.m()`` and ``Class()`` (edge to ``Class.__init__`` when
  the class or one of its bases defines it, else to ``Class``);
* ``unresolved`` - an attribute call on any other object, recorded as
  ``?.<attr>`` so a reader can audit what was not resolved.

Re-exports (``fastapi/__init__.py`` importing from submodules) are followed.
Standard library and third-party calls are dropped. This is static name
resolution, not type inference: every gold set built from it was checked by
reading the call sites.
"""

from __future__ import annotations

import ast
import os
import sys


def module_name(root: str, path: str) -> str:
    rel = os.path.relpath(path, root)[:-3].replace(os.sep, ".")
    return rel[: -len(".__init__")] if rel.endswith(".__init__") else rel


class Module:
    def __init__(self, name: str, path: str, tree: ast.Module, is_pkg: bool):
        self.name, self.path, self.tree, self.is_pkg = name, path, tree, is_pkg
        self.defs: dict[str, str] = {}  # local name -> "func" | "class"
        self.imports: dict[str, tuple[str, str | None]] = {}  # alias -> (module, attr or None)
        self.classes: dict[str, ast.ClassDef] = {}

    def resolve_relative(self, level: int, mod: str | None) -> str:
        if level == 0:
            return mod or ""
        parts = self.name.split(".")
        base = parts if self.is_pkg else parts[:-1]
        base = base[: len(base) - (level - 1)] if level > 1 else base
        return ".".join(base + ([mod] if mod else []))


def load(root: str, packages: list[str]) -> dict[str, Module]:
    mods: dict[str, Module] = {}
    for pkg in packages:
        for dp, dn, fn in os.walk(os.path.join(root, pkg)):
            dn[:] = sorted(d for d in dn if d not in ("__pycache__", ".git"))
            for f in sorted(fn):
                if not f.endswith(".py"):
                    continue
                p = os.path.join(dp, f)
                try:
                    tree = ast.parse(open(p, encoding="utf-8").read(), p)
                except SyntaxError:
                    continue
                m = Module(module_name(root, p), p, tree, f == "__init__.py")
                mods[m.name] = m
    for m in mods.values():
        for node in m.tree.body:
            _collect(m, node)
    return mods


def _collect(m: Module, node: ast.stmt) -> None:
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        m.defs[node.name] = "func"
    elif isinstance(node, ast.ClassDef):
        m.defs[node.name] = "class"
        m.classes[node.name] = node
    elif isinstance(node, ast.ImportFrom):
        src = m.resolve_relative(node.level, node.module)
        for a in node.names:
            m.imports[a.asname or a.name] = (src, a.name)
    elif isinstance(node, ast.Import):
        for a in node.names:
            if a.asname:
                m.imports[a.asname] = (a.name, None)
            else:
                m.imports[a.name.split(".")[0]] = (a.name.split(".")[0], None)
    elif isinstance(node, (ast.If, ast.Try)):
        for sub in node.body + getattr(node, "orelse", []) + getattr(node, "finalbody", []):
            _collect(m, sub)
        for h in getattr(node, "handlers", []):
            for sub in h.body:
                _collect(m, sub)


def lookup(mods: dict[str, Module], mod: str, name: str, depth: int = 0) -> tuple[str, str] | None:
    """(module, name) where ``mod.name`` is defined, following re-exports."""
    if depth > 10 or mod not in mods:
        if f"{mod}.{name}" in mods:
            return (f"{mod}.{name}", "")
        return None
    m = mods[mod]
    if name in m.defs:
        return (mod, name)
    if name in m.imports:
        src, attr = m.imports[name]
        if attr is None:
            return (src, "") if src in mods else None
        return lookup(mods, src, attr, depth + 1)
    if f"{mod}.{name}" in mods:
        return (f"{mod}.{name}", "")
    return None


def class_bases(mods, mod: str, cls: str) -> list[tuple[str, str]]:
    out, todo, seen = [], [(mod, cls)], set()
    while todo:
        mo, c = todo.pop(0)
        if (mo, c) in seen or mo not in mods or c not in mods[mo].classes:
            continue
        seen.add((mo, c))
        out.append((mo, c))
        for b in mods[mo].classes[c].bases:
            if isinstance(b, ast.Name):
                r = lookup(mods, mo, b.id)
                if r and r[1]:
                    todo.append(r)
            elif isinstance(b, ast.Attribute) and isinstance(b.value, ast.Name):
                r = lookup(mods, mo, b.value.id)
                if r and not r[1]:
                    r2 = lookup(mods, r[0], b.attr)
                    if r2 and r2[1]:
                        todo.append(r2)
    return out


def method_of(mods, mod: str, cls: str, meth: str, skip_self: bool = False) -> str | None:
    chain = class_bases(mods, mod, cls)
    if skip_self:
        chain = chain[1:]
    for mo, c in chain:
        for n in mods[mo].classes[c].body:
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name == meth:
                return f"{mo}:{c}.{meth}"
    return None


def edges(mods: dict[str, Module]):
    for m in mods.values():
        def visit(node, owner: str, cls: str | None, top_cls: str | None):
            for child in ast.iter_child_nodes(node):
                o, c = owner, cls
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)) and owner == "<module>":
                    o = f"{top_cls}.{child.name}" if top_cls else child.name
                if isinstance(child, ast.ClassDef):
                    if owner == "<module>" and top_cls is None:
                        yield from visit(child, owner, child.name, child.name)
                        continue
                if isinstance(child, ast.Call):
                    callee = resolve_call(mods, m, child.func, cls)
                    if callee:
                        yield (f"{m.name}:{o}", callee[0], f"{os.path.relpath(m.path, ROOT)}:{child.lineno}", callee[1])
                yield from visit(child, o, c, top_cls)
        yield from visit(m.tree, "<module>", None, None)


def target_name(mods, mod: str, name: str) -> tuple[str, str] | None:
    r = lookup(mods, mod, name)
    if not r or not r[1]:
        return None
    mo, n = r
    if mods[mo].defs.get(n) == "class":
        init = method_of(mods, mo, n, "__init__")
        return (init or f"{mo}:{n}", "direct")
    return (f"{mo}:{n}", "direct")


def resolve_call(mods, m: Module, func: ast.expr, cls: str | None):
    if isinstance(func, ast.Name):
        return target_name(mods, m.name, func.id)
    if isinstance(func, ast.Attribute):
        v = func.value
        if isinstance(v, ast.Name) and v.id in ("self", "cls") and cls:
            meth = method_of(mods, m.name, cls, func.attr)
            return (meth, "direct") if meth else (f"?.{func.attr}", "unresolved")
        if (isinstance(v, ast.Call) and isinstance(v.func, ast.Name) and v.func.id == "super" and cls):
            meth = method_of(mods, m.name, cls, func.attr, skip_self=True)
            return (meth, "direct") if meth else (f"?.{func.attr}", "unresolved")
        if isinstance(v, ast.Name):
            r = lookup(mods, m.name, v.id)
            if r and not r[1]:  # a module
                return target_name(mods, r[0], func.attr)
            if r and mods[r[0]].defs.get(r[1]) == "class":
                meth = method_of(mods, r[0], r[1], func.attr)
                if meth:
                    return (meth, "direct")
            if v.id in m.imports and m.imports[v.id][0] not in mods and r is None:
                return None  # stdlib / third-party module attribute
        return (f"?.{func.attr}", "unresolved")
    return None


ROOT = "."

if __name__ == "__main__":
    ROOT = os.path.abspath(sys.argv[1])
    pkgs = sys.argv[2:] or ["fastapi"]
    mods = load(ROOT, pkgs)
    for e in sorted(set(edges(mods))):
        print("\t".join(e))
