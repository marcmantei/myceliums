# Docs to check for a code diff, without an LLM (#210)

As a fixed CI step, without an agent: which existing documentation pages must be looked at for a
code diff? Does myceliums' graph and search find more of them than a strong text search, at the same
review effort (pages shown)? Many real spring-framework commits answer it; no session is run.

**Status: protocol and dev only. Held-out is not scored**; it is scored once, after an independent
review of this directory. `prepare.py` and `evaluate.py` refuse held-out without `--i-have-the-review`,
and `evaluate.py` refuses a second scoring.

## Protocol (24 November 2026, before any held-out result)

What was known when this was written: the subjects and gold pages of all commits (the selection prints
them; no method was run on held-out), and on dev two rounds of the text methods (the BM25 and grep grids were
revised once after the first). No myceliums method had been scored; it gets the same one revision.

### Commits

Rule ([`select_commits.py`](select_commits.py), list [`data/commits.json`](data/commits.json)), from paths,
subjects, dates and, for rule 6, whether code changes; never from a method's output:

| Step | Commits left |
|---|---:|
| 1. non-merge commits of `origin/main` (pin `9a156f878a`) since 2023-04-25, the week the docs were split into one file per page | 7,582 |
| 2. that change main code (`spring-*/**/src/main/**`) and a docs unit that exists at the parent | 341 |
| 3. subject not excluded by #183's rule (upgrade, polish, revert, format, release, ...) | 271 |
| 4. at most 40 files | 255 |
| 5. at most 10 docs units | 254 |
| 6. main code changes beyond comments, imports and white space | 174 |
| 7. same subject (forward port): the oldest | 172 |
| **dev**: committer date before 2024-06-01 | **65** |
| excluded: the replayed commits of #183 (20 tasks, 9 reserves) and their neighbours (same issue, or within 14 days and a shared main file) | 23 |
| **held-out**: from 2024-06-01 | **84** |

A **docs unit** is a file: an AsciiDoc page or partial (`framework-docs/modules/ROOT/{pages,partials}`) or
a sample (`framework-docs/src/main`); about 450 pages, and 18 to 500 samples depending on the date. **Gold:** the units
the commit modifies or deletes that exist at the parent; a moved file only when its content changes. Pages the
commit creates do not count (nobody can be sent to them). 100 % of gold is at the parent by construction.

Base rate: of 3,914 non-merge code commits in the range with at most 40 files and an accepted subject, 255 (6.5 %) change a docs unit.
A CI step runs on all of them; this benchmark scores only commits with gold, so **it says which pages, not
when to speak**. What the method shows on the other 93 % is not measured (decision 4 below).

### Gold noise (estimated on dev)

Read after the dev averages of the first text grids, before any per-commit ranking was looked at: a seeded sample of 20 dev commits, 31 gold pages
([`data/gold_noise.json`](data/gold_noise.json); one agent, not reviewed). For 20 pages the change makes the page
wrong or incomplete, for 7 the page documents the feature and the edit adds to it, for **4 (13 %; Wilson 95 % interval 5-29 %)
the edit has another cause** (wording, anchors, a documentation issue). So a perfect method cannot reach recall 1; the same noise
falls on every method. The other direction, pages the committer did not touch and should have, is estimated from the top-5 non-gold pages
on dev (*Result on dev*): precision is a lower bound.

### Input

Every method reads the diff of the files outside `framework-docs/` (main code, tests, build files) and the repository at the parent. The
commit message and the docs side of the commit are never read. Gold pages are in the docs at the parent by construction.

### Methods and budgets

| Method | What | Configurations |
|---|---|---:|
| `plain-grep` | untuned reference: occurrences of the changed types' names | 0 |
| `grep` | identifiers of the diff by role (types and members around the changed lines, also new and old names; declaration changed or not; accessors as properties; `Owner.member` and qualified names; annotation attributes; property keys and dotted or dashed literals; other identifiers of code and comments; tests), whole word, weighted by rarity in the docs, frequency and length | 48 |
| `bm25` | words of the same sources and of the diff's comments: camel-case parts, whole identifiers, stems, weighted; scored per section (the best section of a page), page path as a field | 48 |
| `text-fusion` | `grep` and `bm25` fused (RRF or normalised sum) | 24 |
| `mentions` | the graph: MENTIONS edges from docs sections to the changed symbols and their types; profiles add members, one hop (overridden and overriding methods, doc-comment references in both directions, name references), documentation samples that call or use the symbol, callees and callers | 48 |
| `keyword`, `hybrid` | `myc search`, keyword and with embeddings, over the docs: six query formulations (types; declarations and keys; rarest identifiers; weighted words; one query per type; per member), four aggregations of sections to pages | 24 + 24 |
| `myc-fusion` | `mentions` and the better search | 24 |
| `text+mentions`, `text+myc-best` | the best text method with `mentions`, with the best myceliums method | 24 + 24 |

Text 120, myceliums 120. The **contenders**: `text` = best of grep, BM25 and their fusion; `myc` = best of the myceliums methods; `text+myc` = the better of the two last rows. All
"best" are by dev recall@5 (ties: MRR, then grid order), chosen once and frozen in [`data/dev.json`](data/dev.json). Units are ranked by score, ties by path.

How the symbols are chosen is the same on both sides: the declaration scanner of `change.py` (types and members whose lines, doc comment included, a hunk touches) and the
graph's line ranges. **The graph is given more than a text search:** two indexes per commit, the parent and the parent with the non-docs diff applied
(new symbols; the docs stay the parent's); the neighbours of both are merged. `grep`, `bm25` and the fusions use no index.

myceliums search is run on an index of `framework-docs/` alone (no file filter exists; over the whole repository the first hits are code).
Hybrid uses jina-embeddings-v2-base-code, 1,473 vectors (all sections and documents) for the docs.

### Metrics and statistics

Recall@k and precision@k for k = 1, 3, 5, 10, MRR (first gold page), success@5 (any gold page), pages shown (a method may show fewer than 5), mean over commits; run time per commit and index cost. **Primary: recall@5.**

Two registered comparisons, paired over the 84 held-out commits: `text+myc` - `text` (what myceliums adds) and `myc` - `text`. Each: mean difference, 95 %
percentile interval of a paired bootstrap over commits (10,000, seed 210), p = twice the smaller tail share at 0, **Holm over the two**. **Clearly better:** difference >= 0.10,
interval above 0, Holm p <= 0.05. All other numbers are described, not tested.

Power, from dev's paired differences, is given under *Result on dev*. The ceiling matters: with the text at 0.86 on dev, +0.10 needs 0.96, about the noise limit of the gold (87 % of the gold pages have a cause in the code).
So "no clear win" is the likely outcome when the text stays at 0.86, and it decides for the text search by design.

### Decision table

| Held-out | CI step |
|---|---|
| `text+myc` clearly better than `text` (and `myc` not at least as good) | text search **with** myceliums |
| `myc` clearly better than `text` (and `text+myc` not better) | **myceliums** |
| neither | **text search** (the frozen configuration; a few hundred lines of Python, no index) |
| the winner's recall@5 is below 0.50 | **none** |

A win for text is a result. Cost decides nothing in the table; it stands beside it (index time, embeddings, dependency).

### Resources (measured on dev)

Graph-only index of Spring: 22 s unloaded, 50-100 s at a load of 60-130; two per commit, deleted after use (556 MB at once, one at a time); export and neighbourhood 5 s. Embedding the docs once (1,473 sections and documents): 801 s at a load of 60-130 (5 cores at
nice 15); per commit a docs index derived at the same path (`analyze --force`, all unchanged vectors reused, 0.4 s) and about 13 s of searches. Dev: 65 x ~100 s graph and 65 x ~15 s search. Held-out: 84 x the same, about 3 h. Embedding the whole repository would take about 5 h per first index (#183) and is **not done**.
Network: git only, 17,678 blobs in 91 s for the 65 parents; about the same again for held-out (two header-change commits).

### Choices open for a decision

1. The margin 0.10 near the ceiling (above); 0.05 would be easier to reach but not clearly a reason for an index.
2. Only commits with gold are scored; what the method shows on the 93 % of code commits without docs is the adoption question of a CI step that always runs. A cheap text-only check of a threshold, or a second sample of such commits (60 and 60, text and graph, about 4 h), would answer it.
3. Comment-only commits are out (rule 6: 80 of 254): a Javadoc diff is a docs edit; a check on it is a different step.
4. Samples are units of their own; 3 of 65 dev commits have one in the gold.
