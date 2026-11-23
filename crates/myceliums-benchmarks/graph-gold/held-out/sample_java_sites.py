"""Draw call sites for the held-out Java CALLS set (#177).

Like `sample_sites.py`, a seeded regex over `git ls-files` of the pinned
checkout picks the candidates, so the selection does not depend on myceliums.
Two kinds of strata:

- `random`: call-looking tokens (`name(`) drawn uniformly from the production
  sources (`<module>/src/main/java/`, the reference-doc samples left out).
- one targeted stratum per hard Java case, each with its own candidate rule
  (below). The rules only propose sites; the labeller reads each candidate in
  order, keeps it when it is a call made inside a named method and really is
  the case, and skips it otherwise, until the stratum has its quota.

    python3 -I crates/myceliums-benchmarks/graph-gold/held-out/sample_java_sites.py [checkout]

Prints `stratum<TAB>file<TAB>line<TAB>callee<TAB>source line` per candidate.
Default checkout: ~/.cache/myceliums-eval-repos/spring-framework (see
`fetch_java.sh`).
"""

import os
import random
import re
import subprocess
import sys

SEED = 177
RANDOM_CANDIDATES = 80
CASE_CANDIDATES = 30

NOT_CALLS = {
    "if", "for", "while", "switch", "return", "catch", "synchronized", "super",
    "this", "new", "throw", "assert", "try", "else", "case", "yield", "do",
}
CALL = re.compile(r"(?<![\w$])([A-Za-z_][A-Za-z0-9_]*)\s*\(")
# A method, constructor or class declaration line: not a call site.
DECLARATION = re.compile(
    r"^\s*(@\w+(\([^)]*\))?\s+)*((public|protected|private|static|final|abstract|synchronized|"
    r"native|default|strictfp|sealed|non-sealed)\s+)*"
    r"(<[^>]+>\s+)?([\w.$]+(<.*>)?(\[\])*\s+)?[A-Za-z_$][\w$]*\s*\([^;]*$"
    r"|^\s*((public|protected|private|static|final|abstract|sealed)\s+)*(class|interface|enum|record|@interface)\b"
)
MAIN = re.compile(r"^(?!framework-docs/)[^/]+/src/main/java/")
METHOD_DECL = re.compile(
    r"^\s*((public|protected|private|static|final|abstract|synchronized|native|default)\s+)*"
    r"(<[^>]+>\s+)?[\w.$]+(<.*>)?(\[\])*\s+([A-Za-z_$][\w$]*)\s*\("
)
TYPE_DECL = re.compile(r"\b(abstract\s+class|interface)\s+([A-Z][\w$]*)")
IMPORT_STATIC = re.compile(r"^import\s+static\s+([\w.]+)\.([a-zA-Z_][\w$]*)\s*;")
EXTENDS = re.compile(r"^(public\s+|abstract\s+|final\s+)*class\s+\w+(<.*?>)?\s+extends\s+([\w.]+)")
IMPORT = re.compile(r"^import\s+([\w.]+)\.([A-Z][\w$]*)\s*;")
# java.lang types a class commonly extends without an import.
JAVA_LANG = {"Thread", "Exception", "RuntimeException", "Error", "Throwable", "Number",
             "ClassLoader", "IllegalArgumentException", "IllegalStateException",
             "SecurityManager", "Enum", "Record"}
INJECT = re.compile(r"@(Autowired|Inject|Resource|MockitoBean|MockitoSpyBean)\b")
FIELD = re.compile(r"^\s*((private|protected|public|final|static)\s+)*([\w.$]+(<.*>)?)\s+(\w+)\s*(=[^;]*)?;")
STEREOTYPE = re.compile(r"^@(Component|Service|Repository|Controller|RestController)\b")
FINAL_FIELD = re.compile(r"^\s*private\s+final\s+([\w.$]+(<.*>)?)\s+(\w+)\s*;")
ANONYMOUS = re.compile(r"\bnew\s+[\w.$]+(<[^()]*>)?\s*\([^;]*\)\s*\{\s*$")
NESTED = re.compile(r"^\s+((public|protected|private|static|final|abstract)\s+)*(class|enum|record)\s+\w+")
GENERIC_RESULT = re.compile(
    r"\.(get|getFirst|getLast|next|orElse|orElseThrow|poll|peek|pop|getValue|getKey|getObject)"
    r"\([^()]*\)\.([a-zA-Z_]\w*)\s*\("
)
TYPE_PARAM_BOUND = re.compile(r"<\s*([A-Z])\s+extends\s+([\w.]+)")
# A declared variable, field or parameter: its type's simple name and its name.
VARIABLE = re.compile(r"(?<![\w.$])([A-Z][\w$]*)(<[^;=()]*>)?\s+([a-z]\w*)\s*[;=,)]")
METHOD_REF = re.compile(r"(?<![\w$])([\w$]+|this|super)::([a-zA-Z_][\w$]*)")


def code_lines(text):
    """(number, line) of the lines that are not blank or comments."""
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if stripped and not stripped.startswith(("//", "*", "/*", "import ", "package ")):
            yield number, line


def calls_in(line):
    """(callee, column) of the call-looking tokens of a line."""
    if DECLARATION.match(line):
        return []
    return [(m.group(1), m.start()) for m in CALL.finditer(line) if m.group(1) not in NOT_CALLS]


def block_lines(lines, start):
    """Line numbers of the brace block opened on `lines[start]`."""
    depth = 0
    out = []
    for i in range(start, len(lines)):
        code = re.sub(r'"([^"\\]|\\.)*"', '""', lines[i])
        depth += code.count("{") - code.count("}")
        if i > start:
            out.append(i + 1)
        if depth <= 0 and i > start:
            break
    return out


def strata(root):
    files = sorted(
        f for f in subprocess.run(
            ["git", "-C", root, "ls-files"], capture_output=True, text=True, check=True
        ).stdout.split()
        if f.endswith(".java")
    )
    texts = {}
    for path in files:
        with open(os.path.join(root, path), encoding="utf-8", errors="replace") as source:
            texts[path] = source.read()
    main = [f for f in files if MAIN.match(f)]
    abstract_types = {m.group(2) for f in main for m in TYPE_DECL.finditer(texts[f])}

    out = {name: [] for name in (
        "random", "dispatch", "overload", "generics", "inner-anonymous", "lambda",
        "method-ref", "static-import", "injected-field", "inherited-library",
    )}
    for path in main:
        text = texts[path]
        lines = text.splitlines()
        declared = [m.group(6) for line in lines if (m := METHOD_DECL.match(line))]
        overloaded = {n for n in declared if declared.count(n) > 1}
        variables = [(m.group(1), m.group(3)) for m in VARIABLE.finditer(text)]
        typed_vars = {name for ty, name in variables if ty in abstract_types}
        bounds = {m.group(1) for m in TYPE_PARAM_BOUND.finditer(text)}
        bound_vars = {name for ty, name in variables if ty in bounds}
        extends = next((m.group(3) for line in lines if (m := EXTENDS.match(line))), None)
        imports = {m.group(2): m.group(1) for line in lines if (m := IMPORT.match(line))}
        library_base = extends is not None and (
            (extends in imports and not imports[extends].startswith("org.springframework"))
            or (extends not in imports and extends in JAVA_LANG)
        )
        nested = set()
        for i, line in enumerate(lines):
            if ANONYMOUS.search(line) or NESTED.match(line):
                nested.update(block_lines(lines, i))
        for number, line in code_lines(text):
            calls = calls_in(line)
            stripped = line.strip()
            for callee, column in calls:
                out["random"].append((path, number, callee, stripped))
                if callee in overloaded and re.search(rf"(?<![\w.$]){callee}\s*\(|this\.{callee}\s*\(", line):
                    out["overload"].append((path, number, callee, stripped))
                if number in nested:
                    out["inner-anonymous"].append((path, number, callee, stripped))
                if "->" in line[:column]:
                    out["lambda"].append((path, number, callee, stripped))
                if library_base and re.search(rf"(?<![\w.$]){callee}\s*\(|super\.{callee}\s*\(", line) \
                        and callee not in declared and not re.search(rf"new\s+{callee}\s*\(", line):
                    out["inherited-library"].append((path, number, callee, stripped))
            for m in re.finditer(r"(?<![\w$.])(\w+)\.(\w+)\s*\(", line):
                if m.group(1) in typed_vars:
                    out["dispatch"].append((path, number, m.group(2), stripped))
                if m.group(1) in bound_vars:
                    out["generics"].append((path, number, m.group(2), stripped))
            for m in GENERIC_RESULT.finditer(line):
                out["generics"].append((path, number, m.group(2), stripped))
            for m in METHOD_REF.finditer(line):
                if m.group(2) != "new":
                    out["method-ref"].append((path, number, m.group(2), stripped))
    for path in files:
        text = texts[path]
        lines = text.splitlines()
        # The project's own static methods: a library one (assertThat, given)
        # is a trivial negative.
        statics = {
            m.group(2) for line in lines
            if (m := IMPORT_STATIC.match(line)) and m.group(1).startswith("org.springframework.")
        }
        injected = set()
        for i, line in enumerate(lines):
            if INJECT.search(line):
                for candidate in (line, lines[i + 1] if i + 1 < len(lines) else ""):
                    m = FIELD.match(INJECT.sub("", candidate))
                    if m and "(" not in candidate:
                        injected.add(m.group(5))
                        break
        if any(STEREOTYPE.match(line) for line in lines):
            injected.update(m.group(3) for line in lines if (m := FINAL_FIELD.match(line)))
        for number, line in code_lines(text):
            stripped = line.strip()
            if statics:
                for callee, _ in calls_in(line):
                    if callee in statics and re.search(rf"(?<![\w.$]){callee}\s*\(", line):
                        out["static-import"].append((path, number, callee, stripped))
            for m in re.finditer(r"(?<![\w$])(?:this\.)?(\w+)\.(\w+)\s*\(", line):
                if m.group(1) in injected:
                    out["injected-field"].append((path, number, m.group(2), stripped))
    return out


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
        "~/.cache/myceliums-eval-repos/spring-framework")
    rng = random.Random(SEED)
    for name, sites in strata(root).items():
        sites = sorted(set(sites))
        count = RANDOM_CANDIDATES if name == "random" else CASE_CANDIDATES
        for path, number, callee, text in rng.sample(sites, min(count, len(sites))):
            print(f"{name}\t{path}\t{number}\t{callee}\t{text[:140]}")


if __name__ == "__main__":
    main()
