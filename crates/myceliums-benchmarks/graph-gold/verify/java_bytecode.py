"""Cross-check the Java gold set against javac's output, not against myceliums.

    python3 -I crates/myceliums-benchmarks/graph-gold/verify/java_bytecode.py

Compiles the fixture into a temporary directory and reads `javap -c -p`:

1. Symbols: every class, method and constructor in the bytecode must be a
   gold symbol (by class, name and arity) and every gold symbol must exist.
   Exempt: lambda bodies, members a record or a class without a declared
   constructor gets implicitly.
2. Calls: every invoke instruction names its resolved target. Each call
   between fixture methods must be a gold edge (primary or also_accept) and
   every gold edge must be seen. Lambda bodies belong to their enclosing
   method; an inherited method referenced through a subclass is looked up in
   the superclass; a call to an implicit constructor counts as a call to its
   class. Method references (invokedynamic) are not calls.

Exits non-zero on any disagreement.
"""

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parents[3] / "tests" / "fixtures" / "graph-accuracy" / "java"
GOLD = json.loads((HERE.parent / "java.json").read_text())


def arity(params):
    """Number of parameters in a javap parameter list (generic-aware)."""
    params, depth, count = params.strip(), 0, 0
    if not params:
        return 0
    for ch in params:
        depth += {"<": 1, ">": -1}.get(ch, 0)
        count += ch == "," and depth == 0
    return count + 1


def descriptor_arity(desc):
    inner = desc[1 : desc.index(")")]
    return len(re.findall(r"\[*(?:L[^;]+;|[BCDFIJSZ])", inner))


def main():
    failures = []
    gold_ids = {s["id"] for s in GOLD["symbols"]}

    def gold_id(cls, name, n):
        """Gold id for member `name` with `n` parameters of JVM class `cls`."""
        file = "src/" + cls.split("$")[0] + ".java"
        qual = cls.split("/")[-1].replace("$", ".")
        if name == "<init>":
            name = qual.split(".")[-1]
        base = f"{file}::{qual}.{name}"
        if base in gold_ids:
            return base
        overloads = [g for g in gold_ids if g.startswith(base + "(")]
        for g in overloads:
            if arity(g[len(base) + 1 : -1]) == n:
                return g
        return None

    with tempfile.TemporaryDirectory() as out:
        sources = sorted(str(p) for p in FIXTURE.rglob("*.java"))
        subprocess.run(["javac", "-Xlint:all", "-Werror", "-d", out, *sources], check=True)
        classes = sorted(p for p in Path(out).rglob("*.class"))
        listing = subprocess.run(
            ["javap", "-c", "-p", *map(str, classes)], capture_output=True, text=True, check=True
        ).stdout

    supers, members, calls = {}, [], []
    cls = record = None
    caller = None
    for line in listing.splitlines():
        head = re.match(r"(?:\w+ )*(class|interface) ([\w.$]+)(?: extends ([\w.$]+))?", line)
        if head and not line.startswith(" "):
            cls = head.group(2).replace(".", "/")
            record = head.group(3) == "java.lang.Record"
            if head.group(3) and not head.group(3).startswith("java."):
                supers[cls] = head.group(3).replace(".", "/")
            members.append((cls, None, 0, False))
            continue
        member = re.match(r"  (?:[\w.<>\[\], ?$]+ )*?([\w.$]+)\((.*)\)(?: throws [\w., ]+)?;$", line)
        if member:
            name, n = member.group(1), arity(member.group(2))
            if name == cls.replace("/", "."):
                name = "<init>"
            lam = re.match(r"lambda\$(\w+)\$\d+", name)
            if lam:
                outer = "<init>" if lam.group(1) == "new" else lam.group(1)
                caller = next((g for g in [gold_id(cls, outer, k) for k in range(8)] if g), None)
                continue
            implicit = record and name in ("<init>", "toString", "hashCode", "equals", "price")
            members.append((cls, name, n, implicit))
            caller = gold_id(cls, name, n)
            continue
        invoke = re.search(r"invoke(?:static|virtual|special|interface)\s.*// (?:Interface)?Method (?:([\w/$]+)\.)?(\"<init>\"|\w+):(\([^)]*\)\S+)", line)
        if invoke and caller:
            target_cls = invoke.group(1) or cls
            name = invoke.group(2).strip('"')
            calls.append((caller, target_cls, name, descriptor_arity(invoke.group(3))))

    for c, name, n, implicit in members:
        if name is None:
            sid = f"src/{c.split('$')[0]}.java::{c.split('/')[-1].replace('$', '.')}"
            if sid not in gold_ids:
                failures.append(f"class missing from gold: {sid}")
            continue
        if implicit:
            continue
        if gold_id(c, name, n) is None:
            simple = c.split("/")[-1].split("$")[-1]
            source = (FIXTURE / "src" / (c.split("$")[0] + ".java")).read_text()
            declared = re.search(rf"^\s*(?:(?:public|protected|private) )?{simple}\s*\(", source, re.M)
            if name == "<init>" and n == 0 and not declared:
                continue  # implicit default constructor
            failures.append(f"member missing from gold: {c}.{name}/{n}")
    present = set()
    for c, name, n, implicit in members:
        if name is None:
            present.add(f"src/{c.split('$')[0]}.java::{c.split('/')[-1].replace('$', '.')}")
        elif not implicit:
            present.add(gold_id(c, name, n))
    for sid in sorted(gold_ids - present):
        failures.append(f"gold symbol not in bytecode: {sid}")

    observed = set()
    for caller, target_cls, name, n in calls:
        if target_cls.startswith("java/"):
            continue
        callee, c = None, target_cls
        while c and callee is None:
            callee = gold_id(c, name, n)
            c = supers.get(c)
        if callee is None and name == "<init>":
            # An implicit constructor: the call instantiates the class.
            sid = f"src/{target_cls.split('$')[0]}.java::{target_cls.split('/')[-1].replace('$', '.')}"
            callee = sid if sid in gold_ids else None
        if callee:
            observed.add((caller, callee))

    accepted = {}
    for i, edge in enumerate(GOLD["calls"]):
        for target in [edge["to"], *edge.get("also_accept", [])]:
            accepted[(edge["from"], target)] = i
    dynamic = {(d["from"], d["to"]) for d in GOLD.get("dynamic", [])}
    seen = set()
    for pair in sorted(observed):
        if pair in dynamic:
            continue
        if pair in accepted:
            seen.add(accepted[pair])
        else:
            failures.append(f"bytecode call not in gold: {pair[0]} -> {pair[1]}")
    for i, edge in enumerate(GOLD["calls"]):
        if i not in seen:
            failures.append(f"gold edge not in bytecode: {edge['from']} -> {edge['to']}")

    for line in failures:
        print(line)
    print(f"{len(gold_ids)} gold symbols, {len(GOLD['calls'])} gold edges, {len(failures)} disagreements")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
