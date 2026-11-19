# Retrieval-Quality Benchmark — Methodology

How Myceliums measures whether search actually finds the right code, and what
the resulting numbers do and do not mean.

The benchmark answers one question: **given a query an agent would realistically
type, does the engine return the symbol that answers it, and how near the top?**
Everything below exists to make that question answerable with a number instead
of an opinion — and to say how sure that number is.

Run it with:

```bash
# lexical only — offline, no model weights
cargo run -p myceliums-benchmarks --bin retrieval-eval
# all four modes — downloads the model weights once if they are not cached
cargo run -p myceliums-benchmarks --bin retrieval-eval -- --allow-model-download
# what CI runs: fail on a material regression whose 95% interval is below zero
cargo run -p myceliums-benchmarks --bin retrieval-eval -- --check-baseline
# re-record the baseline (every mode must run)
MYC_EVAL_MODELS=1 cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
```

## The golden dataset

`golden/queries.json` holds 51 labelled queries over the fixture corpus. Each
query carries the text an agent would type, the symbols a human judged relevant,
an intent category, and a rationale explaining the labelling decision.

**Labelling criteria.** Relevant symbols are actual symbol names from the fixture
projects, hand-selected to represent realistic agent queries. A symbol is
labelled relevant when it genuinely answers the query — the code you would want
to read first. Symbols that merely mention a query term are *not* relevant; a
`user` variable is not an answer to "look up a stored user by id".

Labels are data, not code, so that changing a judgement is a reviewable diff and
a movement in the numbers can always be traced to either a code change or a label
change — never to an invisible one.

**Symbol identity.** A label is the pair `(file, qualified_name)`, for example
`sample-py-project/services/user_service.py::UserService.get_user`. The engine's
own `CodeSymbol.uid` is a fresh UUID on every parse and therefore cannot identify
a labelled answer across runs. The `(file, qualified_name)` pair is stable for as
long as the fixture file is unchanged, which is exactly what a golden label needs.
A unit test asserts every label resolves to a real corpus symbol, so a renamed
fixture fails loudly instead of quietly reporting a recall regression.

**Case and identifier splitting.** Search tokenizes on word *and* identifier
boundaries and lowercases the result, so `format_name`, `formatName` and
`FormatName` are the same token sequence. Labels reflect this: a query for
`format_name` lists the Python, TypeScript *and* JavaScript definitions, because
all three genuinely answer it. Labelling only one language variant would score
the engine down for behaving correctly.

### Intent categories

Each query declares what it probes, so failures can be read by category rather
than one query at a time:

| Intent | Count | What it tests |
| --- | --- | --- |
| `exact-name` | 15 | The query names the symbol almost exactly (`formatName`). |
| `behavioural` | 14 | The query describes what the code does, not what it is called. |
| `paraphrase` | 12 | The query uses different words for the same idea. |
| `conceptual` | 10 | The query names a concept spread over several symbols. |

These four values are the whole vocabulary: **kebab-case, British spelling**,
matching the prose in this crate. The set is closed — the loader rejects
anything outside it, so `behavioral` or `exact_name` fails at load rather than
silently becoming a fifth category that no aggregate reports on.

Query ids follow the same convention: a descriptive slug prefixed by intent,
such as `behavioural-trim-whitespace`. Slugs rather than a numbered sequence,
because a slug survives insertion and retirement without renumbering — there is
no gap for a reader to interpret — and a query that scores badly names itself in
the report instead of sending the reader back to the dataset to look up what
`q37` was. `query_ids_describe_the_query` enforces both properties.

The spread is deliberate. A dataset of only exact-name lookups would report a
flattering number that says nothing about the queries agents actually struggle
with; the paraphrase and conceptual queries are where a lexical engine is
expected to do badly, and where a semantic leg should earn its keep.

## Metrics

Every metric is a pure function of a ranking and a relevant set — no clock, no
I/O, no model. Relevance is binary: a symbol is a labelled answer or it is not.

**recall@k** — of the symbols labelled relevant, the fraction in the top `k`:

```
recall@k = |relevant ∩ top-k retrieved| / |relevant|
```

**hit@k** — `1` when at least one labelled answer is in the top `k`, else `0`.
Unlike recall it does not penalise a query for carrying many labels, so it is
the fairer "did the agent get anything useful" number for broad queries.

**MRR@10** — the mean over queries of `1 / rank` of the *first* relevant hit,
counting only the top 10 (0 when nothing relevant is there). A correct answer
in position 1 scores 1.0, position 2 scores 0.5, position 4 scores 0.25. The
measure rewards putting the right symbol first, which is what an agent with a
small context window needs.

**nDCG@10** — normalised discounted cumulative gain with binary relevance:

```
DCG@10  = Σ_{i ≤ 10, result i relevant} 1 / log2(i + 1)
IDCG@10 = Σ_{i ≤ min(10, |relevant|)}  1 / log2(i + 1)
nDCG@10 = DCG@10 / IDCG@10
```

It rewards every answer's position, not just the first, so it moves when a
second or third answer climbs — which MRR cannot see.

**R-precision** — precision at rank `R = |relevant|`: the fraction of the first
`R` results that are answers. A one-label query scores it as hit@1; a five-label
query scores how many of the first five results are answers. It needs no
arbitrary cutoff, which makes it the fairest single number across queries with
different label counts.

Recall and hit rate are reported at k = 1, 5, 10. Every mode is asked for 20
results (`RANKING_DEPTH`): deep enough for every cutoff and for R-precision on
the query with the most labels (13), and equal to the default result count of
`myc search --hybrid` and the MCP `hybrid_search` tool. The evaluator refuses a
golden set whose largest label set would not fit.

**The empty relevant set.** Recall, nDCG and R-precision divide by
`|relevant|`, so they are undefined when nothing is labelled. They return `0.0`
for that case, chosen so a data-quality problem shows up as a visibly bad score
rather than a perfect one: returning `1.0` would read as "found everything" and
let an unlabelled query flatter every aggregate it appears in. The dataset
validator rejects unlabelled queries outright, so evaluation never reaches this
path; it exists to make the metrics total, and to fail conspicuously if the
validator is ever bypassed.

Aggregates are unweighted means over queries, so a mode cannot inflate its score
by doing well on the queries that happen to carry many labels. Every metric is
also reported per intent and per query.

**Reading recall@k when a query has many labels.** Recall is a fraction of *all*
labelled answers, so a query with 13 relevant symbols caps recall@10 at 0.77 by
construction. That ceiling is a property of the query, not a defect of the
engine. MRR, hit@k and nDCG are the honest headline numbers for "did it find
something useful"; recall@10 answers "did it find everything".

## Search modes

Every mode runs through the code that ships, with the shipped default
configuration (`[embedding]` defaults: `multilingual-e5-small`, reranker
`bge-reranker-v2-m3`). Nothing is re-implemented for the benchmark.

| Mode | What runs | Same as |
| --- | --- | --- |
| `lexical` | `myceliums_core::search_symbols` — BM25 over name, signature, content, metadata | `myc search` |
| `semantic` | `Embedder::embed_query` → `Store::vector_search` over the persisted vectors | `myc semantic-search`, MCP `semantic_search` |
| `hybrid` | `hybrid_search` — BM25 and the vector leg fused by RRF (k = 60, 100 vector candidates) | `myc search --hybrid`, MCP `hybrid_search` |
| `hybrid+rerank` | `hybrid_search` with limit 20, then `rerank_results` with the reranker the index recorded, cut to the top 20 | `myc search --hybrid --rerank`, MCP `hybrid_search` with `rerank` |

For the three model-backed modes the corpus is indexed the way `myc analyze`
indexes a repository: symbols are written to a scratch LanceDB store, embedded
with `Embedder::embed_symbols` (the production document template and token
budget), stored with `Store::store_embeddings`, and the embedding configuration
is recorded in the index meta. The query side then resolves its embedder from
that record with `embedder_for_index`, exactly as every query path does. The
reranker reorders the 20 results `--rerank` reorders by default, so the
measurement covers the pool users actually get.

Both BM25 rankings — the `lexical` mode and the BM25 leg of `hybrid` — rank
`store.get_symbols()`, the slice every product search path hands to BM25, in
the order the store returns it. BM25 and RRF break exact ties in that order, so
this is what makes tie-breaking match `myc search` and `myc search --hybrid`.
Without model weights the lexical mode reads the symbols back from a store
built without embeddings (as `--skip-embeddings` builds it); with them, from the
embedded index. `hybrid` vs `lexical` then differs only by the vector leg and
the fusion.

### Model weights are opt-in

The weights are a one-time download — about 0.5 GB for the embedding model and
2.2 GB for the default reranker. The benchmark fetches them only when asked:

```bash
cargo run -p myceliums-benchmarks --bin retrieval-eval -- --allow-model-download
MYC_EVAL_MODELS=1 cargo run -p myceliums-benchmarks --bin retrieval-eval
cargo run -p myceliums-benchmarks --bin retrieval-eval -- --fetch-models   # download only
```

Weights live in the fastembed cache: `FASTEMBED_CACHE_DIR`, else `HF_HOME`, else
`.fastembed_cache` in the working directory. Point both variables at one
directory outside the repository. Without the opt-in, a mode whose weights are
already cached runs; a mode whose weights are missing is reported `UNAVAILABLE`
with the reason and the opt-in, and asking it to rank anyway is an error.
`--update-baseline` refuses to record a baseline in which any mode is
unmeasured.

**Unavailable modes carry no numbers at all.** An absent measurement must never
be mistaken for a zero, so an unmeasured mode reports `status: "UNAVAILABLE"`, a
`reason`, and `null` for `metrics`, `by_intent` and `per_query` — never zeroes,
and never absent keys a reader might default to zero.

## Determinism

The lexical mode is offline and reproducible by construction: the corpus is
parsed from in-repo fixtures, files are visited in sorted order, BM25 scoring
and its stable sort are deterministic.

The model-backed modes add more sources of variation; each is handled:

- **Symbol ids.** The parser mints a random UUID per symbol per parse. The store
  upserts by uid and fusion matches its two legs by uid, so the corpus replaces
  it with a digest of file, qualified name, kind and start line — opaque like a
  UUID, identical on every run, and checked for uniqueness at load.
- **Symbol order.** Ties in BM25 and RRF break by input order, and the
  product's input order is whatever `store.get_symbols()` returns. The
  benchmark ranks that slice rather than the parse order (see above) and prints
  whether the two coincide.
- **Fusion ties.** RRF routinely produces exactly equal scores (BM25 rank 1 +
  vector rank 2 equals BM25 rank 2 + vector rank 1). `reciprocal_rank_fusion`
  used to collect candidates from a `HashMap`, so ties came back in per-process
  random order — in the product, not only here. It now keeps list order (BM25
  ranking first, then vector-only candidates), which the stable sort makes the
  tie-break.
- **Model arithmetic.** ONNX inference is deterministic on one machine with one
  runtime build, but not guaranteed to be bit-identical across CPU
  architectures. A baseline
  recorded on one platform and compared on another can differ where two
  candidates score within float noise of each other. The report records its
  `platform`, the gate notes a mismatch, and the interval-plus-minimal-effect
  rule below is what keeps such noise from failing a build.

Two consecutive runs on one machine produce byte-identical report JSON for all
four modes (verified by diffing two runs; `evaluation_is_deterministic` and
`corpus_uids_are_stable_and_unique` pin the lexical and corpus parts in unit
tests), and the CI run on linux-x86_64 reproduced the macos-aarch64 baseline
to the fourth decimal. Floats are rounded to four decimals so the JSON is
diffable, and a rounded `-0.0` is written as `0.0`.

## Comparing runs: paired bootstrap

With 51 queries one query flipping from a hit to a miss moves a mean by about
0.02, and the standard error of MRR is around ±0.06. A difference between two
numbers is therefore reported with a 95% confidence interval, never alone.

Both runs being compared score the *same* queries, so the comparison is paired:
for each query take `d_i = treatment_i − control_i`, resample the 51 queries
with replacement 10 000 times (fixed seed, in-crate SplitMix64 generator), and
take the 2.5th and 97.5th percentiles of the resampled mean differences
(percentile method, linear interpolation). The point estimate is the observed
mean difference. Pairing matters: scores vary far more between queries than
between two modes on the same query, and pairing cancels that variance.

The interval answers "would this difference survive a different sample of
queries like these?" It says nothing about queries unlike these — see the
limitations below.

Each report carries these intervals for five mode pairs (each model-backed mode
vs `lexical`, `hybrid` vs `semantic`, `hybrid+rerank` vs `hybrid`) on every
metric, and the gate computes the same interval for current vs baseline. Both
pair the queries in one fixed order (sorted query id), so the same per-query
data always yields the same interval.

**What "significant" means here.** Throughout this document, the README and
the CI summary, a difference is called *significant* when its 95% interval
excludes zero — nothing more. Each interval is computed on its own and **not
corrected for multiple comparisons**: a report computes 45 of them (5 pairs × 9
metrics) and the gate 36 (4 modes × 9 metrics). With no real differences at
all, about two of 45 would still exclude zero by chance, and the metrics are
strongly correlated, so several "significant" cells can share one cause. Read a
lone significant cell as a lead, not a finding.

## The CI gate

`.github/workflows/retrieval-quality.yml` scores all four modes on every push and
pull request with `retrieval-eval --check-baseline` and writes the tables to the
job summary. The model weights are cached by `actions/cache`, keyed by the
shipped model ids (`retrieval-eval --model-cache-key`); on a miss they are
downloaded with `--fetch-models` and saved immediately, so a failing gate does
not discard the download. The evaluation step itself has no download opt-in: if
the weights are still missing, the modes report `UNAVAILABLE` and the gate fails
loudly rather than silently skipping them.

The cache is *restored* on every run but *saved* only from pushes to `main`:
the repository's 10 GB cache budget cannot hold a 1.5 GB copy per pull request,
so a pull request that misses downloads the weights and discards them. The key
(`retrieval-eval --model-cache-key`) is the locked fastembed version plus the
HuggingFace repositories of both models, e.g.
`fastembed-4.9.1-intfloat--multilingual-e5-small-rozgo--bge-reranker-v2-m3`.
It does **not** pin a HuggingFace revision — fastembed downloads each
repository's `main` — so if a repository is re-uploaded under the same name, a
cache hit keeps serving the old files while a fresh download gets the new ones.

**Decision rule.** For each mode measured in the baseline and each metric, take
the paired bootstrap difference `current − baseline` (95% interval, per
comparison, uncorrected — see above). It is a **regression** only when

1. the whole 95% interval lies below zero (upper bound < 0), **and**
2. the point estimate is at least **0.02** below zero (`MIN_EFFECT`).

The build fails when a regression hits a **gated metric** — MRR@10, nDCG@10 or
recall@10 — in any mode. Every other difference, significant or not, is
reported and never fails the build.

Why both conditions: significance alone would fail on a consistent but
negligible shift (nDCG moving by 0.005 across many queries); a size threshold
alone would fail on a single query flipping, which on 51 queries is
indistinguishable from noise. 0.02 is about one query of 51 going from a perfect
score to nothing (1/51 ≈ 0.0196) — less than that is not worth blocking a PR
on. With this rule a drop needs roughly four queries moving the same way before
it can fail.

Why only three gated metrics: recall@1/5, hit@k and R-precision are close
relatives of the gated three, so gating them too would count one ranking change
against the build several times over. Three metrics × four modes is still twelve
one-sided tests at 2.5% each; they are strongly correlated, and the lexical
mode is fully deterministic, so a change that does not move rankings can never
fail. A failure is evidence to look at, not a verdict.

The gate also fails — as a broken comparison, not a regression — when the
baseline belongs to a different golden set (dataset digest mismatch), when a
mode the baseline measured did not run, or when the baseline cannot be read.

**Accepting an intended drop.** Re-record the baseline in the same PR and say
why in the PR description. That is a reviewed decision, which is the point:
the gate stops *unnoticed* regressions, not deliberate trade-offs.

The graph-accuracy gate (`graph-eval --check-baseline`, same workflow) applies
the same rule — 95% interval wholly below zero and a drop of at least 0.02 —
to overall CALLS precision and recall, with a paired bootstrap over gold and
extracted edges instead of queries; see `graph-gold/README.md`.

## Fixture coverage

The corpus is the multi-file `sample-*-project` fixtures under `tests/fixtures`,
96 retrievable symbols in total:

| Fixture | Language | Queries touching it | Labels |
| --- | --- | --- | --- |
| `sample-ts-project` | TypeScript | 31 | 42 |
| `sample-js-project` | JavaScript | 24 | 39 |
| `sample-py-project` | Python | 20 | 24 |
| `sample-go-project` | Go | 14 | 20 |
| `sample-csharp-project` | C# | 11 | 14 |

Single-file grammar fixtures (`rust/`, `java/`, `kotlin/`, `ruby/`, `php/`, `c/`,
`cpp/`, `swift/`, `edge-cases/`) are deliberately excluded: they exist to test
the parser, and their trivially unique symbol names would inflate retrieval
scores without telling us anything about relevance.

Every configured project must contribute at least one symbol, or loading fails.
A renamed or emptied fixture would otherwise shrink the corpus and move every
reported metric with no visible cause — precisely the silent regression this
benchmark exists to catch. Content files (Markdown, JSON) are skipped because
they hold prose, not retrievable symbols.

Queries average 2.7 labelled answers each.

## Baseline

`golden/baseline.json` records the reference scores of all four modes — means,
per-intent means, every metric for every query, and the mode-vs-mode intervals —
plus the provenance needed to tell what they were measured against:

- `dataset_digest` — SHA-256 of `golden/queries.json` exactly as it ships. This
  is the binding guarantee: `baseline_matches_the_current_dataset_digest` fails
  if the labels changed after the baseline was recorded, and the CI gate refuses
  to compare across digests. It is content-addressed on purpose: every pull
  request lands via squash merge, so the commit a baseline was recorded on does
  not survive the merge (see `eval/baseline.rs` for the full history).
- `dataset_version` — the hand-maintained label version; must match too.
- `models` — embedding model and weights repository, dimension, index
  fingerprint, embedding-text version, reranker, rerank pool and RRF constant.
- `platform` — `os-arch` the numbers were produced on.
- `commit_sha` and `timestamp` — human breadcrumbs, not gates. A `-dirty`
  suffix means the run measured uncommitted code.

`baseline_measures_every_mode_query_by_query` fails if the committed baseline
lacks a mode, per-query scores, per-intent means or model provenance — a
baseline recorded without weights would otherwise quietly stop gating the
model-backed modes.

**The file is generated output; never hand-edit it.** Re-record when a scoring,
model or label change is *intended*, and commit the regenerated file in the
same change as its cause:

```bash
MYC_EVAL_MODELS=1 cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
```

The file records that command in its own `regenerate_with` field.

> **On the dates.** `dataset_version` and the baseline `timestamp` are the real
> dates these artefacts were produced, taken from the repository's own clock.
> `dataset_version` is a *content* version, not a release date: its job is to
> make two scores comparable only when they were measured against the same
> labels.

### Recorded results

51 queries over 96 symbols, recorded on `macos-aarch64` with
`multilingual-e5-small` and `bge-reranker-v2-m3` (embedding-text version 1):

| Mode | MRR@10 | nDCG@10 | recall@10 | hit@1 | hit@10 | R-precision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `lexical` | 0.6405 | 0.6255 | 0.6983 | 0.5294 | 0.8235 | 0.5242 |
| `semantic` | 0.8027 | 0.7896 | 0.8422 | 0.7647 | 0.9412 | 0.6877 |
| `hybrid` | 0.7713 | 0.7435 | 0.8102 | 0.7255 | 0.9412 | 0.6419 |
| `hybrid+rerank` | 0.8106 | 0.7773 | 0.8330 | 0.7647 | 0.9216 | 0.6783 |

recall@1 / recall@5 / hit@5 are in the baseline file. The lexical row is
unchanged from the previous baseline (MRR 0.6405, recall@10 0.6983): adding the
other modes moved no lexical number.

Differences, paired bootstrap 95% CI (10 000 resamples, fixed seed). **Bold**
intervals exclude zero — per comparison, uncorrected for the 45 intervals the
run computes (see "What significant means here"):

| Comparison | Δ MRR@10 | Δ nDCG@10 | Δ recall@10 |
| --- | ---: | ---: | ---: |
| semantic − lexical | **+0.162 [+0.070, +0.258]** | **+0.164 [+0.095, +0.241]** | **+0.144 [+0.073, +0.222]** |
| hybrid − lexical | **+0.131 [+0.056, +0.211]** | **+0.118 [+0.069, +0.174]** | **+0.112 [+0.056, +0.177]** |
| hybrid+rerank − lexical | **+0.170 [+0.065, +0.278]** | **+0.152 [+0.068, +0.240]** | **+0.135 [+0.060, +0.218]** |
| hybrid − semantic | −0.031 [−0.094, +0.025] | **−0.046 [−0.095, −0.007]** | −0.032 [−0.084, +0.016] |
| hybrid+rerank − hybrid | +0.039 [−0.043, +0.127] | +0.034 [−0.032, +0.106] | +0.023 [−0.025, +0.078] |

By intent, MRR@10 / nDCG@10:

| Intent (queries) | `lexical` | `semantic` | `hybrid` | `hybrid+rerank` |
| --- | ---: | ---: | ---: | ---: |
| `exact-name` (15) | 0.933 / 0.929 | 0.947 / 0.952 | 0.933 / 0.933 | 0.889 / 0.900 |
| `paraphrase` (12) | 0.396 / 0.436 | 0.662 / 0.687 | 0.573 / 0.616 | 0.715 / 0.777 |
| `behavioural` (14) | 0.589 / 0.569 | 0.871 / 0.802 | 0.839 / 0.743 | 0.871 / 0.769 |
| `conceptual` (10) | 0.567 / 0.478 | 0.659 / 0.652 | 0.671 / 0.613 | 0.723 / 0.606 |

Per-intent cells rest on 10–15 queries each; read them as where the modes
differ, not as estimates precise to the second decimal.

### What the numbers say — and do not

- **The semantic leg earns its keep.** Every model-backed mode beats BM25 by
  0.11–0.17 on MRR, nDCG and recall@10, with intervals well clear of zero. The
  gain sits where it was predicted: paraphrase and behavioural queries, where
  BM25 has no shared tokens to match. On exact-name queries all four modes are
  within a query or two of each other.
- **Hybrid does not beat semantic search on this set.** Fusing BM25 into the
  vector ranking costs 0.03–0.05 on MRR, nDCG@10 and recall@10; only the
  nDCG@10 interval excludes zero (uncorrected, and its upper end is −0.007). The loss is concentrated in
  paraphrase queries (MRR 0.66 → 0.57), where BM25 has little to contribute,
  while exact-name queries — where BM25 should help — were already solved by
  the vector leg. A plausible reading is that RRF's equal weighting lets the
  weaker ranking pull good vector results down; it has not been tested.
- **Reranking is not measurably better than hybrid.** +0.04 MRR with an
  interval from −0.04 to +0.13: it helps paraphrase queries and hurts a few
  exact-name ones (`exact-name-database-findbyid` drops from rank 1 to rank 3),
  and 51 queries cannot tell the net effect from zero.
- **This set is too small and too synthetic to choose a default from.** 96
  symbols in five toy projects, one labeller. The ranking above says the
  semantic leg matters; it does not say semantic-only should replace hybrid on
  real repositories, where identifiers are long, domain-specific and far more
  numerous. Nothing here was tuned on this set, and nothing should be: RRF `k`,
  leg weights, the rerank pool and model choice belong on a separate
  development split, with this set kept for the final measurement. That split
  now exists — see [the real-repository set](#real-repository-held-out-set-95).

### The exact-name failure

`exact-name-userservice-get-user` searches `"UserService get_user"` and BM25
returns no labelled answer in the top 10. Naming the class repeats the token
`user`, which lifts bare `user` variables and the `UserService` class itself
above the getter that was actually asked for. The vector leg barely helps:
`semantic` ranks a labelled getter fifth, and `hybrid` and `hybrid+rerank` still
miss. Qualifying a query with its class makes retrieval *worse*, which is the
opposite of what anyone typing it would expect; the query stays in the set so
that a fix shows up as a win.

## Real-repository held-out set (#95)

The golden set above is 51 queries over 96 symbols in five toy fixtures,
labelled by the same automation that wrote the search code. It is a good
regression guard and a poor place to choose parameters: anything tuned on it
would be tuned to it. RRF `k = 60` and the 100 vector candidates of hybrid
search were asserted, never measured. This section describes the set built to
measure them, how they were tuned, and what came out.

```bash
crates/myceliums-benchmarks/real-repos/fetch.sh          # check out the pinned repositories
cargo run -p myceliums-benchmarks --bin real-repo-eval    # score, sweep on dev, confirm on held-out
cargo run -p myceliums-benchmarks --bin real-repo-review -- --reviewer <handle>   # review the labels
```

### The set

168 queries over four open-source repositories, each pinned to the commit of
a release and fetched by `real-repos/fetch.sh` into a cache directory
(`$MYC_EVAL_REPOS_DIR`, default `~/.cache/myceliums-eval-repos`) — never
vendored. The evaluator refuses a checkout at any other commit.

| Repository | Language | Release · commit | Symbols | Queries |
| --- | --- | --- | ---: | ---: |
| psf/requests | Python | `v2.34.2` · `6e83187b8f` | 801 | 42 |
| pallets/flask | Python | `3.1.3` · `22d924701a` | 1,574 | 42 |
| gin-gonic/gin | Go | `v1.12.0` · `73726dc606` | 1,898 | 42 |
| honojs/hono | TypeScript | `v4.13.18` · `f6fbaf0299` | 10,420 | 42 |

Each repository has 9 `exact-name`, 10 `paraphrase`, 14 `behavioural` and 9
`conceptual` queries (the golden set's closed vocabulary). Each repository is
indexed on its own, as `myc analyze` indexes one repository; vector search is
exact at every size (see the ANN section), so hono's 10,420 symbols are
scanned exactly like the smaller repositories.

**Corpus.** `Corpus::load_repository` walks a checkout with the analyzer's
discovery rules (hidden directories, `node_modules`, `target`, `dist`,
`build`, virtualenvs and `__pycache__` skipped; symlinks not followed) and its
pre-parse filter `should_skip_file` under the default `[analysis]` settings.
Tests and examples stay in: they are what an agent's search competes with in a
real repository. As in the golden set, Markdown and other prose files are not
corpus — the labels are code symbols. A ranking that returns two symbols with
the same `(file, qualified_name)` (overloads, a getter and its setter) scores
the second as a miss in its slot, so one answer never counts twice.

**Labels.** Every label is a `(file, qualified_name)` pair the parser
produces, with a `why` naming what in that symbol's code answers the query,
plus a per-query `rationale` saying what was deliberately left out. They were
drafted by an AI agent reading the code at the pinned commit — never from
myceliums search output, which would have let the engine grade itself — and
are marked `"labelled_by": "agent:claude"`, `"reviewed_by": null`. **No human
has reviewed them yet; the set is not authoritative until one has**
(`real-repo-review` prints each query with its labels' source and records the
reviewer). The labelling rules applied across all four repositories — notably
"one implementation in several files: label every copy" and "thin wrappers
only when the query names them" — are in
[`real-repos/README.md`](../real-repos/README.md).

### Development and held-out split

`RealRepoSet::split` puts each query in `dev` or `held-out` deterministically:
within each (repository, intent) stratum, queries are ordered by SHA-256 of a
fixed salt and their id, and the first `round(0.6 × n)` are dev. That gives 96
dev and 72 held-out queries, 24 + 18 per repository, every stratum split as
close to 60/40 as its size allows. The assignment depends on the ids only — not
on file order, machine or run — and adding a query to a stratum moves at most
one other across the boundary (`adding_a_query_moves_at_most_one_other`). The
report records the full assignment.

Parameters are chosen on dev only. Held-out is read to confirm or reject the
dev choice.

### Sweep protocol

Fixed before any number was seen:

- **Grid** — every combination of RRF `k` ∈ {10, 20, 40, 60, 100} and minimum
  vector candidates ∈ {50, 100, 200, 400}: 20 configurations, the shipped
  `k = 60`, 100 candidates among them. Hybrid search exposes no other knob.
- **Selection** — the highest dev mean nDCG@10 wins; the shipped default is
  the incumbent, so a tie keeps it.
- **Confirmation** — the winner and the default are run on held-out through
  the production `hybrid_search_with` / `hybrid_search`, and the paired
  bootstrap difference (10,000 resamples, 95%) is taken on nDCG@10.
- **Decision** — the defaults change only if that held-out interval lies
  wholly above zero. Otherwise they stay, whatever dev said: the dev winner is
  the best of 20 tries on the queries it was picked on, and its dev margin is
  biased upward by exactly that.

Scoring 20 configurations per query by calling `hybrid_search_with` 20 times
would repeat the BM25 pass and the query embedding 20 times. The sweep instead
computes them once per query (`ModelIndex::fusion_inputs`: the production
`search_symbols`, `embed_query` and `Store::vector_search` with
`limit.max(candidates)`) and fuses with the production
`reciprocal_rank_fusion`. The evaluator checks on every dev query that this
reproduces the production `hybrid_search` scores exactly at the shipped
defaults, and fails the run otherwise. The held-out confirmation does not rely
on the shortcut.

Not swept: the rerank pool and the models; the vector index is #117's
subject (it is now an exact scan).

### CI

`.github/workflows/retrieval-quality-real-repos.yml` runs the whole thing
weekly and on `workflow_dispatch` — report-only, with the summary in the job
summary and the JSON report as an artifact. Indexing four repositories and
reranking 168 queries took 88 and 109 minutes in the two runs so far on a
hosted runner (reranking is most of it), far
too long for every pull request; the golden-set gate above stays the per-PR
check. The checkouts are cached under a key derived from the pins only, the
model weights share the golden-set job's cache, and the job embeds in batches
of 32 (`MYC_EVAL_EMBEDDING_BATCH`) because the shipped batch of 256 took the
16 GB runner down — batch size changes padding, not vectors beyond float noise.
The job fails only when it cannot measure honestly: a checkout at the wrong
commit, a label naming no corpus symbol, or a sweep that no longer reproduces
the production hybrid ranking.

### Recorded results — real repositories

Recorded on 2026-11-19 by the CI workflow (`linux-x86_64`, hosted runner),
dataset `2026-11-12` (digest `0eeb0ba91cf3…`), shipped models
`multilingual-e5-small` (embedding-text version 1) and `bge-reranker-v2-m3`
via fastembed 4.9.1, rerank pool 20, embedding batch 32. The full report is
[`real-repos/recorded-run.json`](../real-repos/recorded-run.json). It names
commit `e0feab8…`: this branch before its final rebase onto main's #136/#137,
which touch neither the search paths nor the parser output — all four
corpora parse to byte-identical symbol inventories after the rebase. Its
`-dirty` suffix came from the untracked `tee` log in the CI workspace, not from
uncommitted code; the evaluator now ignores untracked files there.

**One label has changed since.** Review feedback found a missing answer:
`requests-conceptual-session-cookie-persistence` (a dev query) now also labels
`SessionRedirectMixin.resolve_redirects`, which carries cookies across redirect
hops. The dataset moved to version `2026-11-19`; the run below was measured on
`2026-11-12` and has not been repeated, so that one dev query is scored
against seven labels instead of eight. The held-out numbers, and therefore
the decision, are unaffected. The next weekly run measures the current set.

**All labels are unreviewed agent drafts.** Read every number below as
provisional.

All 168 queries:

| Mode | MRR@10 | nDCG@10 | recall@10 | hit@1 | hit@10 | R-precision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `lexical` | 0.3834 | 0.3818 | 0.5050 | 0.2619 | 0.6250 | 0.2619 |
| `semantic` | 0.5025 | 0.4774 | 0.5922 | 0.4167 | 0.7083 | 0.3485 |
| `hybrid` | 0.4928 | 0.4818 | 0.6268 | 0.3750 | 0.7619 | 0.3325 |
| `hybrid+rerank` | 0.5491 | 0.5320 | 0.6397 | 0.4286 | 0.7679 | 0.3951 |

Held-out split only (72 queries):

| Mode | MRR@10 | nDCG@10 | recall@10 | hit@1 | hit@10 | R-precision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `lexical` | 0.4451 | 0.4388 | 0.5840 | 0.3056 | 0.7083 | 0.2807 |
| `semantic` | 0.5611 | 0.5481 | 0.6706 | 0.4722 | 0.7778 | 0.4082 |
| `hybrid` | 0.5884 | 0.5639 | 0.7065 | 0.4722 | 0.8611 | 0.3972 |
| `hybrid+rerank` | 0.6029 | 0.5737 | 0.6954 | 0.4444 | 0.8472 | 0.4093 |

Differences, paired bootstrap 95% CI, uncorrected; **bold** intervals exclude
zero:

| Comparison | Δ nDCG@10, all 168 | Δ MRR@10, all 168 | Δ nDCG@10, held-out 72 |
| --- | ---: | ---: | ---: |
| semantic − lexical | **+0.096 [+0.044, +0.146]** | **+0.119 [+0.058, +0.179]** | **+0.109 [+0.029, +0.189]** |
| hybrid − lexical | **+0.100 [+0.069, +0.132]** | **+0.109 [+0.073, +0.146]** | **+0.125 [+0.078, +0.171]** |
| hybrid+rerank − lexical | **+0.150 [+0.096, +0.204]** | **+0.166 [+0.101, +0.230]** | **+0.135 [+0.058, +0.208]** |
| hybrid − semantic | +0.004 [−0.033, +0.041] | −0.010 [−0.056, +0.036] | +0.016 [−0.043, +0.075] |
| hybrid+rerank − hybrid | **+0.050 [+0.008, +0.091]** | +0.056 [−0.000, +0.113] | +0.010 [−0.050, +0.068] |

By repository, nDCG@10 / MRR@10:

| Repository | `lexical` | `semantic` | `hybrid` | `hybrid+rerank` |
| --- | ---: | ---: | ---: | ---: |
| requests | 0.531 / 0.528 | 0.539 / 0.581 | 0.582 / 0.598 | 0.637 / 0.667 |
| flask | 0.483 / 0.469 | 0.596 / 0.611 | 0.608 / 0.609 | 0.647 / 0.631 |
| gin | 0.360 / 0.370 | 0.521 / 0.540 | 0.498 / 0.519 | 0.520 / 0.544 |
| hono | 0.153 / 0.167 | 0.254 / 0.278 | 0.240 / 0.245 | 0.324 / 0.355 |

By intent, MRR@10 / nDCG@10, all queries:

| Intent (queries) | `lexical` | `semantic` | `hybrid` | `hybrid+rerank` |
| --- | ---: | ---: | ---: | ---: |
| `exact-name` (36) | 0.386 / 0.451 | 0.617 / 0.670 | 0.523 / 0.589 | 0.720 / 0.751 |
| `paraphrase` (40) | 0.366 / 0.407 | 0.357 / 0.392 | 0.441 / 0.480 | 0.469 / 0.498 |
| `behavioural` (56) | 0.386 / 0.386 | 0.497 / 0.482 | 0.485 / 0.489 | 0.545 / 0.516 |
| `conceptual` (36) | 0.395 / 0.278 | 0.557 / 0.372 | 0.532 / 0.366 | 0.473 / 0.375 |

**Sweep on dev (96 queries).** The fused dev rankings reproduced the
production `hybrid_search` scores on all 96 dev queries at the shipped
defaults. Δ is against the shipped `k = 60`, 100 candidates on the same
queries:

| RRF k | 50 candidates | 100 candidates | 200 candidates | 400 candidates |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 0.4220 (+0.002) | 0.4218 (+0.002) | 0.4218 (+0.002) | 0.4219 (+0.002) |
| 20 | **0.4244 (+0.004)** | 0.4242 (+0.004) | 0.4195 (−0.001) | 0.4195 (−0.001) |
| 40 | 0.4235 (+0.003) | 0.4230 (+0.003) | 0.4216 (+0.001) | 0.4216 (+0.001) |
| 60 | 0.4209 (+0.001) | 0.4202 (shipped) | 0.4172 (−0.003) | 0.4172 (−0.003) |
| 100 | 0.4219 (+0.002) | 0.4201 (−0.000) | 0.4169 (−0.003) | 0.4169 (−0.003) |

Cells are dev nDCG@10. No dev difference has an interval that excludes zero
except `k = 60` with 50 candidates, whose +0.001 [+0.000, +0.002] is a single
query moving by a fraction of a rank.

**Held-out confirmation (72 queries).** The dev winner, `k = 20` with 50
candidates, against the shipped default, both through the production path:

| Metric | Δ (k = 20, 50 − shipped) [95% CI] |
| --- | ---: |
| nDCG@10 | −0.001 [−0.015, +0.014] |
| MRR@10 | −0.003 [−0.013, +0.007] |
| recall@10 | +0.007 [−0.023, +0.045] |
| hit@10 | −0.014 [−0.069, +0.028] |

**Decision: the defaults stay** — `k = 60` and 100 vector candidates. The dev
lead of +0.004 does not survive on held-out, where the difference is
indistinguishable from zero in both directions.

### What the numbers say — and do not

- **The parameters do not matter much on this set.** All 20 configurations
  sit within ±0.004 nDCG@10 of the default on dev. With a result list of 20,
  more than 100 vector candidates only adds deep vector-only symbols that
  rarely reach the top 20, and slightly hurt; fewer than 100 changed
  almost nothing. RRF `k` between 10 and 100 reorders a few near-ties. The
  values that ship were asserted, not tuned; they now are measured, and
  nothing measurably beats them.
- **On real repositories hybrid and semantic search are not distinguishable.**
  The golden set showed hybrid 0.046 nDCG@10 below semantic (interval
  excluding zero). Here the difference is +0.004 [−0.033, +0.041] overall and
  +0.016 [−0.043, +0.075] on held-out — intervals about ±0.04 and ±0.06 wide,
  so a real difference of that size either way is not ruled out. The two
  trade places by intent: fusion helps `paraphrase` queries (nDCG@10
  0.392 → 0.480) and costs `exact-name` queries (0.670 → 0.589). A plausible
  reading, not checked query by query: BM25 contributes where a paraphrase
  still shares a domain word with the identifier, and on exact names it lifts
  other symbols that repeat the name — tests, callers — into the fused list.
  Hybrid finds *something* more often (hit@10 0.762 vs 0.708, recall@10
  +0.035), semantic puts it first more often (hit@1 0.417 vs 0.375).
- **Everything model-backed beats BM25 by about 0.1 nDCG@10**, on all queries
  and on held-out alike — the one finding that carries over from the golden
  set unchanged.
- **Reranking helps on all queries, not provably on held-out.** +0.050
  [+0.008, +0.091] nDCG@10 over 168 queries, +0.010 [−0.050, +0.068] over the
  72 held-out ones; the gain is concentrated in `exact-name` queries (0.589 →
  0.751). The reranker was not tuned, so all 168 queries are a fair sample for
  it; the held-out interval is simply wider.
- **hono is hard.** 10,420 symbols, 8,503 of them in test, runtime-test and
  benchmark files (6,604 are local constants there); every mode scores
  0.15–0.36 nDCG@10 there against 0.48–0.65 on the Python repositories. An
  exploratory run before #135, when hono's vectors still went through the
  IVF-PQ index, gave semantic 0.314 nDCG@10 on hono against 0.254 with today's
  exact scan — 42 queries, unreviewed labels, one run each. Tracked in #141;
  not a finding.
- **Scores are lower than on the golden set across the board** (hybrid
  nDCG@10 0.48 vs 0.74). Real identifiers are numerous and repetitive, and
  tests and examples compete with the definitions. The golden-set numbers are
  a regression guard, not an estimate of real-repository quality.

### Limits of this set

- **Unreviewed labels**, drafted by one agent. Until a human reviews them,
  label errors and the labeller's reading of each query are inside every
  number. `real-repo-review` catches wrong labels; it cannot show a reviewer
  the answers that are *missing* (the cookie-persistence query above lacked
  one), so recall-type numbers may be biased in either direction.
- **The held-out split is no longer clean for future tuning.** It was read
  twice here: an exploratory CI run before the final rebase (IVF-PQ still
  active for hono, and a 60-configuration grid that also swept a BM25-leg
  weight of 0.25/0.5/1) picked `k = 20`, 50 candidates, weight 0.25 on dev and
  moved held-out nDCG@10 by +0.012 [−0.037, +0.061]; the weight was then
  dropped to keep the sweep to the knobs hybrid search has, and the protocol
  was re-run as recorded above. Neither look changed a default, but every look
  is a chance to fit the held-out queries. The next tuning question — fusion
  weights, the rerank pool, a model swap — needs fresh held-out queries (new
  queries, or new repositories), not another pass over these 72.
- **Intervals ignore clustering by repository.** The bootstrap resamples
  queries independently, but queries of one repository share a corpus, an
  index and a labeller's reading of that code, so they are not independent.
  With four repositories the honest intervals are wider than those reported
  — likely noticeably so; a cluster (repository-level) bootstrap with four
  clusters would be too coarse to help. Treat a bound near zero as zero.
- **Four repositories, three languages, English queries**; 18–24 queries per
  repository per split. Per-repository and per-intent cells rest on 9–56
  queries and are directions, not estimates.
- **Prose is excluded** from the corpus; an agent searching a real index also
  sees Markdown sections.
- **One platform.** The recorded run is `linux-x86_64`; model arithmetic can
  differ in the last bits elsewhere (see Determinism).


## ANN recall: what the vector index costs

Every semantic query and the vector leg of every hybrid query search the
`symbols` vector column. Since #117 that search is exact at every size — a flat
scan, which is also what the golden-set numbers above measure. Until #117 a
repository of 5,000 symbols or more got an approximate-nearest-neighbour (ANN)
index that every query went through. `ann-recall` measures what an index
setting costs against exact search, through the production `Store` API, and
what production search returns:

```bash
# Production (exact) search next to the pre-#117 index, synthetic tables:
cargo run -p myceliums-benchmarks --release --bin ann-recall
# The comparison of index settings (issue #117), synthetic or real embeddings:
cargo run -p myceliums-benchmarks --release --bin ann-recall -- --sweep --sizes 5000,20000,50000
cargo run -p myceliums-benchmarks --release --bin ann-recall -- --sweep \
    --repo <checkout> --cache <vectors.bin> --sizes 5000,20000
```

For each table the harness stores the vectors with indexing disabled, checks
that LanceDB's flat scan agrees with brute-force cosine exactly (it must, or
the harness is wrong), then for each setting builds the index with
`create_ann_index`, runs every query through `vector_search` with that
setting's query parameters, and compares the top 10 with the brute-force top
10. Recall is tie-tolerant (a returned row counts if its exact cosine is at
least the 10th best), because real code contains duplicate symbols with
identical vectors. Intervals are 95% bootstrap intervals over the 200 queries.
Latency is wall time of one `vector_search` call (median / 95th percentile)
after three warm-up queries.

**Data.**

- *Synthetic, clustered*: 384-dimensional unit vectors around 64 random
  centres (cosine ≈ 0.5 between cluster mates), seed 92, 200 held-out queries
  from the same distribution. *Uniform*: isotropic noise on the sphere, the
  worst case for any partition-based index.
- *Real*: Django 5.1.4 (`codeload.github.com/django/django/tar.gz/refs/tags/5.1.4`,
  SHA-256 `adbb3c67…5b97dda`), parsed with the shipped parser (39,997 code
  symbols) and embedded with the shipped default model (`multilingual-e5-small`,
  384-d) and document template, exactly as `myc analyze` embeds them. 200
  symbols, sampled with seed 117, are held out; their qualified names split
  into words (`QuerySet.get_or_create` → `query set get or create`) are the
  queries, embedded with the query prefix as a user's query is. The 5,000-
  and 20,000-row tables are prefixes of one seeded order of the remaining
  symbols. 50,000 rows was not measured on real data: Django has fewer
  symbols, and embedding a larger repository was not feasible on the shared
  measurement machine.

**Machine.** One Apple-silicon Mac (10 cores) shared with other build jobs.
The real-embedding and the final-setting rows below were measured with the
machine lightly loaded; the first synthetic sweep ran under heavy load (load
average above 150), so its latencies are inflated and noisy — compare
latencies within a table, not across them. Recall does not depend on load.

### Before: IVF-PQ from 5,000 rows

Until #117 the index was IVF-PQ (√n partitions, 16 one-byte PQ codes per
vector, LanceDB's default 20 probes, no refine step), built from 5,000 rows. It
returned 0.38 of the true top 10 on the clustered synthetic table (PR #116
measured 0.37) and 0.09 on the uniform one. On real embeddings it was no better:
0.42 at 5,000 rows and 0.40 at 20,000. The cause is two-fold, and the real data
shows the second half, which synthetic clusters hide: product quantisation
ranks on coarse distances (a refine step fixes most of that), and 20 probes
cover a shrinking share of √n partitions, so true neighbours in unprobed
partitions are never seen.

### Real embeddings (Django, multilingual-e5-small)

| Rows | Setting | recall@10 [95% CI] | top-1 | build s | latency ms p50 / p95 |
| ---: | --- | ---: | ---: | ---: | ---: |
| 5,000 | flat scan (exact) | 1.000 | 1.00 | — | 2.3 / 2.5 |
| 5,000 | IVF_PQ/16 (before) | 0.419 [0.392, 0.445] | 0.40 | 1.0 | 2.1 / 4.4 |
| 5,000 | IVF_PQ/16 refine=10 | 0.801 [0.772, 0.829] | 0.90 | 1.0 | 2.6 / 3.2 |
| 5,000 | IVF_PQ/16 refine=20 | 0.847 [0.823, 0.871] | 0.90 | 2.0 | 3.4 / 5.4 |
| 5,000 | IVF_PQ/16 nprobes=10 | 0.409 [0.384, 0.436] | 0.37 | 1.0 | 1.7 / 1.9 |
| 5,000 | IVF_PQ/16 nprobes=50 | 0.438 [0.413, 0.464] | 0.39 | 0.9 | 2.3 / 2.8 |
| 5,000 | IVF_PQ/16 nprobes=50 refine=10 | 0.863 [0.842, 0.884] | 0.94 | 0.8 | 3.0 / 3.4 |
| 5,000 | IVF_PQ/32 | 0.540 [0.516, 0.563] | 0.52 | 1.5 | 1.9 / 2.2 |
| 5,000 | IVF_PQ/48 | 0.626 [0.601, 0.650] | 0.59 | 2.3 | 2.0 / 2.3 |
| 5,000 | IVF_PQ/96 | 0.753 [0.732, 0.774] | 0.70 | 3.5 | 2.8 / 3.1 |
| 5,000 | IVF_PQ/96 refine=10 | 0.850 [0.824, 0.874] | 0.86 | 7.1 | 4.4 / 11.0 |
| 5,000 | IVF_FLAT | 0.841 [0.815, 0.865] | 0.87 | 0.2 | 2.0 / 11.8 |
| 5,000 | **IVF_FLAT nprobes=75%** | **0.988 [0.982, 0.993]** | 0.98 | 0.1 | 2.0 / 2.3 |
| 5,000 | IVF_HNSW_SQ ef=100 | 0.852 [0.826, 0.877] | 0.86 | 0.2 | 2.2 / 6.3 |
| 5,000 | IVF_HNSW_SQ nprobes=50% ef=100 | 0.921 [0.905, 0.936] | 0.90 | 0.2 | 2.0 / 2.7 |
| 5,000 | IVF_HNSW_SQ nprobes=75% ef=100 | 0.968 [0.960, 0.976] | 0.96 | 0.1 | 2.1 / 2.5 |
| 20,000 | flat scan (exact) | 1.000 | 1.00 | — | 5.2 / 5.7 |
| 20,000 | IVF_PQ/16 (before) | 0.399 [0.372, 0.427] | 0.30 | 7.4 | 2.7 / 3.8 |
| 20,000 | IVF_PQ/16 refine=10 | 0.724 [0.692, 0.755] | 0.85 | 5.6 | 3.4 / 4.1 |
| 20,000 | IVF_PQ/16 refine=20 | 0.800 [0.771, 0.828] | 0.87 | 5.5 | 3.9 / 4.3 |
| 20,000 | IVF_PQ/16 nprobes=50% refine=20 | 0.869 [0.848, 0.891] | 0.96 | 4.7 | 4.4 / 5.6 |
| 20,000 | IVF_PQ/96 | 0.728 [0.701, 0.754] | 0.67 | 22.0 | 2.9 / 3.4 |
| 20,000 | IVF_PQ/96 refine=10 | 0.824 [0.795, 0.852] | 0.87 | 15.0 | 3.7 / 4.1 |
| 20,000 | IVF_FLAT | 0.798 [0.767, 0.828] | 0.83 | 0.8 | 2.4 / 2.7 |
| 20,000 | IVF_FLAT nprobes=50% | 0.970 [0.960, 0.980] | 0.98 | 0.7 | 2.9 / 3.4 |
| 20,000 | **IVF_FLAT nprobes=75%** | **0.987 [0.980, 0.994]** | 0.99 | 0.6 | 3.3 / 3.7 |
| 20,000 | IVF_HNSW_SQ ef=100 | 0.809 [0.781, 0.836] | 0.81 | 0.7 | 2.5 / 3.7 |
| 20,000 | IVF_HNSW_SQ nprobes=50% ef=100 | 0.952 [0.939, 0.963] | 0.94 | 0.8 | 4.0 / 11.4 |
| 20,000 | IVF_HNSW_SQ nprobes=75% ef=100 | 0.972 [0.964, 0.979] | 0.95 | 0.7 | 3.5 / 4.0 |

`nprobes=N` probes N partitions; `nprobes=P%` probes P% of the √n partitions
(`Probes::Percent`), so 75% is 53 of 70 at 5,000 rows and 106 of 141 at 20,000.
`ef` is the HNSW candidate list (LanceDB default 1.5 × limit); `ef=200` measured
the same as `ef=100` (0.970 and 0.968). The full sweep, 18 more rows per size
(PQ 32/48 with refine, `nprobes=50`, HNSW without probes), is in the
`--sweep` output and behaves as the rows above: nothing reaches 0.95 without
probing a large share of the partitions.

### Synthetic, clustered

| Rows | Setting | recall@10 [95% CI] | latency ms p50 / p95 | flat ms p50 / p95 |
| ---: | --- | ---: | ---: | ---: |
| 5,000 | IVF_PQ/16 (before) | 0.368 [0.349, 0.387] | 2.1 / 9.3 | 5.6 / 25.5 |
| 5,000 | IVF_PQ/16 refine=10 | 1.000 | 2.6 / 6.5 | 5.6 / 25.5 |
| 5,000 | IVF_PQ/96 | 0.745 [0.730, 0.760] | 3.2 / 7.4 | 5.6 / 25.5 |
| 5,000 | IVF_FLAT | 1.000 | 2.0 / 4.5 | 5.6 / 25.5 |
| 5,000 | IVF_HNSW_SQ ef=100 | 0.977 [0.971, 0.983] | 1.8 / 2.6 | 5.6 / 25.5 |
| 20,000 | IVF_PQ/16 (before) | 0.224 [0.208, 0.240] | 3.0 / 5.0 | 12.0 / 37.2 |
| 20,000 | IVF_PQ/16 refine=20 | 0.960 [0.952, 0.969] | 13.4 / 53.3 | 12.0 / 37.2 |
| 20,000 | IVF_PQ/96 refine=10 | 1.000 [0.998, 1.000] | 21.3 / 63.3 | 12.0 / 37.2 |
| 50,000 | IVF_PQ/16 (before) | 0.147 [0.131, 0.162] | 35.1 / 87.4 | 31.1 / 87.9 |
| 50,000 | IVF_PQ/16 refine=20 | 0.765 [0.745, 0.784] | 6.2 / 40.9 | 31.1 / 87.9 |
| 50,000 | IVF_PQ/96 refine=10 | 0.997 [0.995, 0.999] | 31.2 / 100.7 | 31.1 / 87.9 |
| 50,000 | IVF_FLAT | 1.000 | 29.5 / 73.8 | 31.1 / 87.9 |
| 50,000 | IVF_HNSW_SQ ef=100 | 0.970 [0.964, 0.977] | 3.9 / 10.2 | 31.1 / 87.9 |

These rows come from the heavily loaded run (latencies inflated). On the
synthetic clusters a few partitions hold all true neighbours, so IVF probing
is not what limits recall there and quantisation is; on real embeddings the
opposite holds. That is why the first synthetic-only measurement pointed at
the refine step and the real data did not confirm it. The two best index
settings from the real-embedding table, re-measured on synthetic tables with
the machine lightly loaded:

| Data | Rows | Setting | recall@10 [95% CI] | top-1 | latency ms p50 / p95 | exact ms p50 / p95 |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| clustered | 5,000 | IVF_FLAT nprobes=75% | 1.000 | 1.00 | 3.4 / 8.9 | 3.8 / 9.8 |
| clustered | 20,000 | IVF_FLAT nprobes=75% | 1.000 | 1.00 | 3.9 / 7.8 | 6.1 / 10.7 |
| clustered | 50,000 | IVF_FLAT nprobes=75% | 1.000 | 1.00 | 5.0 / 7.0 | 9.3 / 11.5 |
| clustered | 100,000 | IVF_FLAT nprobes=75% | 1.000 | 1.00 | 6.5 / 7.6 | 15.7 / 21.6 |
| uniform | 5,000 | IVF_FLAT nprobes=75% | 0.917 [0.905, 0.929] | 0.91 | 2.3 / 3.5 | 2.9 / 3.5 |
| uniform | 5,000 | IVF_FLAT nprobes=50% | 0.726 [0.706, 0.746] | 0.79 | 2.2 / 2.6 | 2.9 / 3.5 |
| uniform | 100,000 | IVF_FLAT nprobes=75% | 0.908 [0.896, 0.921] | 0.91 | 13.3 / 25.6 | 15.4 / 18.6 |
| clustered | 100,000 | IVF_HNSW_SQ nprobes=75% ef=100 | 0.968 [0.961, 0.975] | 0.95 | 7.9 / 8.7 | 15.2 / 17.5 |
| uniform | 100,000 | IVF_HNSW_SQ nprobes=75% ef=100 | 0.891 [0.878, 0.903] | 0.87 | 8.9 / 10.8 | 15.5 / 18.8 |

### Choice: exact search at every size

Production builds no ANN index (`AnnSettings::PRODUCTION.threshold = None`).
Every vector search is an exact scan.

**Exact-search latency** (median of one `vector_search`, 384-d, one
Apple-silicon Mac, lightly loaded):

| Rows | Data | p50 ms | Runs |
| ---: | --- | ---: | --- |
| 5,000 | real (Django) | 2.4 | three runs, 2.3–2.5 |
| 20,000 | real (Django) | 5.2 | one lightly loaded run |
| 50,000 | synthetic | 9.5 | two runs, 9.3–9.6 |
| 100,000 | synthetic | 15 | four runs, 14.5–15.7 |

The cost is linear in the number of vectors — about 1.5 ms plus 0.135 ms per
1,000 vectors over these points, so a million symbols would be on the order of
150 ms per query (extrapolated, not measured). Vector data does not change the
cost of a scan, so synthetic rows stand in for real ones here.

**Why no index.** The best index measured, IVF_FLAT probing 75% of √n
partitions (≤ 256), scored recall@10 0.987–0.988 on real embeddings at 5,000
and 20,000 rows and cut a query at 100,000 synthetic rows from 15.7 to 6.5 ms.
In exchange it stores a second copy of every vector (about 1.5 KB per symbol),
runs a k-means build on every full analysis, loses about one true neighbour in
a hundred, and was never measured on real embeddings at the sizes where it
would apply. With partitions capped at 256 and 75% probed, its speed-up cannot
grow much beyond that either (the uniform table at 100,000 rows: 13.3 against
15.4 ms). IVF_HNSW_SQ with the same probes scored 0.97, and IVF-PQ never
reached 0.95 on real embeddings in any tested combination of refine factor,
probes and sub-vectors. Exact search is simpler and always right at the
latencies above.

**When to revisit.** If repositories of several hundred thousand symbols
become common, or exact search shows up as a measured bottleneck: build the
index for those sizes only (the machinery stays — set
`AnnSettings::threshold`; `index`/`query` in `PRODUCTION` hold the best
measured setting), measure its recall on real embeddings at that size with
`ann-recall --sweep --repo`, and check the trade-off against the table above.

**Existing indexes.** Indexes built by earlier releases do not change results
any more: `vector_search` bypasses any index when no threshold is set. The
next embedding write — `myc analyze`, or an incremental re-index — drops them
to reclaim the disk space. No fingerprint changes; nothing is re-embedded.

**CI check.** `production_search_is_exact_even_over_a_stale_index` runs on
every `cargo test`. For a clustered and a uniform synthetic table (2,000 rows,
100 queries, seed 92) it leaves a pre-#117 IVF-PQ index on the table, as an
upgrade does, then searches through production settings. Recall@10 against
brute force must be `CI_RECALL_FLOOR` = 1.0 (exact, tie-tolerant). The stale
index itself must score below 0.9 when searched with the old settings, so
the check can tell a bypass from an index search. `the_harness_checks_itself_on_a_small_table`
checks the harness itself.

## Known limitations

- **Fixture-scale corpus.** 96 symbols in five small projects. Larger real
  repositories behave differently — BM25 precision usually degrades as the
  corpus grows. Vector search stays exact at every size (see the ANN
  section), so only its latency grows. These numbers are a regression guard and a
  first comparison of modes, not a promise about real repositories.
- **No development split in the golden set.** Every golden query is in the
  one evaluation set; nothing is tuned on it. Tuning happens on the dev split
  of the real-repository set (#95), whose own limits are listed in its section.
- **Single labeller.** Labels reflect one reviewer's judgement, recorded in each
  query's `rationale`. There is no inter-annotator agreement measure.
- **English-only queries**, and five languages of the twenty the parser supports.
- **Concept queries are judgement calls.** Where a concept has no clean boundary
  ("persistence layer"), the labels take the narrowest defensible reading.
- **Model-mode numbers are per platform.** The baseline is recorded on one
  platform; CI runs on another. Differences within float noise can move a rank;
  the significance rule absorbs that, but the absolute numbers in a CI summary
  can differ slightly from the table above.
- **Intervals cover query sampling only.** The bootstrap asks whether a
  difference would survive other queries *like these*. It cannot speak for
  query types, languages or corpus sizes the set does not contain.
