# Graph-accuracy gold set

Hand-labelled symbols and CALLS edges for the fixture projects in
`tests/fixtures/graph-accuracy/` (Python, TypeScript, Go, Rust, Java). The
`graph-eval` binary runs the production analyzer over those fixtures and
scores what it extracts against these labels.

```sh
cargo run -p myceliums-benchmarks --bin graph-eval                      # report
cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline # re-record
```

The run is offline and needs no model weights. It prints Markdown tables
(per language, per resolution label, per tag, every false positive and
negative) and writes `report.json` here (git-ignored). `baseline.json` is the
committed reference. CI runs it as a **report-only** step of the Retrieval
Quality workflow: it never fails a build.

## Review status

**Labelled by an agent, not yet reviewed by a human.** Every file says
`"labelled_by": "agent", "reviewed_by": null`. Until a human has gone through
the labels, treat the numbers as provisional. A reviewer who signs off sets
`reviewed_by` and re-records the baseline.

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
- `traps`: call sites that must *not* produce an edge to a fixture symbol, with
  the reason. Documentation only: scoring needs no list of negatives, because
  any extracted edge not in `calls` is a false positive. The report annotates
  a false positive that falls into a listed trap.

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

The analyzer walks files in name order, and that order decides which of
several same-named symbols a bare-name call resolves to, so a run is
deterministic. The report still records the platform, and the comparison
flags a mismatch.
