# Docs to check for a code diff, without an LLM (#210)

As a fixed CI step, without an agent: which existing documentation pages must be looked at for a
code diff? Does myceliums' graph and search find more of them than a strong text search, at the same
review effort (pages shown)? Many real spring-framework commits answer it; no session is run.

**Status: protocol and dev only. Held-out is not scored**; it is scored once, after an independent
review of this directory. `prepare.py` and `evaluate.py` refuse held-out without `--i-have-the-review`,
and `evaluate.py` scores held-out once: it needs the committed [`data/protocol.lock.json`](data/protocol.lock.json) (hashes of the
protocol below, the commit lists, dev's results and the code) to match, takes `data/HELD_OUT_SCORED.json` before it reads anything of
held-out, and refuses when that marker or a result exists in the working tree or in any commit. Neither path is an argument.

## Protocol (24 November 2026, before any held-out result; amended the same day after the review of dev)

What was known when this was first written: the subjects and gold pages of all commits (the selection prints
them; no method was run on held-out), and on dev two rounds of the text methods (the BM25 and grep grids were
revised once after the first). No myceliums method had been scored; it got the same one revision.

### Amendments (24 November 2026, evening; after dev, before held-out, from the independent review)

| What | Before | Now | Why |
|---|---|---|---|
| Licence headers | in the diff and in samples | cut from the diffs of all methods, from the samples of the text corpus, and from the samples and queries of the search index | 32 of 65 dev diffs held one; samples filled 42 of 325 BM25 top-5 slots; the first dev numbers understated the text methods and are replaced |
| Contenders | best by dev recall@5 | best by **cross-validated** recall@5, for the search inside `myc-fusion` and for `text`, `myc` and `text+myc`; configurations are tuned on dev as before | selecting by the tuned score picked text-fusion over a better BM25 |
| Margin | 0.10 | **unchanged**; also reported whether an advantage of 0.10 is excluded (upper interval end below 0.10), and the difference with its interval whatever it is | a margin lowered after dev would be fitted to it |
| Bootstrap | over commits | over **clusters of commits that share an issue number** (cross-validation folds keep a cluster together, too) | 16 of the 84 held-out commits share an issue with another (11 pairs) |
| Comment-only commits | out | out of the docs set; a marked stratum of the false-alarm set | a Javadoc diff is a docs edit of its own |
| False alarms | not measured | 100 code commits without docs per period (below) | the step would run on 93 % of commits for nothing |
| Second scoring | refused by `--out` only | lock and marker (above) | another `--out` would have scored again |

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
| `keyword`, `hybrid` | `myc search`, keyword and with embeddings, over the docs: six query formulations (types; declarations and keys; rarest identifiers; weighted words; one query per type; per member) and six pairs of them fused, each with two aggregations of sections to pages | 24 + 24 |
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
percentile interval of a paired bootstrap over clusters (10,000, seed 210), p = twice the smaller tail share at 0, **Holm over the two**. **Clearly better:** difference at least 0.10,
interval above 0, Holm p at most 0.05. Reported for each whatever the outcome: the difference with its interval, and whether an advantage of 0.10 is excluded. All other numbers are described, not tested.

Expected conclusions, from dev's cross-validated pairs (numbers under *Result on dev*): at a true difference of 0.10 the point estimate is at least 0.10 in about half of the samples, so the power for "clearly better" is **about 50 %**; at a true difference of 0.2 it is near 1. "Not clearly better" is therefore **not** "no better": held-out can end with the advantage of 0.10 excluded (small advantages, close to 0), with a difference above 0 but under the margin, or with nothing excluded (wide interval), and the report says which. The gold-noise estimate sets no ceiling: the text finds 3 of the 4 pages of the noise sample that had another cause.

### False alarms

A CI step runs on every code commit, of which 6.5 % change a docs unit. [`data/alarms.json`](data/alarms.json): 100 code commits **without any change under `framework-docs/`** from the dev period and 100 from the held-out period (seed 210; same subject, size and main-code rules; one per subject; the parent has the split docs; held-out without #183's commits and neighbours), `comment_only` marked (12 and 8). Only `bm25` (the raw score of the best section, no fusion). A page is shown when it is among the best five and its score reaches a threshold, **tuned on dev** to lose at most 0.05 of recall@5 on the docs commits (largest score among the gold pages' scores that keeps it). Reported: alarm rate (a commit with a page shown) with Wilson interval, for all, code and comment-only commits, pages shown per 100 commits, and recall@5 at the threshold on the docs commits. Held-out is scored once, with the main result. The graph is run on these commits only if it wins the main comparison. 20 dev alarms are read for real omissions.

### Decision table

| Held-out | CI step |
|---|---|
| `text+myc` clearly better than `text` (and `myc` not at least as good) | text search **with** myceliums |
| `myc` clearly better than `text` (and `text+myc` not better) | **myceliums** |
| neither | **text search** (the frozen configuration; a few hundred lines of Python, no index) |
| the winner's recall@5 is below 0.50 | **none** |

A win for text is a result. Cost decides nothing in the table; it stands beside it (index time, embeddings, dependency).

### Changes during dev (all before any held-out result)

* Text: the grep and BM25 grids were replaced once after the first averages (BM25 sat at the edge of its grid; grep's code-span variant was worse everywhere).
* myceliums: the mentions and search grids were replaced once, after an interim run on the first 44-45 of the 65 dev commits (search: single formulations and two-formulation sets instead of `rrf`/`first`, which ranked like `max`; mentions: hop weights and a page-length exponent instead of the `declared` factor, which had no effect).
* Owners and members come from line containment: `MEMBER_OF` links the members of a class with a constructor to the constructor method (27,395 of 106,850 edges on one Spring parent), so an owner walk over it finds nothing.
* `search_code` (MCP) shortens paths, so the search uses `myc search`; both rank with the same library.
* Mentions: the graph of the parent and the graph of the parent with the non-docs diff are both built; before the first run the owner walk was wrong (fixed after two commits).

### Resources (measured on dev)

Graph-only index of Spring: 22 s unloaded, 50-100 s at a load of 60-130; two per commit, deleted after use (556 MB at once, one at a time); export and neighbourhood 5 s. Embedding the docs once (1,473 sections and documents): 801 s at a load of 60-130 (5 cores at
nice 15); per commit a docs index derived at the same path (`analyze --force`, all unchanged vectors reused, 0.4 s) and about 13 s of searches. Dev: 65 x ~100 s graph and 65 x ~15 s search. Held-out: 84 x the same, about 3 h. Embedding the whole repository would take about 5 h per first index (#183) and is **not done**.
Network: git only, 17,678 blobs in 91 s for the 65 parents; about the same again for held-out (two header-change commits).

## Result on dev (65 commits, not a held-out result)

Tuned on dev and scored on the same commits; the cross-validated column (5 folds, position-interleaved, every method re-tuned without the fold) is the less optimistic one. Configurations: [`data/dev.json`](data/dev.json).

| Method | recall@1 | recall@3 | **recall@5** | recall@10 | precision@5 | MRR | pages shown | recall@5, cross-validated |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| plain-grep (untuned) | 0.400 | 0.583 | **0.617** | 0.673 | 0.157 | 0.550 | 3.1 | |
| grep | 0.435 | 0.686 | **0.753** | 0.842 | 0.203 | 0.658 | 5.0 | 0.697 |
| bm25 | 0.509 | 0.715 | **0.859** | 0.867 | 0.231 | 0.738 | 5.0 | 0.841 |
| text-fusion (= text) | 0.458 | 0.783 | **0.863** | 0.890 | 0.234 | 0.705 | 5.0 | 0.815 |
| mentions (graph) | 0.450 | 0.635 | **0.733** | 0.756 | 0.200 | 0.645 | 4.3 | 0.683 |
| keyword search | 0.532 | 0.739 | **0.818** | 0.878 | 0.215 | 0.751 | 5.0 | 0.782 |
| hybrid search | 0.450 | 0.727 | **0.811** | 0.858 | 0.215 | 0.691 | 5.0 | 0.783 |
| myc-fusion (= myc) | 0.555 | 0.813 | **0.849** | 0.878 | 0.222 | 0.783 | 5.0 | 0.795 |
| text + mentions | 0.496 | 0.779 | **0.847** | 0.882 | 0.228 | 0.740 | 5.0 | 0.803 |
| text + myc-best (= text+myc) | 0.496 | 0.810 | **0.886** | 0.897 | 0.240 | 0.752 | 5.0 | 0.849 |

Seconds per commit (medians, machine load 30-130): text 0.3 (reading the diff and the docs) + 0.02; graph 64 (two indexes) + 6 (export, neighbourhood); search 16 (docs index derived, changed sections embedded) + 0.3 keyword or 0.8 hybrid. First embedding of the docs: 801 s.

**What myceliums adds over the best text method on dev:** +0.023 recall@5 (0.886 against 0.863; 2 commits better, none worse, 63 equal; +0.033 cross-validated, interval -0.005 to 0.085); `myc` alone is 0.014 below (4 worse, 1 better). Both far from the margin of 0.10; by the decision table dev says **text search**. It is a dev number: the tuned configurations are chosen on the same 65 commits.

Power, from dev's paired differences of `text+myc` - `text` (SD 0.14): 84 held-out commits give a 95 % half-width of about 0.03. A true difference of 0.02 cannot reach the margin; held-out would have to show a difference five times as large as dev.

### What the methods miss (examples, dev)

* **Nothing names the change:** `InputStreamResource` constructor (`e509385eae`): both gold pages (`responsebody.adoc`, `responseentity.adoc`) outside the top 100 of every method. Neither names the class or `Resource` at the parent (checked with grep): the connection is only in the reader's head.
* **Vocabulary gap:** `Handle STOMP messages to user destination in order` (`3277b0d6ac`): `ordered-messages.adoc` at rank 26 for the text, 21 for `myc`; the page is about the order of messages, the diff about a class named `OrderedMessageChannelDecorator`; no shared identifier.
* **Search finds what words do not:** `@Generated` on generated classes (`2eba3510f7`): `core/aot.adoc` at 20 for the text, 1 for `myc` and 2 for `text+myc`.
* **Graph signal lost in the fusion:** `@HttpExchange` on the server side (`d1d5b54f12`): the mentions rank the two pages 4 and 5, the text 23 and 32, the fusion 29 and 32.
* **Mentions reach one page, the text the others:** `LazyConnectionDataSourceProxy` (`3b4c7a8906`): no data-access page names the class at the parent; `connections.adoc` is found by mentions of other changed symbols, `hibernate.adoc` and `jpa.adoc` only by the text's words.
* **Gold noise in the misses:** `language-ref.adoc` (`906c54faff`, rank 93) is a title-only edit in the noise sample (`other`).
* **The graph skips listing blocks** (#180): a page that names a class only in code listings has no mention edge.

### Omitted pages (dev, weak estimate)

For the 20 commits of the noise sample, the pages outside gold that `text` or `text+myc` show first (90 pairs): the lines naming a changed type or member were read, not the whole page. None was clearly wrong after the change; three were undecided (`factory-extension.adoc` for e3aa5b6b11, `annotation-dynamicpropertysource.adoc` for 6cdb34410b, `ann-validation.adoc` for b068742ec8). Committers' omissions are therefore not shown to be large, but this is one reader and a skim; precision stays a lower bound.

### What dev cannot say

How the methods do on the other 93 % of code commits; anything about held-out; a private multi-repository code base (its docs, names and modules differ, and no such base was tested); the cost of keeping a graph index fresh in CI beyond the measured seconds.
