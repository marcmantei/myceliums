"""Cross-check the Rust gold set against rustc, not against myceliums.

    python3 -I crates/myceliums-benchmarks/graph-gold/verify/rust_mir.py

1. Symbols: every fn, struct, enum, trait, type alias and trait/impl method in
   the (rustfmt-formatted) fixture must be in the gold set at the same line,
   and nothing else.
2. Calls: rustc's MIR (`-Zunpretty=mir`, enabled on stable through
   RUSTC_BOOTSTRAP) names the resolved target of every call. Each call
   between fixture functions must be a gold edge (primary or also_accept),
   and every gold edge must be seen. Closures belong to their enclosing fn.

Exits non-zero on any disagreement. rustc writes nothing to the tree.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parents[3] / "tests" / "fixtures" / "graph-accuracy" / "rust"
GOLD = json.loads((HERE.parent / "rust.json").read_text())


def source_symbols():
    """`{id: line}` from the source, by rustfmt's indentation."""
    found = {}
    for path in sorted((FIXTURE / "src").glob("*.rs")):
        rel = path.relative_to(FIXTURE).as_posix()
        owner = None
        for number, line in enumerate(path.read_text().splitlines(), 1):
            top = re.match(r"(?:pub )?(fn|struct|enum|trait|type) (\w+)", line)
            if top:
                found[f"{rel}::{top.group(2)}"] = number
                owner = top.group(2) if top.group(1) == "trait" else None
                continue
            impl = re.match(r"impl(?:<[^>]*>)? (?:\w+ for )?(\w+)", line)
            if impl:
                owner = impl.group(1)
                continue
            method = re.match(r"    (?:pub )?fn (\w+)", line)
            if method and owner:
                found[f"{rel}::{owner}.{method.group(1)}"] = number
            if line == "}":
                owner = None
    return found


def strip_generics(text):
    out, depth = [], 0
    i = 0
    while i < len(text):
        if text.startswith("::<", i) and depth == 0 and not text.startswith("::<impl at", i):
            depth, i = 1, i + 3
            continue
        if depth:
            depth += {"<": 1, ">": -1}.get(text[i], 0)
        else:
            out.append(text[i])
        i += 1
    return "".join(out)


def main():
    failures = []
    gold_lines = {s["id"]: s["line"] for s in GOLD["symbols"]}
    found = source_symbols()
    for sid, line in sorted(found.items()):
        if sid not in gold_lines:
            failures.append(f"symbol missing from gold: {sid} (line {line})")
        elif gold_lines[sid] != line:
            failures.append(f"symbol line differs: {sid} gold {gold_lines[sid]} source {line}")
    for sid in sorted(set(gold_lines) - set(found)):
        failures.append(f"gold symbol not in source: {sid}")

    types = {}  # type or trait name -> file
    free = {}  # free fn name -> [ids]
    for sid in found:
        file, qual = sid.split("::")
        if "." not in qual:
            free.setdefault(qual, []).append(sid)
            types[qual] = file

    mir = subprocess.run(
        ["rustc", "--edition", "2021", "-Zunpretty=mir", "src/main.rs"],
        cwd=FIXTURE, env={**os.environ, "RUSTC_BOOTSTRAP": "1"},
        capture_output=True, text=True, check=True,
    ).stdout

    def function(path):
        """Gold id for a MIR path to a fixture fn, or None."""
        path = strip_generics(re.sub(r"::\{closure#\d+\}", "", path))
        impl = re.search(r"<impl at ([\w/]+\.rs):(\d+):\d+: [^>]*>::(\w+)$", path)
        if impl:
            file, line, name = impl.groups()
            text = (FIXTURE / file).read_text().splitlines()[int(line) - 1]
            owner = re.match(r"impl(?:<[^>]*>)? (?:\w+ for )?(\w+)", text)
            return f"{file}::{owner.group(1)}.{name}" if owner else None
        dyn = re.match(r"<dyn [\w:]+ as (?:\w+::)*(\w+)>::(\w+)$", path)
        if dyn:
            trait, name = dyn.groups()
            return f"{types[trait]}::{trait}.{name}" if trait in types else None
        if path.startswith("<"):
            return None  # <T as ForeignTrait>::method
        parts = path.split("::")
        if len(parts) >= 2 and parts[-2] in types:
            sid = f"{types[parts[-2]]}::{parts[-2]}.{parts[-1]}"
            return sid if sid in found else None
        if len(parts) >= 2 and (FIXTURE / "src" / f"{parts[-2]}.rs").exists():
            sid = f"src/{parts[-2]}.rs::{parts[-1]}"
            return sid if sid in found else None
        if len(parts) == 1 and len(free.get(parts[0], [])) == 1:
            return free[parts[0]][0]
        return None

    observed = set()
    caller = None
    for line in mir.splitlines():
        header = re.match(r"fn (.+?)\(", line)
        if header:
            caller = function(header.group(1))
            continue
        call = re.search(r"= (.+?)\(.*-> \[return", line)
        if call and caller:
            callee = function(call.group(1))
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
            failures.append(f"MIR call not in gold: {pair[0]} -> {pair[1]}")
    for i, edge in enumerate(GOLD["calls"]):
        if i not in seen:
            failures.append(f"gold edge not in MIR: {edge['from']} -> {edge['to']}")

    for line in failures:
        print(line)
    print(f"{len(found)} symbols, {len(GOLD['calls'])} gold edges, {len(failures)} disagreements")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
