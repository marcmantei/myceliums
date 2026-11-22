# Call Resolution: How It Works, How Far to Trust It

Myceliums builds a call graph from what the code declares: every call site is linked to the function, method or class it calls, by the language's own scoping rules and the types the code spells out. It does no type inference. This guide says what the resolver does, what each edge's resolution label means, how accurate it measured, and where it is blind.

## How call resolution works

The parsers report each call with its line, the callee's name, how the call names it (a bare `f()`, a call on a receiver `x.f()` / `X::f()` / `pkg.F()`, a constructor `new X()`, `super(...)`) and the receiver's expression. For Python, TypeScript/JavaScript, Go, Rust and Java they also report every import, and every parameter, local variable and field with its declared type or the expression that initialised it.

**The caller** is found by position: the innermost function or method in the call's file whose lines contain the call site. Calls inside closures, lambdas and nested blocks belong to the enclosing named function. A call outside every function (module-level code) has no caller and no edge.

**The callee** is looked up only among symbols of the caller's [language family](#language-families), nearest scope first:

1. **Bare call `f()`.** A parameter or local named `f` shadows everything: the call goes through a function value and gets no edge (unless the name holds a class, as `cls` does in a Python classmethod). Then, in Java, the methods of the enclosing classes and their base classes; then the caller's own file; then what the file imports; then (Go) the other files of the package. In these five languages nothing else is in scope, so a name found nowhere there — a built-in like `print` or `len`, a library function — gets no edge. A bare call never reaches a method in Python, TypeScript, Go or Rust.
2. **Call on a receiver `x.f()`.** The receiver's type is worked out from declarations in view, without inference:
   - `self`, `this`, `super`, Rust's `Self` → the enclosing class (its base class);
   - a class or module name → `Config.load()`, `Validator::new()`, `config.parse()`;
   - a constructor or literal → `new Config(...).get()`, `User(...).save()`, `&model.User{...}`, `"x".join(...)`;
   - a parameter, variable or field with a declared type (`repo: Repository`, `Repository<User> repo`, `repo *store.Repo[*User]`, a struct field), or initialised from one of the above (`user = User(...)`);
   - a module-level value with a declared type, also imported (`from .globals import current_app` where `current_app: Flask = ...`);
   - a call's declared return type (`load(text): Config`, `-> Self`, `func Load(...) (*Config, error)`).

   On a typed receiver the call goes to that class's method, or the nearest base class's (Go: embedded struct's); on a module, to its top-level function; on a library type (a string, a `Map`, `strings`, `std::`), nowhere. Once a receiver is typed, a member its class does not declare — here or in a project base class — comes from a library base class (gin's `c.Writer.Header()`: `Header` is `http.ResponseWriter`'s), so the rest of the chain is a library value and is never matched by name. Two exceptions treat the receiver as untyped instead: a class with a base the resolver cannot pin down (a parameter, as in Django's related managers built by `create_forward_many_to_many_manager(superclass, …)`; a call result; a name two classes share), and a Rust type when a project trait declares a method of that name (the type may implement the trait elsewhere; `impl Trait for Type` is recorded as a base, so a trait's default method is found `exact`). Only a receiver whose type is not known — an untyped parameter, a field assigned from one, the result of a function without a declared return type — falls back to the method name: any *method* of that name in the language family, never a free function.
3. **Constructor call `new X()`** (and Python's `X()`) → the class `X` in scope. **`super(...)`** → the base class's constructor.
4. **Languages whose extractor does not record call forms or bindings** (C, C++, C#, Ruby, Kotlin, Swift, PHP, Lua, Zig, PowerShell, Elixir, Scala, Objective-C, Dart): by name, any callable kind, nearest scope first — the caller's file, its imports, its directory, the whole language family.

**Imports** are resolved to project files per language: Python modules and packages (absolute from a source root — a module inside a package, a directory with an `__init__.py`, is never a top-level module, so `from io import BytesIO` stays the standard library's even next to Django's `django/contrib/gis/geos/io.py` — or relative; `import a.b` binds `a`), relative TypeScript paths, the repository's own [workspace packages and tsconfig `paths`](#monorepos-workspace-packages-and-tsconfig-paths) (and `@/`, `~/` aliases), Go import paths to the package directory, Rust `use` paths to module files, Java classes and static imports. An import binds what the file declares under the name: a symbol, a module-level value, a submodule, or a re-export — followed up to three deep, also through `from m import *`, TypeScript `export { x } from './m'`, `export * from`, `export { x as y }` and barrels that import and re-export; a library name a project module re-exports stays a library name. A specifier that names no project file is a library: a call through it gets no edge. A name a project file exports without a declaration the resolver can see is *unknown*, not a library: method calls on it fall back to the method name.

**Several candidates.** Where more than one symbol qualifies — a method name shared by two classes on an untyped receiver, two build-tag variants of a Go function — the call is linked to each candidate, labelled `ambiguous`, up to `MAX_AMBIGUOUS_TARGETS` (2). With three or more candidates the name says too little and the call gets no edge. Java overloads are first narrowed by argument count.

Every CALLS edge records how its target was found in its metadata (`line:<n>;resolved:<label>`); the MCP tools that list callers and callees show it.

**Calls that get no edge are counted.** Each analysis records how many call sites were linked and why the others were not — more than two candidates, a library target, a call through a variable, nothing in scope, outside any function. `myc analyze` and the MCP `analyze` tool print the counts (on Django 5.1.4: 60,298 call sites linked; 97,203 not: 9,041 with more than two candidates, 68,032 into libraries, 2,006 through variables, 17,877 with nothing in scope, 247 outside any function). Unlinked call sites whose name a project function, method or class of the same language has are also counted per name, and each caller's own unlinked calls that may reach project code — more than two candidates, a project name nothing in scope binds, a call through a variable, parameter or callback — are recorded with their line (#169; library calls are not). The walks report both as places they could not follow ([below](#walks-that-stop-early)). Incremental re-indexing (`analyze_files`) keeps the last full run's record, so its lines can be stale for files changed since.

### Walks that stop early

`trace_flow`, `find_callers`/`find_callees` (and `myc callers`/`myc callees`) with a depth, and `impact_of_change` (and `myc impact`) never present a cut walk as complete (#169). When they stopped short they add an `Incomplete (N branch(es) not followed)` block — in `find_callers`/`find_callees` the `incomplete` object (`reasons`, `branches_cut`, `next`) — right under the first line, with one reason per cut and a `Next:` step that is the exact call to continue:

| reason | when | next step |
|---|---|---|
| depth limit | a transitive walk (`depth` > 1, `impact_of_change`, `trace_flow` without a chain) stopped with edges leading further: how many symbols one more step adds, and through which | the same call with `depth`/`max_depth` raised |
| longer chains | `trace_flow` found a chain, and more symbols lie on chains longer than the listed window (shortest + `extra_calls`) within `max_depth` | the same call with `extra_calls` raised; `find_callers` of the target when the gap is over 5 calls |
| page | the list is paged | the same call with `offset` |
| calls made here got no CALLS edge | a walked symbol makes calls the resolver could not link that may reach project code (callbacks, more than two candidates, unbound names): each with file and line | read those lines |
| call sites named like a symbol here got no CALLS edge | callers the graph may be missing | grep for the name |

Direct callers (`depth` 1) are the whole answer to "who calls X", so their depth limit is not a reason; their unlinked call sites are. Nothing is said when nothing was cut — which still does not make a graph answer complete: it is a heuristic, and for completeness-critical questions the agent guidance (server instructions, the routing block, the `code-explorer` subagent) says to read the source.

### Monorepos: workspace packages and tsconfig paths

A bare specifier is a library unless the repository declares it (#169). Before that, every import of a sibling package — `import { isArray } from '@vue/shared'` in vuejs/core — was a library import, and the calls through it had no edge. The analyzer now reads, besides the source files:

- **workspaces**: `package.json` `workspaces` (an array or `{ "packages": [...] }`), `pnpm-workspace.yaml` `packages` (with `!` excludes), `lerna.json` `packages` (default `packages/*`), at any depth (a workspace root need not be the repository root). Each member's `package.json` `name` is a package of the repository. Its entry is looked for among the indexed files: a declared entry (`source`, `exports["."]` with a `source` condition first, `module`, `main`, `browser`, `types`) under a build directory (`dist/index.js`, `lib/…`, `esm/…`) stands for the same path under `src/`, since build output is not indexed; then `src/index.*`; then the declared entries as written (a root `index.js` that loads `dist/` comes after `src/index`); then the package's own `index.*`. A subpath (`@acme/store/memory`) goes through `exports` subpaths (with `*` patterns), then `src/<subpath>`, then `<subpath>`. Of two members with one name, the one sharing more leading directories with the importer wins. Nx and other tools that only use tsconfig `paths` are covered by the next point.
- **tsconfig/jsconfig**: `compilerOptions.paths` and `baseUrl` of every `tsconfig*.json`/`jsconfig*.json` (comments and trailing commas allowed), following `extends` (a relative path, with or without `.json`, or a file of a workspace package such as `@repo/typescript-config/base.json`; never `node_modules`; an array of bases, later ones winning). `paths` targets are relative to `baseUrl`, else to the file that declares `paths`. The aliases apply to the files under the config's directory; the nearest directory with a `tsconfig.json`, `jsconfig.json` or other `tsconfig.*.json` that sets `paths` or `baseUrl` wins. An exact pattern beats a wildcard, a longer wildcard prefix a shorter one, and a pattern whose targets name no indexed file falls through.

A bare specifier is tried against the importer's `paths`, then its `baseUrl`, then the workspace packages, then the `@/x`/`~/x` source-root convention; whatever names no indexed file stays a library. A call through such an import is labelled `import`, like any other import. `export *` and `export { … } from` chains are followed across packages as within one (up to three deep). Changing a `package.json` or tsconfig does not mark the index stale: re-run `analyze` after such a change.

Measured (#169):

| | before | after |
|---|---:|---:|
| graph-eval monorepo set (`ts-monorepo/`, 25 gold edges, 14 cross-package, 5 through `paths`): recall | 8 / 25 = 0.32 (0.17–0.52) | **25 / 25 = 1.00 (0.87–1.00)** |
| … precision | 8 / 8 | **25 / 25** |
| vuejs/core v3.5.46 (the #163 checkout and index excludes): call sites linked | 3,682 | 5,481 |
| … into libraries | 5,984 | 3,723 |
| … CALLS edges | 2,670 | 3,692 |
| … CALLS edges between two `packages/*` | 138 | 948 |
| … of those, the TypeScript checker resolves a call in the caller's lines to the target (`evals/agent/groundtruth/vue_edges.js`) | 11 | 808 |
| … checker-resolved cross-package call sites inside a function that have an edge (of 1,811) | 9 | 1,162 |

The other five gold sets and the held-out set are unchanged. Of the 810 Vue cross-package edges the change adds, 789 are `import` edges; 797 of the 810 match a checker-resolved call at the edge's line. Of the 13 that do not, 2 are right (`new EffectScope(…)` → the class, where the checker names the constructor) and 11 wrong: 9 are the name fallback for receivers of unknown type, which now reaches values typed by interfaces of another package (`exp.children.forEach(…)` → a test file's `Collection.forEach`), one is `this._app.unmount()` linked `ambiguous` to two wrong methods, and one follows a barrel that imports a name without re-exporting it. A sample of 10 added edges (seed 169) was checked against the source: all 10 right (e.g. `createRoot` imported from `@vue/compiler-dom`, which re-exports `@vue/compiler-core`). The other 127 cross-package edges the checker does not confirm (140 after, of 948) were there before, of 138 (`name-match` and `ambiguous` edges on untyped receivers).

## Language families

A call is never resolved to a symbol of another family. A family is a group of languages that call each other directly, with no binding layer:

| family | extensions |
|---|---|
| `js` | `ts`, `tsx`, `mts`, `cts`, `js`, `jsx`, `mjs`, `cjs`, `vue`, `svelte` |
| `c` | `c`, `h`, `cpp`, `cc`, `cxx`, `hpp`, `hh`, `hxx`, `m`, `mm` |
| `jvm` | `java`, `kt`, `kts`, `scala` |
| `python` | `py`, `pyi`, `ipynb` |
| `csharp` | `cs` |
| `elixir` | `ex`, `exs` |
| `powershell` | `ps1` |

Every other extension is a family of its own (`rs`, `go`, `rb`, `swift`, `php`, `lua`, `zig`, `dart`, ...). Calls across any other boundary — FFI, generated bindings, HTTP — are not resolved. (`myceliums_core::language_family` is the table.)

## Resolution labels

| label | meaning | gold set (all languages) |
|---|---|---:|
| `exact` | Determined by the call's syntax and the declarations in view: a method on a typed receiver (`self`/`this`/`super`, `Self`, a class, a variable, field or parameter whose type is declared or constructed in view, a declared return type), a class's static member, a constructor call, an unqualified call to the enclosing class's method (Java). | 172 / 172, precision 1.00 (0.98–1.00) |
| `import` | Bound by an import: an imported name, or a member of an imported module or package (also a workspace package or `paths` alias). | 56 / 56, 1.00 (0.94–1.00) |
| `name-match` | Found by name in the nearest scope where exactly one symbol qualifies: the caller's file, its package (Go) — or, for a call on a receiver of unknown type, the only method of that name in the language family; for languages without call forms, any scope up to the family. | 45 / 45, 1.00 (0.92–1.00) |
| `ambiguous` | Two symbols qualify; the call is linked to both. | none produced (see below) |

Intervals are 95% Wilson intervals. On the gold set every `ambiguous` case was resolved by receiver typing; the evidence for the label and for the cap of 2 comes from the same fixtures resolved *without* receiver typing (the step-3 row below): there, two-candidate edges were right 33 of 64 times (0.52) and edges among three or more candidates 21 of 104 times (0.20). That is why three or more candidates get no edge. Treat an `ambiguous` edge as "one of these two".

## Measured accuracy

### The gold set

Issue #94 added a gold set: small fixture projects in Python, TypeScript, Go, Rust and Java (`tests/fixtures/graph-accuracy/`) with every symbol and every static call labelled by hand, each edge with a one-line rationale, and every label cross-checked against the language's own toolchain. `cargo run -p myceliums-benchmarks --bin graph-eval` scores the production analyzer against it; the labelling rules are in `crates/myceliums-benchmarks/graph-gold/README.md`.

**Read these numbers with four caveats.**

- **The resolver rules were written while reading these fixtures** (#139). They implement general language rules — scoping, imports, declared types — not fixture-specific cases, but a gold set the rules were developed against measures the rules' coverage of its own constructs, not accuracy on unseen code. See [real repositories](#real-repositories) for an outside check.
- The labels were written by an agent and **have not been reviewed by a human yet**.
- The fixtures are a **stress test**: they deliberately reuse names (`parse`, `get`, `add`, `save`, `render`, `validate`, `map`) across modules, classes and languages, and declare most types.
- The sets are small (43–59 edges per language, 25 in the monorepo set, 274 in all). The intervals below say how little a perfect score on 45 edges proves.

Recorded baseline (`crates/myceliums-benchmarks/graph-gold/baseline.json`), each fixture analyzed as its own repository (`isolated`); with all six fixtures in one index (`polyglot`) every number is identical:

| language | gold edges | extracted | correct | call precision | call recall |
|---|---:|---:|---:|---:|---:|
| Go | 45 | 45 | 45 | 1.00 (0.92–1.00) | 1.00 (0.92–1.00) |
| Java | 59 | 59 | 59 | 1.00 (0.94–1.00) | 1.00 (0.94–1.00) |
| Python | 43 | 42 | 42 | 1.00 (0.92–1.00) | 0.98 (0.88–1.00) |
| Rust | 52 | 52 | 52 | 1.00 (0.93–1.00) | 1.00 (0.93–1.00) |
| TypeScript | 50 | 50 | 50 | 1.00 (0.93–1.00) | 1.00 (0.93–1.00) |
| TypeScript monorepo (#169) | 25 | 25 | 25 | 1.00 (0.87–1.00) | 1.00 (0.87–1.00) |
| **all** | **274** | **273** | **273** | **1.00 (0.99–1.00)** | **0.996 (0.98–1.00)** |

Symbols: 359 of 359 gold symbols are found (Go and TypeScript interface methods are now extracted); the one extra symbol is a local arrow function in TypeScript (`index.ts::handler`), which the gold set leaves out of scope. The one missed edge is `cfg = config.Config.load(text); cfg.get(...)` in Python: `load` is a classmethod returning `cls(...)` with no return annotation, so `cfg`'s type is not declared anywhere.

How each step of #139 moved the numbers (all languages):

| step | isolated P | isolated R | polyglot P | polyglot R | cross-language false edges | wrong-caller false edges |
|---|---:|---:|---:|---:|---:|---:|
| before (#94 baseline) | 0.57 | 0.56 | 0.34 | 0.20 | 45 | 43 |
| 1. caller by position | 0.69 | 0.72 | 0.26 | 0.27 | 160 | 0 |
| 2. + language families | 0.69 | 0.72 | 0.69 | 0.69 | 0 | 0 |
| 3. + scopes, imports, call forms (cap 1 / 2 / 3) | 0.95 / 0.84 / 0.66 | 0.72 / 0.86 / 0.94 | same as isolated | | 0 | 0 |
| 4. + receiver typing (cap 2) | 1.00 | 0.996 | 1.00 | 0.996 | 0 | 0 |

Step 1 alone raised the cross-language count: once callers were attributed to their real language, more calls reached for other languages' symbols. "Cap" is `MAX_AMBIGUOUS_TARGETS`; after step 4, caps 1 and 2 give the same result and cap 3 adds 1 correct and 11 wrong edges (precision 0.96, recall 1.00).

### Real repositories

The gold fixtures cannot show how the rules generalise. Two checks on real code:

**The held-out set** (`crates/myceliums-benchmarks/graph-gold/held-out/`): 52 call sites drawn at random — by a seeded regex sampler, not from myceliums — from flask, requests, gin and hono at the pinned commits of the real-repository retrieval set (#95), 13 per repository, each traced to its target by reading the code; 22 of them call a project symbol, 30 a built-in, a library method or a value. The resolver was spot-checked against these four repositories during #139 (the bullets below), but the sites were drawn afterwards and scored without being looked at first. `graph-eval` reports the set next to the fixture gate whenever the checkouts are present (`real-repos/fetch.sh`); it is never gated. Scored per (caller, callee name) pair:

| | recall (pairs with a project target found) | precision (edges from those callers to symbols of that name) |
|---|---:|---:|
| before #139 (`origin/main`) | 10 / 22 = 0.45 (0.27–0.65) | 10 / 21 = 0.48 (0.28–0.68) |
| after | **21 / 22 = 0.95 (0.78–0.99)** | **21 / 22 = 0.95 (0.78–0.99)** |

The miss: Hono's `Context.redirect` calls `this.newResponse(...)`, and both are class fields holding arrow functions, which the TypeScript extractor does not extract as methods. The wrong edge: flask's `app.send_static_file(...)` on an untyped `app` fixture is linked to both `Flask.send_static_file` and `Blueprint.send_static_file` (`ambiguous`). These labels are an agent's, not yet reviewed; 22 positive pairs give wide intervals.

**Whole-repository changes** (CALLS edges, each repository analyzed on its own):

| repository | language | edges before | after | after: exact / import / name-match / ambiguous |
|---|---|---:|---:|---|
| flask | Python | 1,609 | 1,306 | 378 / 395 / 400 / 133 |
| requests | Python | 1,175 | 903 | 291 / 511 / 87 / 14 |
| gin | Go | 3,120 | 2,250 | 1,154 / 11 / 1,041 / 44 |
| hono | TypeScript | 1,333 | 1,220 | 152 / 557 / 447 / 64 |
| django 5.1.4 | Python | 65,808 | 44,641 | 19,693 / 18,513 / 4,733 / 1,702 |

Before #139, 41% of the edges in the first four were `ambiguous` and none `import`. Spot checks made during development (an agent reading the code): a uniform sample of 40 new edges had 38 correct, both wrong ones `ambiguous` fan-outs on untyped receivers; of 53 sampled dropped `name-match` edges, 35 were wrong before (the wrong caller, a call line outside the caller, a library method linked to a project one), 7 went through a parameter, loop variable or dynamically imported name, 1 moved to the interface method it statically binds to, and 10 were real misses, 9 of which were fixed (package re-exports, an absolute import matched to a test fixture's module of the same name, functions nested in methods, Go named slice types). An independent review of #144 (72 hand-traced pairs) found further general gaps, fixed in the same PR: module-level values and names a project module exports without a visible declaration were treated as libraries (flask's `current_app`), TypeScript `export … from` re-exports were not followed, `import a.b` bound `b`, and a chain on a typed receiver fell back to name matching after a library member (gin's `c.Writer.Header().Set()`). Its re-check found the opposite gap: a member a typed class does not declare counted as a library call even when the class has an unresolved project base, or (Rust) implements a project trait in another file — those calls now fall back to the name rules. Diffing whole-repository edges before and after that change exposed four more binding errors, also fixed: an absolute Python import matched a module inside a package (`import json` in flask bound `flask/json/__init__.py` itself; empty `__init__.py` files were not seen), `export { Hono as HonoBase }` was not followed, a library name re-exported by a project module was not a library name, and members of two same-named classes in one directory were mixed (gin's build-tag twins; Hono's `super(...)` linked the class's own constructor). On flask, requests, gin and hono the re-check removed 12 wrong edges, added 4 right ones and turned 3 `ambiguous` pairs into `exact` edges. On Django it adds name-matched edges from classes whose base is a parameter; as for any untyped receiver, some are wrong (`self.through(...)` → `ManyToManyDescriptor.through`).

The fallback for receivers of unknown type stays the weak spot: `ep.load()` on an entry point linked to the only project method named `load` is labelled `name-match` and wrong. Untyped code — most Python and JavaScript — relies on this fallback for its method calls.

**Cost.** Analyzing Django 5.1.4 (2,788 files, 41,842 symbols, ~157,000 call sites; debug build, embeddings off, two runs each): 51.6–54.1 s and 1,164–1,176 MiB peak RSS before (`origin/main`), 54.5–55.9 s and 1,098–1,129 MiB after (analysis time; a shared machine, so runs vary by a few seconds). Base classes and open hierarchies are memoised per class; without that the re-check's fallback cost Django 1–5 s more (the #139 commit message's "55–59 s vs 60 s": the memoised re-check head against the same head without memoisation, measured in another session than the before/after figures above, so the two pairs are not directly comparable).

## What is modelled, and what is not

No type inference: a receiver's type is known only when a declaration in view says it. What each language's extractor provides, and the main blind spots:

| language | modelled | not modelled |
|---|---|---|
| Python | call forms; `import`/`from … import` (absolute, relative, `*`, re-exports); parameter annotations, assignments, module-level annotated values, `self.x = …` fields, `cls` in classmethods; return annotations; base classes | untyped parameters (most real code: method calls fall back to the name); bases that are parameters or call results (members fall back to the name); return types without annotations; `super()` beyond the first base; decorators that replace functions; `__getattr__`, monkeypatching |
| TypeScript / JavaScript | call forms, `new`, `super(...)`; named, default and namespace imports, `require`, `export … from` re-exports; parameter, variable and field annotations, constructor parameter properties, `this.x = …`; declared return types; `extends`/`implements`; interface and abstract methods | inferred types (`const x = f()` where `f` declares no return type); types of destructured values; class fields holding arrow functions are not methods (no symbol, no caller); calls inside top-level arrow functions passed as arguments (`describe`/`it` callbacks) have no named caller |
| Go | call forms; import paths to package directories, aliases, dot imports; receivers, parameters, `:=`/`var` declarations, composite literals, struct fields; result types; embedding; interface method sets | interface dispatch links the interface method, not implementations; type parameters; method values; variables holding closures; build tags (a type declared in two variant files: calls inside each file stay in it, a call from elsewhere links one variant) |
| Rust | call forms, paths (`Type::f`, `module::f`, `Self::f`); `use` paths, `mod`; parameters, `let` bindings, statics and consts, struct fields; return types (`-> Self`); `impl Trait for Type` (trait default methods); calls written in macro arguments (bare calls and `self.m()`) | trait dispatch on generic `T: Trait` and trait objects beyond the trait's declaration; methods reached through `Deref` (`Box`, `Rc`); other calls inside macros; trait impls for library types |
| Java | call forms, `new`, `super(...)`; class, static and wildcard imports, same-package classes; parameters, locals, `var`, fields, record components; return types; `extends`/`implements`; implicit `this`; overloads by argument count | overloads with the same count (needs argument types); generics; members inherited from library classes; static wildcard imports; anonymous classes |
| other languages | caller by position, language family, scope preference (file, imports, directory, family) | call forms, receivers and bindings: a call on an object may link to any function or method of that name; their accuracy is not measured |

For every language: dynamic dispatch is linked to the statically bound declaration (`self.render_row()` in a base class goes to the base class's `render_row`, not to overrides; `sink.write()` on a `Sink` interface goes to `Sink.write`, not to `ConsoleSink.write`). So **an implementation of an interface, trait or abstract method gets no callers from calls made through the interface**: `find_callers` and `impact_of_change` on `ConsoleSink.write` miss them — look at the interface method's callers too. Calls through function values, callbacks, reflection and dynamic imports have no static target and no edge.

## Interpreting results: confidence guidelines

### Higher confidence

- `exact` and `import` edges.
- `name-match` edges between functions in the same file or package.

### Verify

- `ambiguous` edges: one of the two targets is usually wrong.
- `name-match` edges for a method call on an untyped value (`x.load()` where nothing declares `x`), or on an instance of a class with an unresolved base: the method may belong to a library type.
- Callers and callees in languages without call forms (see the table above).

### Missing edges

A missing edge is now the more likely error, especially in untyped code: a method call on a value of unknown type whose name three or more classes share gets no edge; a call through a variable, parameter or callback never does; an interface or abstract method's implementations get no callers from calls through the interface. "No callers" does not mean "unused": `find_callers`, `find_callees`, `trace_flow` and `impact_of_change` list the call sites they could not follow in their `Incomplete` block ([walks that stop early](#walks-that-stop-early)).

## Recommended practices

1. **Check the resolution label.** `exact` and `import` are almost always right; `ambiguous` needs a look at the code.
2. **Type annotations pay off.** In Python and TypeScript, an annotated parameter or return type turns a name guess into an `exact` edge.
3. **Verify impact analysis on edits to methods with common names** in untyped code: the fallback links by name.
4. **Trust the call graph for exploration, verify for decisions**: for "is it safe to refactor this?", combine it with your IDE's references and the tests.

## Future work

- Implementations for interface and trait dispatch (today the declaration is the target).
- Return types without annotations: a function whose body returns a constructor call (`return cls(...)`) could type its result.
- Call forms and bindings for C/C++, C#, Kotlin, Swift and the other extractors.
- A human review of the gold labels, and a gold set drawn from a real repository rather than written fixtures.
- Optional LSP enrichment for compiler-grade resolution.
