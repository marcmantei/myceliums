# Graph-accuracy gold set

Hand-labelled symbols and CALLS edges for the fixture projects in
`tests/fixtures/graph-accuracy/` (Python, TypeScript, Go, Rust, Java). The
`graph-eval` binary runs the production analyzer over those fixtures and
scores what it extracts against these labels.

```sh
cargo run -p myceliums-benchmarks --bin graph-eval                      # report
cargo run -p myceliums-benchmarks --bin graph-eval -- --check-baseline  # gate, as CI runs it
cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline # re-record
```

The run is offline and needs no model weights. It prints Markdown tables
(per language, per resolution label, per tag, every false positive and
negative; precision and recall with 95% Wilson intervals) and writes
`report.json` here (git-ignored). `baseline.json` is the committed
reference.

## The CI gate

The Retrieval Quality workflow runs `graph-eval --check-baseline` on every
push and pull request (since #139). It fails the build when

- overall CALLS precision or recall, in either setup, drops by at least
  **0.02** (about five of the 249 gold edges) **and** the paired bootstrap 95%
  interval of the drop lies wholly below zero — the retrieval gate's rule
  (`benchmarks/METHODOLOGY.md`, "The CI gate"); recall pairs the gold edges,
  precision the union of the edges either run extracted
  (`src/graph_eval/gate.rs`);
- any CALLS edge in the `polyglot` run connects two languages: the resolver
  never links across language families, so one such edge is a bug;
- the baseline was recorded against other labels or fixtures, or lacks a
  setup.

Per-language and per-label numbers are reported, not gated: 43–59 edges per
language are too few. An intended drop is accepted by re-recording the
baseline in the same PR and saying why.

## Held-out real-repository set

`held-out/real-repos.json` is the outside check on the fixtures, which the
resolver's rules were developed against (#139): 52 call sites drawn at random
from flask, requests, gin and hono at the pinned commits of the real-repository
retrieval set (`../real-repos/`), 13 per repository, each traced to its target
by reading the code. `held-out/sample_sites.py` draws them (a seeded regex over
`git ls-files`, no myceliums); the set's `rules` say which drawn sites were
skipped and how a target is labelled. 22 pairs call a project symbol; 30 call a
built-in, a library method or a value, so any edge from them is wrong.

`graph-eval` scores the set whenever the checkouts are present
(`../real-repos/fetch.sh`, `$MYC_EVAL_REPOS_DIR`), per (caller, callee name)
pair — recall over the pairs with a project target, precision over the edges
from those callers to symbols of that name — and prints it after the gate.
**It is never gated**: the pairs are few, and an agent labelled them (checked
in #164 under an owner policy; see "Review status"). Numbers: `docs/guides/call-resolution-limitations.md`.

## Review status

**Labelled by an agent; checked in #164 under an owner policy, mostly not by
a human one by one.** Every file says `"labelled_by": "agent"`. Its
`reviewed_by` names who checked its items, with a count per class: Marc
himself, the pre-screen accepted on the strength of his sample, or a
second-check agent. The per-item record is `evals/review/provenance.jsonl`,
and the counts are under [Review of the labels](#review-of-the-labels-164)
below. A change of labels or `reviewed_by` re-records the baseline.

The labels were written from reading the code, never from myceliums output,
and then cross-checked with the language's own tooling (below). Those checks
agree with every label; they cannot vouch for the *policy* choices, which are
the part a reviewer should look at first.

## Files

| file | contents |
|---|---|
| `<language>.json` | the gold set for one fixture |
| `baseline.json` | recorded scores, with provenance (generated) |
| `verify/` | the cross-checks, one per language |

Each gold file has:

- `symbols`: `{id, kind, line}`. The id is `<fixture-relative file>::<Container.name>`.
  Java overloads add their parameter types: `Repository.add(T, int)`. `line`
  is the line of the name in the definition.
- `calls`: `{from, to, also_accept?, line, tags, why}`. One entry per
  caller/callee pair; `line` is the first call site; `why` is a one-line
  rationale. `also_accept` lists other targets that count as the same edge.
- `dynamic`: calls that happen at run time but have no static target (a call
  through a function value). They are not scored.
- `traps`: call sites where a resolver is tempted into a wrong edge, with the
  reason. Most must not produce an edge to any fixture symbol (a built-in or
  library call). Where the site also makes a real call that is in `calls`, the
  trap names only the wrong target: "must not target `OrderReport.RenderRow`"
  (#164). Documentation only: scoring needs no list of negatives, because any
  extracted edge not in `calls` is a false positive. The report annotates a
  false positive that falls into a listed trap.

## Labelling rules

**Symbols in scope:** functions, methods (including constructors, trait and
interface method declarations), classes, structs, interfaces, traits, enums
and named type aliases. Out of scope: variables, constants, fields, enum
variants, modules, imports, local closures and lambdas. An extracted symbol of
an out-of-scope kind is ignored; an extracted function, method, class,
interface, enum or type alias with no gold counterpart is a false positive.

**A symbol matches** an extracted symbol in the same file with the same short
name whose line span contains the gold line. The kind is not part of the
match (a struct reported as a class still counts); disagreements are listed.

**An edge** is a static call: the caller is the innermost named function or
method containing the call site (calls inside closures, lambdas and
generator expressions belong to it), and the callee is the fixture symbol the
compiler or interpreter binds the call to. Calls to anything outside the
fixture (standard library, built-ins) are not edges.

- **Instantiation** (`Foo()`, `new Foo()`, `cls()` in a classmethod) is an
  edge to the class; the class's explicit constructor is accepted instead.
  Go and Rust struct literals are not calls. `Foo::new()`, `NewFoo()` are
  ordinary calls.
- **`super(...)` / `super().__init__()`** is an edge to the base-class
  constructor; the base class is accepted instead.
- **Dynamic dispatch** (`self.render_row()` overridden in a subclass, a call
  through an interface or `dyn Trait`) is an edge to the statically bound
  declaration; overriding implementations are accepted instead. Go embedding
  and Rust composition do not dispatch: there only the static target counts.
- **Calls through function values** (a parameter, a callback list) have no
  static target. Where the fixture runs one, it is listed under `dynamic`.
- **Macros** are expanded as the compiler expands them: a call written inside
  `println!(...)` is a call.

Tags say what makes an edge easy or hard: `same-name` (another fixture
symbol shares the callee's name), `alias` (import alias, static import),
`typed-receiver` (the receiver's declared type decides the target),
`method-vs-function`, `constructor`, `inherited`, `dispatch`,
`interface-dispatch`, `overload`, `shadowing`, `recursion`, `self-call`,
`qualified`/`module-qualified`, `chained`, `cross-file`, `same-file`,
`indirect` (the call site does not spell the callee's name, e.g. `cls()`,
`super()`).

The five fixtures share one small domain (users, orders, reports) and
deliberately reuse names across modules, classes and languages: `parse`,
`get`, `add`, `save`, `render`, `validate`, `map`, `join`. That is what the
set is for, and it makes the numbers a stress test, not an estimate for an
average repository. In the `polyglot` setup, where all five fixtures are one
repository, the shared vocabulary is a worst case.

## Cross-checks

Each script reads the gold file and compares it with what the toolchain
resolves; each exits non-zero on any disagreement and writes nothing into the
repository. Run them after changing a fixture or a label.

| language | tooling | command |
|---|---|---|
| Python | `ast` for symbols; `sys.setprofile` over a driver that runs every function | `python3 -I crates/myceliums-benchmarks/graph-gold/verify/python_trace.py` |
| TypeScript | `tsc --strict`; the compiler API resolves every call and `new` | `npm install --prefix /tmp/tsc typescript@5 && NODE_PATH=/tmp/tsc/node_modules node crates/myceliums-benchmarks/graph-gold/verify/ts_calls.cjs` |
| Go | `go vet`, `go build`; `go/types` resolves every call | `cd crates/myceliums-benchmarks/graph-gold/verify/go_calls && go run .` |
| Rust | `rustc` compiles; every call in rustc's MIR | `python3 -I crates/myceliums-benchmarks/graph-gold/verify/rust_mir.py` |
| Java | `javac -Xlint:all -Werror`; every invoke instruction in `javap -c -p` | `python3 -I crates/myceliums-benchmarks/graph-gold/verify/java_bytecode.py` |

The Python check is dynamic: it observes calls actually made, so it confirms
the `dynamic` entries too. The others are static and cover every call site.
The cargo tests additionally check that every gold line names its symbol and
every call line names its callee (unless tagged `indirect`), and that the
committed baseline was recorded against the current labels and fixtures
(`dataset_digest` covers both).

## What the numbers mean

See `docs/guides/call-resolution-limitations.md` for the measured results and
their interpretation. In short: per-label precision says how far to trust an
edge carrying that label; `recall_share` is the part of all gold edges found
with that label (the shares add up to the overall recall).

The analyzer walks files in name order, and the resolver no longer lets that
order pick among same-named symbols (#139): a run is deterministic. The report
still records the platform, and the comparison flags a mismatch.

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
| graph-gold | 640 | 96 | 541 | 0 | 0 | 3 | 0 / 0 | 0 | 0 | 5 of 5 | 96 / 0 |
| graph-held-out | 52 | 10 | 36 | 6 | 0 | 0 | 0 / 0 | 0 | 0 | 1 of 1 | 10 / 0 |

Checked against the reviewed labels and current:

- crates/myceliums-benchmarks/graph-gold/baseline.json — re-recorded on the reviewed labels (digest 3ad580be10c6…)
<!-- label-review:end -->
