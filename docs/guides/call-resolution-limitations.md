# Call Resolution: Understanding Limitations and Confidence

## How Call Resolution Works

Myceliums builds a call graph by parsing function definitions and tracking function calls. The resolver matches call sites to their target functions using a **name-based heuristic approach**.

### The Approach: Name-Based Matching

The call resolver uses this strategy, in order of precedence:

1. **Expression chain resolution**: For calls like `foo.bar()`, take the receiver identifier and look for `foo.bar` (qualified name)
2. **Import aliases**: If `bar` was imported as a local alias, resolve via the import mapping
3. **SSA-derived aliases**: If static single assignment analysis tracked `handler = authenticate`, resolve via that alias
4. **Bare name lookup**: Fall back to matching the bare function name globally
5. **Module graph**: Only when no symbol of that name exists, follow the file's imports

The **caller** of an edge is resolved the same way: by the name of the function the call appears in, not by its position in the file.

Every CALLS edge records how its target was picked (since #96): `resolved:exact` (a qualified name exactly one symbol has), `resolved:import` (through the module graph), `resolved:name-match` (a bare name exactly one symbol has) or `resolved:ambiguous` (several symbols share the name; one was picked). The MCP tools that list callers and callees show it; it is also in the edge's `metadata`.

### The Limitation: Last-Writer-Wins on Common Names

Because Myceliums uses a global `name -> uid` map, **same-named symbols merge into a single entry**. This causes false positives on common names:

**Example:**

```rust
// File: src/auth.rs
fn parse(input: &str) -> Result<Token> { /* ... */ }

// File: src/config.rs
fn parse(json: &str) -> Result<Config> { /* ... */ }

// File: main.rs
let token = parse(user_input);  // Which parse() is this?
```

Both `parse` functions collide in the global map. The resolver cannot distinguish whether `main.rs` is calling `auth::parse` or `config::parse`, so it picks one (the last one registered, which depends on the order files are walked). This produces a false edge in the call graph, labelled `ambiguous`.

The same collision applies to callers: a call inside `config::parse` can be recorded as a call from `auth::parse`.

**Real-world impact:**

- A developer edited a `.tsx` file containing a `.map()` call
- Myceliums reported **407 indirectly affected Rust symbols** via `myc impact`
- The false positive occurred because a Rust `map` constant collided with the `.map()` method
- Cross-language matches were equally uncertain

## Measured Accuracy

Issue #94 added a gold set: small fixture projects in Python, TypeScript, Go, Rust and Java (`tests/fixtures/graph-accuracy/`) with every symbol and every static call labelled by hand, each edge with a one-line rationale, and every label cross-checked against the language's own toolchain (the interpreter, the TypeScript compiler API, `go/types`, rustc's MIR, `javap`). `cargo run -p myceliums-benchmarks --bin graph-eval` scores the production analyzer against it; the labelling rules are in `crates/myceliums-benchmarks/graph-gold/README.md`.

**Read these numbers with three caveats.**

- The labels were written by an agent and **have not been reviewed by a human yet**.
- The fixtures are a **stress test, not a sample**: they deliberately reuse names (`parse`, `get`, `add`, `save`, `render`, `validate`, `map`) across modules, classes and methods. A typical repository has fewer collisions, so expect better numbers there; these show where resolution breaks, not how often it breaks on average.
- The sets are small (43–59 edges per language, 249 in all). A difference of a few points between languages is noise. Where a share is quoted below, the 95% interval (Wilson) is given.

Recorded baseline (macOS, `crates/myceliums-benchmarks/graph-gold/baseline.json`), each fixture analyzed as its own repository:

| language | symbol precision | symbol recall | gold edges | extracted | correct | call precision | call recall | call F1 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Go | 1.00 | 0.98 | 45 | 49 | 31 | 0.63 | 0.69 | 0.66 |
| Java | 1.00 | 1.00 | 59 | 58 | 36 | 0.62 | 0.61 | 0.62 |
| Python | 1.00 | 1.00 | 43 | 44 | 27 | 0.61 | 0.63 | 0.62 |
| Rust | 1.00 | 1.00 | 52 | 53 | 24 | 0.45 | 0.46 | 0.46 |
| TypeScript | 0.99 | 0.99 | 50 | 41 | 22 | 0.54 | 0.44 | 0.48 |
| **all** | **1.00** | **0.99** | **249** | **245** | **140** | **0.57** (0.51–0.63) | **0.56** (0.50–0.62) | **0.57** |

By resolution label, all languages together:

| label | edges | correct | precision | share of all gold edges found with it |
|---|---:|---:|---:|---:|
| `exact` | 2 | 2 | 1.00 (0.34–1.00) | 0.01 |
| `import` | 0 | – | – | 0.00 |
| `name-match` | 134 | 97 | 0.72 (0.64–0.79) | 0.39 |
| `ambiguous` | 109 | 41 | 0.38 (0.29–0.47) | 0.16 |

### What the numbers say

**Symbols are reliable.** 335 of 337 gold symbols are found; the misses are interface method declarations in Go and TypeScript (`Sink.Write`, `Sink.write`), and the one extra symbol is a local arrow function in TypeScript. Symbol extraction is not where the graph goes wrong.

**Roughly every other CALLS edge is wrong or missing.** Precision and recall both sit near 0.57 on these fixtures.

**Does `exact` deserve its name?** On this evidence, yes when it appears, but it almost never appears: 2 of 245 extracted edges, both correct, both Python method calls on a freshly built object whose expression starts with the class name (`Config({...}).get(...)`, `OrderReport([]).summary()`). Two edges do not establish a precision; the interval runs from 0.34 to 1.00. The label needs the first identifier of the call expression to be the name of the class that defines the method, so `cfg.get()` on a `Config` variable is never `exact`.

**`import` was never assigned.** The module graph is consulted only when no symbol anywhere has the called name. In code where names repeat, that never happens: an imported, aliased or module-qualified call is resolved by bare name first, and labelled `name-match` or `ambiguous`.

**`name-match` is right about three times in four** (0.72). A unique name is good evidence for the *callee*, but a quarter of these edges are still wrong, mostly for two reasons: the call was attributed to the wrong *caller* (a function elsewhere with the caller's name), or a library method shares the name of a project function (`items.map(...)` linked to the project's own `map()`, `rows.find(...)` linked to `Repository.find` as a recursive call).

**`ambiguous` is wrong more often than right** (0.38). Treat it as a hint that *some* function of that name is called, not which one.

**The caller is as much a problem as the callee.** At least 44 of the 105 wrong edges (42%) are recorded at a call site outside the caller's body: the call was made by another function of the same name (two `__init__`s, two `get`s, two `new`s) and attributed to whichever one the name lookup returned. This is a lower bound; edge metadata keeps the call line but not its file.

**What gets missed**, by the tags the gold set puts on each edge (found / labelled):

| situation | found |
|---|---:|
| callee name unique in the fixture (no `same-name` tag) | 106 / 147 (0.72) |
| callee shares its name with another fixture symbol | 34 / 102 (0.33) |
| receiver's declared type decides the target (`repo.add()` on a `Repository`) | 32 / 64 |
| call on `self`/`this` | 6 / 25 |
| instantiation / constructor | 29 / 51 |
| inherited method, `super(...)`, Java overloads | 1 / 4, 0 / 3, 0 / 3 |
| recursion, interface dispatch | 5 / 5, 4 / 4 |

Both calls written inside a Rust macro (`println!("{}", run(&argv))`) were missed: to the parser, the arguments of a macro are an unparsed token tree.

**Several languages in one index make it much worse.** With all five fixtures indexed as one repository, call precision drops to 0.26 and recall to 0.16. 68 of the 109 wrong edges connect two languages; for example, TypeScript's `items.map(...)` inside `Order.total` became an edge from Java's `Order.total` to Rust's `utils::map`: wrong caller and wrong callee at once. Nothing in the resolver prevents a call from one language resolving to a symbol in another. The fixtures share one vocabulary on purpose, so this is a worst case, but a real polyglot monorepo is exposed to the same mechanism.

**The result depends on file order.** Which of several same-named symbols wins is decided by the order the analyzer walks the directory, which is the filesystem's order. Two machines can produce different `ambiguous` edges for the same code.

## Interpreting Results: Confidence Guidelines

When using `myc impact` or querying the call graph, apply these heuristics:

### Higher Confidence

- Edges labelled `exact` (rare: the call expression starts with the defining class's name)
- Edges labelled `name-match` whose caller and callee both have names that are unique across the repository (e.g., `initializeAuthenticationSystem`)
- Recursion and calls through an interface or trait object, which were all found in the gold set

### Low Confidence (Verify Manually)

- Edges labelled `ambiguous`: on the gold set, fewer than half are correct
- Calls to or from functions with **common names** (`parse`, `handle`, `process`, `validate`, `get`, `add`, `map`, `filter`, `transform`): both the callee and the caller may be the wrong function of that name
- Method calls on an object (`repo.add(x)`): the receiver's type is not used, so any method of that name qualifies
- Calls across language boundaries (e.g., TypeScript → Rust in a monorepo): the resolver does not check languages
- Impact analysis that reports a **very large blast radius** (400+ affected symbols): this is often a cascade of false positives

### Missing Edges

A missing edge is as likely as a wrong one. Calls on `self`/`this`, constructor calls, `super(...)`, inherited methods, Java overloads and calls inside Rust macros are often not linked. "No callers" does not mean "unused".

## Cross-Language Matches

The resolver does not consider language. A CALLS edge from a TypeScript function to a Rust function is created the same way as any other, and labelled `name-match` or `ambiguous` like any other. Such an edge is almost always false. Symbol nodes carry a `file_path` but no language, so compare the extensions of the caller's and the callee's files (`s.file_path`, `t.file_path` in a Cypher query) to spot one, and treat a cross-language entry in `myc impact` output as noise unless the code really crosses that boundary (FFI, generated bindings).

## Recommended Practices

1. **Check the resolution label.** `exact` and a `name-match` between uniquely named functions are usually right; `ambiguous` needs a look at the code.

2. **Verify impact analysis on common-name edits**: If `myc impact` reports hundreds of affected symbols after editing a function named `handle` or `process`, spot-check the results manually. The report likely contains false positives.

3. **Use unique naming**: Encourage team conventions that avoid highly generic names. `validateUserEmail` is better than `validate`.

4. **Trust the call graph for exploration, verify for decisions**: The call graph is excellent for code navigation and understanding architecture, but for critical decisions ("is it safe to refactor this?"), combine it with your IDE's refactoring tools and test coverage analysis.

5. **Use BM25 search for high-confidence lookups**: Text search by function name is more reliable than call graph traversal for common names.

6. **Index one repository per language where you can**: the polyglot measurement above shows how much a shared index costs.

## Future Work: Precision Track

These limitations are acknowledged; the gold set (`graph-eval`) measures any change against them:

- **Resolve callers by position**: the enclosing function is known from the parse tree; resolving it by name causes a large share of the wrong edges
- **Per-file/scope keying**: Same-named symbols within different scopes become separate entries
- **Type inference**: Build receiver types to resolve method calls exactly (e.g., `foo.bar()` becomes `TypeOfFoo.bar`)
- **Language-aware resolution**: never resolve a call to a symbol in another language
- **Deterministic tie-breaking**: walk files in a fixed order so `ambiguous` edges do not depend on the filesystem
- **LSP enrichment**: Optional language-server integration for precise resolution

For now, treat Myceliums as a **high-fidelity heuristic tool**: excellent for understanding code structure and identifying likely relationships, but not a replacement for static analysis tools (like `rust-analyzer`) or full-program analysis.
