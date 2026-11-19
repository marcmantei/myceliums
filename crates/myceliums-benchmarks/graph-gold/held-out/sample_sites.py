"""Draw call sites for the held-out real-repository gold set (#139).

Picks call-looking tokens (`name(`) uniformly at random from the tracked
source files of each pinned repository, with a fixed seed, so the selection
does not depend on myceliums. The labeller then reads each site in order and
skips the ones that are not calls made inside a named function (definitions,
strings, comments, module-level code) until a repository has its quota.

    python3 -I crates/myceliums-benchmarks/graph-gold/held-out/sample_sites.py [repos-dir]

Prints `repo<TAB>file<TAB>line<TAB>callee<TAB>source line` per candidate.
Reads the checkouts only (default: ~/.cache/myceliums-eval-repos, fetched by
real-repos/fetch.sh at the pinned commits).
"""

import os
import random
import re
import subprocess
import sys

SEED = 139
CANDIDATES_PER_REPO = 40
# Extensions, and paths left out. Hono's tests are `describe`/`it` callbacks
# at module level: no call in them has a named caller, so its sites come from
# the library sources. (Hono is drawn last, so this leaves the other three
# samples unchanged.)
REPOS = {
    "flask": ((".py",), None),
    "requests": ((".py",), None),
    "gin": ((".go",), None),
    "hono": ((".ts", ".tsx"), re.compile(r"\.test\.tsx?$|(^|/)test/|^perf-measures/|^benchmarks/")),
}
NOT_CALLS = {
    "if", "elif", "for", "while", "switch", "return", "catch", "function", "func",
    "def", "class", "typeof", "with", "assert", "yield", "except", "and", "or",
    "not", "in", "lambda", "super", "import", "from", "await", "sizeof", "case",
}
CALL = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(")
DEFINITION = re.compile(
    r"^\s*(async\s+)?(def|class|func|function)\b"
    r"|^\s*(public |private |protected |static |async |get |set |readonly )*"
    r"[A-Za-z_$][\w$]*\s*(<[^>]*>)?\([^)]*\)\s*(:\s*[^={]+)?\{\s*$"
)


def candidates(root, repo, extensions, excluded):
    files = subprocess.run(
        ["git", "-C", os.path.join(root, repo), "ls-files"],
        capture_output=True, text=True, check=True,
    ).stdout.split()
    sites = []
    for path in sorted(
        f
        for f in files
        if f.endswith(extensions)
        and not f.endswith(".d.ts")
        and not (excluded and excluded.search(f))
    ):
        with open(os.path.join(root, repo, path), encoding="utf-8", errors="replace") as source:
            for number, line in enumerate(source, 1):
                stripped = line.strip()
                if not stripped or stripped.startswith(("#", "//", "*", "/*")) or DEFINITION.match(line):
                    continue
                for match in CALL.finditer(line):
                    if match.group(1) not in NOT_CALLS:
                        sites.append((path, number, match.group(1), stripped))
    return sites


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.cache/myceliums-eval-repos")
    rng = random.Random(SEED)
    for repo, (extensions, excluded) in REPOS.items():
        sites = candidates(root, repo, extensions, excluded)
        for path, number, callee, text in rng.sample(sites, CANDIDATES_PER_REPO):
            print(f"{repo}\t{path}\t{number}\t{callee}\t{text[:120]}")


if __name__ == "__main__":
    main()
