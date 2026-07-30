# Retrieval-Quality Benchmark — Methodology

How Myceliums measures whether search actually finds the right code, and what
the resulting numbers do and do not mean.

The benchmark answers one question: **given a query an agent would realistically
type, does the engine return the symbol that answers it, and how near the top?**
Everything below exists to make that question answerable with a number instead
of an opinion.

Run it with:

```bash
cargo run -p myceliums-benchmarks --bin retrieval-eval
cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
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

The spread is deliberate. A dataset of only exact-name lookups would report a
flattering number that says nothing about the queries agents actually struggle
with; the paraphrase and conceptual queries are where a lexical engine is
expected to do badly, and where a semantic leg should earn its keep.

## Metrics

Both metrics are pure functions of a ranking and a relevant set — no clock, no
I/O, no model.

**recall@k** — of the symbols labelled relevant, the fraction that appear in the
top `k` results:

```
recall@k = |relevant ∩ top-k retrieved| / |relevant|
```

Reported at k = 1, 5, 10. A query with an empty relevant set scores `0.0` rather
than `1.0`; the dataset validator rejects such queries, so this is a defensive
floor rather than an expected path.

**MRR (mean reciprocal rank)** — the mean over queries of the reciprocal of the
rank of the *first* relevant hit:

```
MRR = mean(1 / rank_of_first_relevant),  0 when nothing relevant was retrieved
```

A correct answer in position 1 scores 1.0, position 2 scores 0.5, position 4
scores 0.25. The measure rewards putting the right symbol first, which is what an
agent with a small context window actually needs.

Aggregates are unweighted means over queries, so a mode cannot inflate its score
by doing well on the queries that happen to carry many labels.

**Reading recall@k when a query has many labels.** Recall is a fraction of *all*
labelled answers, so a query with 13 relevant symbols caps recall@10 at 0.77 by
construction. That ceiling is a property of the query, not a defect of the
engine. MRR and recall@1 are the honest headline numbers for "did it find
something useful"; recall@10 answers "did it find everything".

## Search modes

The modes are the engine's real retrieval strategies, not invented names:

| Mode | Implementation | Offline |
| --- | --- | --- |
| `lexical` | `myceliums_core::search_symbols` — BM25 over name, signature, content, metadata | yes |
| `semantic` | vector similarity over persisted embeddings | no |
| `hybrid` | reciprocal-rank fusion of the lexical and semantic rankings | no |
| `hybrid+rerank` | hybrid fusion followed by cross-encoder reranking | no |

Only `lexical` can be scored in this environment. The semantic leg requires
fastembed model weights, which are downloaded on first use — that would make the
benchmark neither offline nor deterministic.

**Unavailable modes report `UNAVAILABLE` with a reason, and carry no numbers at
all.** They are named rather than omitted so the report states plainly what is
unmeasured. An absent measurement must never be mistaken for a zero, and
fabricating a number for an unrun mode would be worse than reporting nothing.
To measure them, run in an environment with the model weights already cached and
extend `SearchMode::rank`.

## Determinism

The benchmark is offline and reproducible by construction:

- The corpus is parsed from in-repo fixtures — no network, no model downloads.
- Fixture files are visited in sorted order and symbols keep parse order.
- BM25 scoring is deterministic; per-query output is sorted with ties broken by
  query id.
- Floats are rounded to four decimals so the JSON is diffable and free of noise.

Two runs over an unchanged tree produce byte-identical stdout and report JSON.
This is verified by the `evaluation_is_deterministic` and
`corpus_loading_is_deterministic` tests, and was confirmed by diffing two
consecutive runs. The only non-reproducible fields — `timestamp` and
`commit_sha` — are confined to the baseline's provenance header and never affect
a score.

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

Queries average 2.7 labelled answers each.

## Baseline

`golden/baseline.json` records the reference scores, refreshed with
`--update-baseline`. It carries the commit, timestamp and dataset version
alongside the numbers, so a score is never compared across different ground
truth. Changing labels means bumping `dataset_version` — a score from one dataset
version is not comparable to a score from another.

The baseline was recorded after the persisted-vector work in #28/#29 landed
(2026-07-23), against the dataset dated 2026-07-30.

### Recorded results

51 queries over 96 symbols:

| Mode | recall@1 | recall@5 | recall@10 | MRR |
| --- | ---: | ---: | ---: | ---: |
| `lexical` | 0.3765 | 0.6468 | 0.6983 | 0.6405 |
| `semantic` | UNAVAILABLE | | | |
| `hybrid` | UNAVAILABLE | | | |
| `hybrid+rerank` | UNAVAILABLE | | | |

Broken down by intent — this is the part worth reading:

| Intent | MRR | recall@1 | recall@10 |
| --- | ---: | ---: | ---: |
| `exact-name` | 0.9333 | 0.7611 | 0.9333 |
| `behavioural` | 0.5893 | 0.3095 | 0.6905 |
| `conceptual` | 0.5667 | 0.1118 | 0.4614 |
| `paraphrase` | 0.3958 | 0.1944 | 0.6111 |

BM25 is close to perfect when the query names the symbol (`exact-name` MRR 0.93)
and less than half as good when it does not (`paraphrase` MRR 0.40). Nine of the
51 queries return nothing relevant in the top 10 at all: four paraphrase, three
conceptual, one behavioural — and one `exact-name`.

That spread is the measurable case for the semantic and hybrid legs. When model
weights become available offline, this table is where the improvement has to
appear — and if it does not, the claim does not survive.

### The exact-name failure is the interesting one

`q02` searches `"UserService get_user"` and returns no labelled answer in the top
10, while `q31` — the bare `"getUser"` — ranks one first. Naming the class
repeats the token `user`, which lifts bare `user` variables and the `UserService`
class itself above the getter that was actually asked for.

Qualifying a query with its class currently makes retrieval *worse*, which is the
opposite of what anyone typing it would expect. The two queries are kept as a
pair so that this stays visible: if a future scoring change fixes it, `q02` moves
and the pair records the win.

## Continuous integration

The evaluation is meant to run on every push and pull request, writing a
current-vs-baseline table to the job summary.

The workflow lives in `benchmarks/retrieval-quality-ci.yml` rather than under
`.github/workflows/`, because the GitHub App that opens these PRs has no
`workflows` permission and GitHub rejects any push that touches a workflow file.
A maintainer enables it by moving the file into place:

```bash
git mv benchmarks/retrieval-quality-ci.yml \
       .github/workflows/retrieval-quality.yml
```

Nothing else needs to change — everything below the explanatory header is a
complete, valid workflow.

The job is **report-only and never fails the build**. A relevance change is a
judgement call — a label fix, a deliberate ranking trade-off, and a genuine
regression all move the same number, and only a human reading the diff can tell
them apart. A hard threshold would either be set so loose it catches nothing or
so tight it blocks honest work, so the job surfaces the delta and leaves the
decision to the reviewer.

It runs with `--no-default-features`: the embeddings feature is not needed to
score the lexical mode, and skipping it keeps the job offline and fast.

## Known limitations

- **Fixture-scale corpus.** The dataset uses only existing fixture projects (96
  symbols). Larger real repositories may exhibit different characteristics —
  in particular, BM25 precision usually degrades as the corpus grows, so these
  numbers should be read as a regression guard, not a promise about real repos.
- **Three modes unmeasured.** `semantic`, `hybrid` and `hybrid+rerank` cannot run
  offline, so the comparison the issue ultimately wants — does hybrid beat
  lexical? — is not yet answerable here.
- **Single labeller.** Labels reflect one reviewer's judgement, recorded in each
  query's `rationale`. There is no inter-annotator agreement measure.
- **English-only queries**, and five languages of the twenty the parser supports.
- **Concept queries are judgement calls.** Where a concept has no clean boundary
  ("persistence layer"), the labels take the narrowest defensible reading.
