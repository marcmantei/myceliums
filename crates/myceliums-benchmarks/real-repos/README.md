# Real-repository retrieval set (#95)

> **The labels were drafted by an AI agent and checked in #164 under an owner
> policy, mostly not by a human one by one.** Every query was labelled by an
> agent (`"labelled_by": "agent:claude"`). In the #164 review Marc checked 111
> of the 713 label items himself. The rest were accepted on the strength of
> his sample in each set (queries: 94 checked, 0 errors; test-seeking: 13
> checked, 0 errors) or decided by a second-check agent; see
> [Review of the labels](#review-of-the-labels-164). `reviewed_by` records
> this per query: 2 of the 168 queries are human-reviewed, 166 are
> agent-verified under the owner policy. The evaluator prints these counts on
> every run.

168 labelled queries over four open-source repositories, split into a
development half for tuning search parameters and a held-out half for
confirming them. Method, results and their interpretation:
[`../benchmarks/METHODOLOGY.md`](../benchmarks/METHODOLOGY.md#real-repository-held-out-set-95).

| Repository | Language | Pinned at | Queries | Symbols |
| --- | --- | --- | ---: | ---: |
| [psf/requests](https://github.com/psf/requests) | Python | `v2.34.2` `6e83187b8f` | 42 | 801 |
| [pallets/flask](https://github.com/pallets/flask) | Python | `3.1.3` `22d924701a` | 42 | 1 574 |
| [gin-gonic/gin](https://github.com/gin-gonic/gin) | Go | `v1.12.0` `73726dc606` | 42 | 1 898 |
| [honojs/hono](https://github.com/honojs/hono) | TypeScript | `v4.13.18` `f6fbaf0299` | 42 | 10 420 |

Each repository has 9 `exact-name`, 10 `paraphrase`, 14 `behavioural` and 9
`conceptual` queries. Each repository is indexed on its own, as `myc analyze`
indexes one repository; vector search is exact at every size (#117), so hono's
10k symbols are scanned exactly like the others'.

## Files

- `queries.json` — the repository pins and the labelled queries. Each query
  has an `id` (`<repo>-<intent>-<slug>`), the `query` text, its `intent`, the
  `relevant` symbols — each with the `why` that justifies it — a `rationale`
  for the label set as a whole, `labelled_by` and `reviewed_by`.
- `fetch.sh` — checks every repository out at its pinned commit into
  `$MYC_EVAL_REPOS_DIR` (default `~/.cache/myceliums-eval-repos`). The sources
  are fetched, never vendored. Needs `git` and `jq`.
- `exact-vs-ann-hono.json` — the `exact-vs-ann` report behind the
  methodology's "Exact search against the old IVF-PQ index on hono" (#141):
  per query, both rankings' top 10, the labels' exact ranks and every score.
  Generated; never edit it by hand.
- `recorded-run.json` — the run the methodology's numbers come from: the JSON
  report of a `real-repo-eval` run (here the CI workflow's artifact; locally,
  `--record` writes it). Generated; never edit it by hand. It was measured on
  dataset `2026-11-12`, one label before the current file (see the
  methodology).

## Running

```bash
crates/myceliums-benchmarks/real-repos/fetch.sh
FASTEMBED_CACHE_DIR=~/.cache/myceliums-eval-models HF_HOME=~/.cache/myceliums-eval-models \
  cargo run -p myceliums-benchmarks --bin real-repo-eval
```

Add `--allow-model-download` the first time if the weights are not cached,
`--no-sweep` to score the four modes only, `--record` to rewrite
`recorded-run.json`. CI runs it weekly and on demand
(`.github/workflows/retrieval-quality-real-repos.yml`), report-only.

### Embedding variants (#157)

```bash
# one variant, three modes, no sweeps; --repos keeps a local run short
cargo run -p myceliums-benchmarks --bin real-repo-eval -- --allow-model-download \
  --model granite-embedding-small-english-r2 --template 2 --chunking on \
  --modes lexical,semantic,hybrid --no-sweep --no-demotion --out variant-D.json
# compare reports against the incumbent's (the shipped default since #157:
# jina-embeddings-v2-base-code, template 2, chunking on)
cargo run -p myceliums-benchmarks --bin real-repo-compare -- --baseline variant-A.json variant-*.json
```

Every report records what indexing cost (embedding seconds, texts, chunk
vectors, vector and disk bytes, query latency, peak memory) and the
long-symbol slice. Heavy runs belong on CI: dispatch the workflow with a
`variants` list (and `release=true` for cost measurements) — see the comment
at the top of the workflow file. Locally, weights a sandbox cannot download
can be placed into the cache by hand at
`$FASTEMBED_CACHE_DIR/models--<org>--<name>/snapshots/<revision>/<path>`; the
loader checks them against the pinned SHA-256.

## Reviewing the labels

```bash
cargo run -p myceliums-benchmarks --bin real-repo-review -- --reviewer <your-handle>
cargo run -p myceliums-benchmarks --bin real-repo-review -- --reviewer <handle> --repo gin
cargo run -p myceliums-benchmarks --bin real-repo-review -- --show gin-paraphrase-stop-remaining-handlers
```

For each unreviewed query the helper prints the query, its intent and split,
the rationale, and every labelled symbol with its `why` and its source code
from the pinned checkout. Answer `y` if the labels are right — `reviewed_by`
becomes `human:<handle>` and the file is saved immediately — or `n` with a
note to list the query for fixing by hand; `s` skips, `q` stops. The file is
written to a temporary file and renamed, so an interrupted save cannot
truncate it. An `agent:` identity (any letter case) and the query's own
labeller are refused as reviewers — by the helper and, for hand edits, by the
dataset validation every tool runs on load.

`reviewed_by` records the provenance. `human:<handle>` is a human review. An
agent check accepted under an owner policy (#164) is recorded with the policy's
exact id, for example `agent:second-check, per marc's policy
(owner-2026-11-22)`, or `mixed, policy owner-2026-11-22: 2× human:marc; 1×
agent:prescreen, …`. Known ids are `OWNER_POLICIES` in
`src/eval/real_repos.rs`. The evaluator counts such a query as "agent-verified
under owner policy", never as human-reviewed, and the review helper still
offers it for a human review. An `agent:` reviewer without a known policy id
is refused.

Ask, for each query: would I want to read exactly these symbols first to
answer it? Is any labelled symbol only a caller, a test or a type that merely
names the answer? Reviewing changes the dataset digest; re-run
`real-repo-eval --record` afterwards so the recorded numbers belong to the
reviewed labels.

**The helper shows what is labelled, not what is missing.** It lets a reviewer
reject a wrong label; it cannot show an answer the labeller never found. To
look for those, read the code around the labelled symbols — callers, sibling
methods, the other half of a feature — or grep the checkout for the query's
key terms. Example: `requests-conceptual-session-cookie-persistence` was first
drafted without `SessionRedirectMixin.resolve_redirects`, which carries
cookies across redirect hops; review feedback added it (dataset `2026-11-19`).
Add a missing answer by hand, with its `why`, and say so in the rationale.

## How the labels were made

The labeller read the code at the pinned commit — never myceliums search
output, which would have made the benchmark grade itself — and checked every
label against the parser's symbol inventory
(`cargo run -p myceliums-benchmarks --bin corpus-inventory -- --repo <checkout>`),
so each `(file, symbol)` pair is something the engine can return. The
evaluator refuses to run if any label stops resolving.

Labelling rules, applied the same way in all four repositories:

1. **Relevant means "read this first".** A symbol is labelled when its code
   is what answers the query. Callers, tests (unless the query asks about
   tests), and types or options that merely name the answer are not.
2. **Same implementation, several files: label every copy.** When one
   implementation exists in more than one file — hono's per-runtime packages
   under `adapters/<runtime>/src/` that copy `src/adapter/<runtime>/`, gin's
   `binding/binding_nomsgpack.go` build-tag variant of `binding/binding.go` —
   each copy is an equally good answer, and which one the engine ranks first
   is not a relevance error. This mirrors the golden set, which labels the
   Python, TypeScript and JavaScript definitions of `format_name` alike.
   Variants that differ in substance (a runtime wrapper around a shared
   helper, a build-tag variant implementing a different codec) are separate
   symbols and are labelled only when they answer the query themselves.
3. **Thin wrappers are labelled only when named.** A function that only
   forwards to another (Flask's `helpers.url_for` → `Flask.url_for`) is
   labelled for an `exact-name` query that names it; otherwise the symbol
   holding the logic is.
4. **One `(file, symbol)` pair is one label.** Overloads or a property getter
   and setter that share a qualified name are covered by a single label; the
   evaluator counts a repeated pair in a ranking once.
5. **Conceptual queries list every symbol that implements the concept**, kept
   to the ones a reader would open, not every helper they call.

The labeller's own uncertainties are recorded in each query's `rationale`
(what was deliberately left out, and why) — the place to start a review.

<!-- label-review:start -->
## Review of the labels (#164)

Applied on 2026-11-22 with `evals/review` under policy `owner-2026-11-22`. An independent AI pre-screen marked every item confirmed, doubtful or wrong. Marc reviewed items one by one in the review page: the queue held every doubtful and wrong item and a seeded random sample of the confirmed ones (seed 164, 15 % per dataset; `evals/review/sample.json`). Over all datasets he decided 229 sampled confirmed items with 0 errors (`evals/review/decisions.jsonl`); the table gives each dataset's own human sample, which can be 0, and the prescreen provenance string names it. Marc's policy of 22 November 2026: pre-screen-confirmed items outside the sample are accepted as correct on the strength of the sample he checked; every other item he did not decide, and every item he marked unsure, is decided by an independent second-check agent (`evals/review/agent-check/decisions-*.jsonl`). An item whose final decision is unsure is not marked reviewed.

Provenance, per item in `evals/review/provenance.jsonl`:

- **human** — `human:marc`: Marc decided the item himself;
- **prescreen** — `agent:prescreen, accepted by marc's policy (owner-2026-11-22); human sample in this dataset: N checked, E errors`: confirmed by the pre-screen, outside the sample, accepted by the policy — no person looked at this item;
- **second-check** — `agent:second-check, per marc's policy (owner-2026-11-22)`: decided by the second-check agent (`evals/review/agent-check/`) — no person looked at this item;
- **rule** — `policy owner-2026-11-22 consistency rule`: decided by the policy's consistency rule (`evals/review/policy/owner-2026-11-22.json`), whatever any single check said;
- **restated** — `<class>, wording restated by policy owner-2026-11-22`: decided as its class says; the policy then restated its wording (a graph-gold trap now names the one target it must not have).

Policy addendum: Consistency rule: an optional answer that the task wording explicitly excludes (a Starlette or library call, a class instantiation where the task excludes instantiations, a deprecated decoy documented as not used internally, a function passed as a value where the task counts that as a reference) is removed from the optional list everywhere in the task set, not only where a second check flagged it; genuine near-misses (recursion or self-calls, thin wrappers, callers on the real path) stay optional. A graph-gold trap whose call site has a real CALLS edge is restated as 'must not target <X>' instead of being deleted.

`reviewed_by` of a unit (query, language file, held-out set, task) names the classes of its items: `human:marc` only when Marc decided every item, otherwise the class, or `mixed, policy owner-2026-11-22: …` with a count per class. It stays `null` where an item is unsure, open or needs a fix by hand. `real-repo-eval` reports retrieval queries as human-reviewed, agent-verified under owner policy, or unreviewed; only `human:` provenance counts as human.

| dataset | items | human | prescreen | second-check | rule | restated | wrong: fixed / by hand | unsure | open | units marked | human sample in this dataset: checked / errors |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| retrieval-queries | 624 | 95 | 529 | 0 | 0 | 0 | 0 / 0 | 0 | 0 | 168 of 168 | 94 / 0 |
| retrieval-test-seeking | 89 | 16 | 58 | 15 | 0 | 0 | 0 / 0 | 0 | 0 | 20 of 20 | 13 / 0 |

Measured on the labels before this review (re-record before relying on them):

- crates/myceliums-benchmarks/real-repos/recorded-run.json — measured on dataset 2026-11-12 (digest 0eeb0ba91cf3…), not on the current file; report-only. Re-run `cargo run -p myceliums-benchmarks --bin real-repo-eval -- --record` when its numbers should belong to the reviewed file
<!-- label-review:end -->
