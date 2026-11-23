"""Draw doc mentions from the spring-framework reference docs (#180).

The population is every mention-shaped token in the AsciiDoc pages of
`framework-docs/modules/` at the pinned commit, outside listing, literal,
comment and passthrough blocks:

- `code-span`: an inline code span (`` `Foo` ``, `` `+Foo+` ``), not the
  curved quotes `"`word`"`;
- `xref`: an `xref:page.adoc#anchor[label]` macro;
- `api-link`: a link into the Javadoc (`{spring-framework-api}/core/Foo.html#bar()[...]`)
  or a `javadoc:` macro. v7.1.0 writes no `javadoc:` macro (0 in the
  checkout); its API links are the equivalent and are drawn instead.

Two sets, two seeds, disjoint: the dev set (seed 180) and the held-out set
(`--heldout`, seed 4180). Each stratum gets a quota; the candidates are
shuffled with the seed and taken in order, skipping a (file, line, text)
already in the other set. Like `sample_java_sites.py`, the selection is a
seeded regex over `git ls-files`; it does not use myceliums.

    python3 -I crates/myceliums-benchmarks/graph-gold/held-out/sample_doc_mentions.py [--heldout] [checkout]

Prints `stratum<TAB>file<TAB>line<TAB>text<TAB>source line` per drawn item.
Default checkout: ~/.cache/myceliums-eval-repos/spring-framework (see
`fetch_java.sh`).
"""

import os
import random
import re
import subprocess
import sys

SEED = 180
HELDOUT_SEED = 4180
DEV_QUOTA = {"code-span": 15, "xref": 5, "api-link": 5}
HELDOUT_QUOTA = {"code-span": 30, "xref": 10, "api-link": 10}

BLOCK = re.compile(r"^(-{4,}|\.{4,}|/{4,}|\+{4,}|```.*)\s*$")
# `"`word`"` and `'`word`'` are AsciiDoc's curved quotes, not code.
CODE_SPAN = re.compile(r"(?<![`\w\\\"'])`(\+?)([^`\n]+?)\1`(?![`\w\"'])")
XREF = re.compile(r"xref:([^\[\s]+)\[([^\]]*)\]")
API_LINK = re.compile(r"\{spring-framework-api\}\+*(/[^\[\s]+?)\+*\[([^\]]*)\]|javadoc:([^\[\s]+)\[([^\]]*)\]")


def candidates(checkout):
    files = subprocess.run(
        ["git", "ls-files", "framework-docs/modules/*.adoc"],
        cwd=checkout, capture_output=True, text=True, check=True,
    ).stdout.split()
    out = {"code-span": [], "xref": [], "api-link": []}
    for path in sorted(files):
        with open(os.path.join(checkout, path), encoding="utf-8") as fh:
            lines = fh.read().split("\n")
        in_block = None
        for number, line in enumerate(lines, 1):
            stripped = line.strip()
            m = BLOCK.match(stripped)
            if m:
                fence = m.group(1)[:3]
                if in_block is None:
                    in_block = fence
                elif in_block == fence:
                    in_block = None
                continue
            if in_block or stripped.startswith("//") or stripped.startswith(":"):
                continue
            if stripped.startswith(("include::", "include-code::")):
                continue
            for m in CODE_SPAN.finditer(line):
                out["code-span"].append((path, number, m.group(2), line))
            for m in XREF.finditer(line):
                out["xref"].append((path, number, m.group(0), line))
            for m in API_LINK.finditer(line):
                out["api-link"].append((path, number, m.group(0), line))
    return out


def draw(pool, seed, quota, exclude):
    rng = random.Random(seed)
    drawn = []
    for stratum in ("code-span", "xref", "api-link"):
        items = list(pool[stratum])
        rng.shuffle(items)
        taken = 0
        for item in items:
            if taken == quota[stratum]:
                break
            if item[:3] in exclude:
                continue
            drawn.append((stratum,) + item)
            taken += 1
    return drawn


def main():
    args = [a for a in sys.argv[1:] if a != "--heldout"]
    heldout = "--heldout" in sys.argv[1:]
    checkout = args[0] if args else os.path.expanduser(
        "~/.cache/myceliums-eval-repos/spring-framework")
    pool = candidates(checkout)
    dev = draw(pool, SEED, DEV_QUOTA, set())
    if heldout:
        chosen = draw(pool, HELDOUT_SEED, HELDOUT_QUOTA, {d[1:4] for d in dev})
    else:
        chosen = dev
    sizes = ", ".join(f"{k}: {len(v)}" for k, v in pool.items())
    print(f"# population: {sizes}", file=sys.stderr)
    for stratum, path, line, text, source in chosen:
        print(f"{stratum}\t{path}\t{line}\t{text}\t{source.strip()[:200]}")


if __name__ == "__main__":
    main()
