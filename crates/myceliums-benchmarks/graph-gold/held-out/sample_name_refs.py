#!/usr/bin/env python3
"""Draw the precision samples of the name-reference edges (#179).

The population is every REFERENCES edge with `extraction_method: "name-ref"`
of a full analysis, dumped one JSON object per line by an ignored test of
`myceliums_core::name_refs`. The draw is deterministic: the edges are ordered
by their canonical JSON (keys sorted, compact), then `random.Random(179)`
samples each stratum in that order. Same code, same checkout: same sample.

Regenerate a dump (debug build; the data dir and HOME are throwaway):

    export MYCELIUMS_DATA_DIR=$(mktemp -d) HOME=$MYCELIUMS_DATA_DIR
    cargo run -p myc -- analyze <checkout> --skip-embeddings --skip-communities --force
    NAME_REFS_INDEX=$MYCELIUMS_DATA_DIR/data/<repo-id> NAME_REFS_REPO=<repo-id> \\
    NAME_REFS_OUT=spring.jsonl cargo test -p myceliums-core --lib \\
        name_refs::sampling_dump -- --ignored

(`<repo-id>` is the id `myc list` prints, e.g. `spring-framework-c15e4211`.)
Then:

    sample_name_refs.py spring spring.jsonl > spring-sample.json
    sample_name_refs.py django django.jsonl > django-sample.json

The items are labelled by hand (`correct`, `note`) into
`name-refs-samples.json`.
"""

import json
import random
import sys

SEED = 179


def canonical(edge):
    return json.dumps(edge, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def stratum(edge):
    if edge["origin"] == "string":
        return "string"
    return "xml" if edge["file"].endswith(".xml") else "other-config"


# Spring: 20 XML, 12 string-literal, 8 other config edges; Django: 15 at random.
PLANS = {
    "spring": [("xml", 20), ("string", 12), ("other-config", 8)],
    "django": [(None, 15)],
}


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in PLANS:
        sys.exit(f"usage: {sys.argv[0]} spring|django <dump.jsonl>")
    with open(sys.argv[2], encoding="utf-8") as f:
        edges = [json.loads(line) for line in f if line.strip()]
    edges.sort(key=canonical)
    rng = random.Random(SEED)
    sample = []
    for name, size in PLANS[sys.argv[1]]:
        pool = [e for e in edges if name is None or stratum(e) == name]
        for edge in rng.sample(pool, size):
            sample.append({"stratum": name or "all", **edge})
    counts = {}
    for e in edges:
        counts[stratum(e)] = counts.get(stratum(e), 0) + 1
    json.dump({"population": len(edges), "by_stratum": counts, "items": sample},
              sys.stdout, indent=1, ensure_ascii=False)
    print()


if __name__ == "__main__":
    main()
