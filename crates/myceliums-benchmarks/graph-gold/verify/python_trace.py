"""Cross-check the Python gold set against the interpreter, not against myceliums.

    python3 -I crates/myceliums-benchmarks/graph-gold/verify/python_trace.py

1. Symbols: every function, method and class `ast` finds in the fixture must
   be in the gold set, at the same line, and nothing else.
2. Calls: a driver exercises every function under `sys.setprofile`; each
   observed caller -> callee pair between fixture functions must be a gold
   edge (or one of its `also_accept` targets), and every gold edge must be
   observed. Constructor calls show up as calls to `__init__`.

Exits non-zero on any disagreement. Writes no files (bytecode caching is off).
"""

import ast
import io
import json
import sys
import types
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parents[3] / "tests" / "fixtures" / "graph-accuracy" / "python"
GOLD = json.loads((HERE.parent / "python.json").read_text())


def ast_symbols():
    found = {}
    for path in sorted(FIXTURE.rglob("*.py")):
        rel = path.relative_to(FIXTURE).as_posix()
        tree = ast.parse(path.read_text())

        def visit(node, prefix):
            for child in ast.iter_child_nodes(node):
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    qual = f"{prefix}{child.name}"
                    found[f"{rel}::{qual}"] = child.lineno
                    if isinstance(child, ast.ClassDef):
                        visit(child, qual + ".")

        visit(tree, "")
    return found


def code_id(code):
    path = Path(code.co_filename).resolve()
    try:
        rel = path.relative_to(FIXTURE).as_posix()
    except ValueError:
        return None
    return f"{rel}::{code.co_qualname}"


def enclosing(frame):
    """The innermost named fixture function a frame runs in."""
    while frame is not None and frame.f_code.co_name.startswith("<"):
        frame = frame.f_back
    return frame


def trace_calls():
    sys.path.insert(0, str(FIXTURE))
    from shop import auth, cli, formatting, models, reports, service, storage

    observed = set()

    def profile(frame, event, _arg):
        if event != "call" or frame.f_code.co_name.startswith("<"):
            return
        callee = code_id(frame.f_code)
        caller_frame = enclosing(frame.f_back)
        caller = code_id(caller_frame.f_code) if caller_frame else None
        if callee and caller:
            observed.add((caller, callee))

    item = types.SimpleNamespace(price=3)
    stdout, sys.stdout = sys.stdout, io.StringIO()  # cli.main prints
    sys.setprofile(profile)
    try:
        cli.main(["ann", "ann@example.com", item])
        repo = storage.Repository()
        users = service.UserService(repo)
        users.login("ann.read")
        auth.refresh({"expired": True})
        users.settings('{"Theme": "dark"}')
        users.register("bob", "bob@example.com")
        users.get(0)
        repo.find(lambda row: True)
        orders = service.OrderService(repo)
        orders.parse('{"Currency": "USD"}')
        user = models.User("cy", "cy@example.com")
        user.display_name()
        order = models.Order(user, [item])
        order.add(item)
        reports.OrderReport([order]).summary()
        reports.OrderReport.empty()
        reports.Report("plain").render([1])
        formatting.pad("x", 3)
    finally:
        sys.setprofile(None)
        sys.stdout = stdout
    return observed


def main():
    failures = []

    gold_symbols = {s["id"]: s["line"] for s in GOLD["symbols"]}
    found = ast_symbols()
    for sid, line in sorted(found.items()):
        if sid not in gold_symbols:
            failures.append(f"symbol missing from gold: {sid} (line {line})")
        elif gold_symbols[sid] != line:
            failures.append(f"symbol line differs: {sid} gold {gold_symbols[sid]} ast {line}")
    for sid in sorted(set(gold_symbols) - set(found)):
        failures.append(f"gold symbol not in source: {sid}")

    accepted = {}
    for i, edge in enumerate(GOLD["calls"]):
        for target in [edge["to"], *edge.get("also_accept", [])]:
            accepted[(edge["from"], target)] = i
            # A constructor call is observed as a call to __init__.
            accepted[(edge["from"], target + ".__init__")] = i
    # Runtime-only edges (calls through function values) are listed apart
    # from the static gold; observing them is expected.
    dynamic = {(d["from"], d["to"]) for d in GOLD.get("dynamic", [])}
    seen_edges = set()
    for pair in sorted(trace_calls()):
        if pair in dynamic:
            continue
        if pair in accepted:
            seen_edges.add(accepted[pair])
        else:
            failures.append(f"observed call not in gold: {pair[0]} -> {pair[1]}")
    for i, edge in enumerate(GOLD["calls"]):
        if i not in seen_edges:
            failures.append(f"gold edge never observed: {edge['from']} -> {edge['to']}")

    for line in failures:
        print(line)
    print(f"{len(found)} symbols, {len(GOLD['calls'])} gold edges, {len(failures)} disagreements")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
