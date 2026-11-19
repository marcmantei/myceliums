# Real-repository retrieval set (#95)

> **The labels in this set are drafts.** Every query was labelled by an AI
> agent (`"labelled_by": "agent:claude"`) and **no human has reviewed them yet**
> (`"reviewed_by": null`). Until a person has checked them with the review
> helper below, treat the set — and every number measured on it — as
> provisional, not as ground truth. The evaluator prints how many queries are
> still unreviewed on every run.

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
- `recorded-run.json` — the run the methodology's numbers come from, written by
  `real-repo-eval --record`. Generated; never edit it by hand.

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
note to list the query for fixing by hand; `s` skips, `q` stops. An `agent:`
identity is refused as a reviewer.

Ask, for each query: would I want to read exactly these symbols first to
answer it? Is any labelled symbol only a caller, a test or a type that merely
names the answer? Is an equally good answer missing? Reviewing changes the
dataset digest; re-run `real-repo-eval --record` afterwards so the recorded
numbers belong to the reviewed labels.

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
