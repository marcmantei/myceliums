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
   - a call's declared return type (`load(text): Config`, `-> Self`, `func Load(...) (*Config, error)`).

   On a typed receiver the call goes to that class's method, or the nearest base class's (Go: embedded struct's); on a module, to its top-level function; on a library type (a string, a `Map`, `strings`, `std::`), nowhere. Only a receiver whose type is not known falls back to the method name: any *method* of that name in the language family, never a free function.
3. **Constructor call `new X()`** (and Python's `X()`) → the class `X` in scope. **`super(...)`** → the base class's constructor.
4. **Languages whose extractor does not record call forms or bindings** (C, C++, C#, Ruby, Kotlin, Swift, PHP, Lua, Zig, PowerShell, Elixir, Scala, Objective-C, Dart): by name, any callable kind, nearest scope first — the caller's file, its imports, its directory, the whole language family.

**Imports** are resolved to project files per language: Python modules and packages (absolute from a source root, or relative), relative TypeScript paths (and `@/`, `~/` aliases), Go import paths to the package directory, Rust `use` paths to module files, Java classes and static imports. Re-exports one import deep are followed (`from .sessions import Session` in a package's `__init__.py`; a TypeScript barrel that imports and re-exports). A specifier that names no project file is a library: a call through it gets no edge.

**Several candidates.** Where more than one symbol qualifies — a method name shared by two classes on an untyped receiver, two build-tag variants of a Go function — the call is linked to each candidate, labelled `ambiguous`, up to `MAX_AMBIGUOUS_TARGETS` (2). With three or more candidates the name says too little and the call gets no edge. Java overloads are first narrowed by argument count.

Every CALLS edge records how its target was found in its metadata (`line:<n>;resolved:<label>`); the MCP tools that list callers and callees show it.

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
| `exact` | Determined by the call's syntax and the declarations in view: a method on a typed receiver (`self`/`this`/`super`, `Self`, a class, a variable, field or parameter whose type is declared or constructed in view, a declared return type), a class's static member, a constructor call, an unqualified call to the enclosing class's method (Java). | 161 / 161, precision 1.00 (0.98–1.00) |
| `import` | Bound by an import: an imported name, or a member of an imported module or package. | 47 / 47, 1.00 (0.92–1.00) |
| `name-match` | Found by name in the nearest scope where exactly one symbol qualifies: the caller's file, its package (Go) — or, for a call on a receiver of unknown type, the only method of that name in the language family; for languages without call forms, any scope up to the family. | 40 / 40, 1.00 (0.91–1.00) |
| `ambiguous` | Two symbols qualify; the call is linked to both. | none produced (see below) |

Intervals are 95% Wilson intervals. On the gold set every `ambiguous` case was resolved by receiver typing; the evidence for the label and for the cap of 2 comes from the same fixtures resolved *without* receiver typing (the step-3 row below): there, two-candidate edges were right 33 of 64 times (0.52) and edges among three or more candidates 21 of 104 times (0.20). That is why three or more candidates get no edge. Treat an `ambiguous` edge as "one of these two".

## Measured accuracy

### The gold set

Issue #94 added a gold set: small fixture projects in Python, TypeScript, Go, Rust and Java (`tests/fixtures/graph-accuracy/`) with every symbol and every static call labelled by hand, each edge with a one-line rationale, and every label cross-checked against the language's own toolchain. `cargo run -p myceliums-benchmarks --bin graph-eval` scores the production analyzer against it; the labelling rules are in `crates/myceliums-benchmarks/graph-gold/README.md`.

**Read these numbers with four caveats.**

- **The resolver rules were written while reading these fixtures** (#139). They implement general language rules — scoping, imports, declared types — not fixture-specific cases, but a gold set the rules were developed against measures the rules' coverage of its own constructs, not accuracy on unseen code. See [real repositories](#real-repositories) for an outside check.
- The labels were written by an agent and **have not been reviewed by a human yet**.
- The fixtures are a **stress test**: they deliberately reuse names (`parse`, `get`, `add`, `save`, `render`, `validate`, `map`) across modules, classes and languages, and declare most types.
- The sets are small (43–59 edges per language, 249 in all). The intervals below say how little a perfect score on 45 edges proves.

Recorded baseline (`crates/myceliums-benchmarks/graph-gold/baseline.json`), each fixture analyzed as its own repository (`isolated`); with all five fixtures in one index (`polyglot`) every number is identical:

| language | gold edges | extracted | correct | call precision | call recall |
|---|---:|---:|---:|---:|---:|
| Go | 45 | 45 | 45 | 1.00 (0.92–1.00) | 1.00 (0.92–1.00) |
| Java | 59 | 59 | 59 | 1.00 (0.94–1.00) | 1.00 (0.94–1.00) |
| Python | 43 | 42 | 42 | 1.00 (0.92–1.00) | 0.98 (0.88–1.00) |
| Rust | 52 | 52 | 52 | 1.00 (0.93–1.00) | 1.00 (0.93–1.00) |
| TypeScript | 50 | 50 | 50 | 1.00 (0.93–1.00) | 1.00 (0.93–1.00) |
| **all** | **249** | **248** | **248** | **1.00 (0.98–1.00)** | **0.996 (0.98–1.00)** |

Symbols: 337 of 337 gold symbols are found (Go and TypeScript interface methods are now extracted); the one extra symbol is a local arrow function in TypeScript (`index.ts::handler`), which the gold set leaves out of scope. The one missed edge is `cfg = config.Config.load(text); cfg.get(...)` in Python: `load` is a classmethod returning `cls(...)` with no return annotation, so `cfg`'s type is not declared anywhere.

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

The gold fixtures cannot show how the rules generalise, so the resolver was also run on four real repositories (the checkouts the #95 evaluation uses), before and after #139. There is no gold set for them; the checks below are samples judged by reading the code (by an agent, not reviewed).

| repository | language | CALLS edges before | after | after: exact / import / name-match / ambiguous |
|---|---|---:|---:|---|
| flask | Python | 1,609 | 1,282 | 337 / 360 / 411 / 133 |
| requests | Python | 1,175 | 901 | 289 / 504 / 89 / 14 |
| gin | Go | 3,120 | 2,278 | 1,134 / 11 / 1,047 / 86 |
| hono | TypeScript | 1,333 | 1,193 | 135 / 354 / 442 / 60 |

(Label counts are over distinct caller–callee name pairs.) Before #139, 41% of these edges were `ambiguous` and none were `import`.

- **A uniform sample of 40 edges after #139** (10 per repository): 38 correct. Both wrong edges are `ambiguous` fan-outs on untyped receivers (`fp.read()` linked to a test helper's `read`; Hono's `c.text()` linked to `HonoRequest.text` beside the correct `Context.text`). Precision 0.95 (0.83–0.99) — a rough figure, from 40 edges.
- **Edges that disappeared**: most were `ambiguous` (2,585 of 3,816). Of 53 sampled dropped `name-match` edges, 35 were wrong before (the wrong caller, a call line outside the caller, a library method linked to a project one), 7 were calls through a parameter, loop variable or dynamically imported name (pytest fixtures, `await import()` destructuring), which have no static target, 1 now points to the interface method it statically binds to, and 10 were real misses. Nine of those were fixed in #139 (package re-exports, an absolute import matched to a test fixture's module of the same name, functions nested in methods, Go named slice types) and are back; one remains: a Go package-level variable holding a closure (`var engine = sync.OnceValue(...)`; `engine()` calls no function symbol).
- The fallback for receivers of unknown type is the weak spot: `ep.load()` on an entry point linked to the only project method named `load` (`JSONProvider.load`) is labelled `name-match` and wrong. Untyped code — most Python and JavaScript — relies on this fallback for its method calls.

## What is modelled, and what is not

No type inference: a receiver's type is known only when a declaration in view says it. What each language's extractor provides, and the main blind spots:

| language | modelled | not modelled |
|---|---|---|
| Python | call forms; `import`/`from … import` (absolute, relative, `*`); parameter annotations, assignments, `self.x = …` fields, `cls` in classmethods; return annotations; base classes | untyped parameters (most real code: method calls fall back to the name); return types without annotations; `super()` beyond the first base; decorators that replace functions; `__getattr__`, monkeypatching |
| TypeScript / JavaScript | call forms, `new`, `super(...)`; named, default and namespace imports, `require`; parameter, variable and field annotations, constructor parameter properties, `this.x = …`; declared return types; `extends`/`implements`; interface and abstract methods | inferred types (`const x = f()` where `f` declares no return type); `export … from` re-exports; types of destructured values; calls inside top-level arrow functions passed as arguments have no named caller |
| Go | call forms; import paths to package directories, aliases, dot imports; receivers, parameters, `:=`/`var` declarations, composite literals, struct fields; result types; embedding; interface method sets | interface dispatch links the interface method, not implementations; type parameters; method values; variables holding closures; build tags (variants become an `ambiguous` pair) |
| Rust | call forms, paths (`Type::f`, `module::f`, `Self::f`); `use` paths, `mod`; parameters, `let` bindings, struct fields; return types (`-> Self`); calls written in macro arguments (bare calls and `self.m()`) | trait dispatch on generic `T: Trait` and trait objects beyond the trait's declaration; methods reached through `Deref` (`Box`, `Rc`); other calls inside macros; trait impls for library types |
| Java | call forms, `new`, `super(...)`; class, static and wildcard imports, same-package classes; parameters, locals, `var`, fields, record components; return types; `extends`/`implements`; implicit `this`; overloads by argument count | overloads with the same count (needs argument types); generics; members inherited from library classes; static wildcard imports; anonymous classes |
| other languages | caller by position, language family, scope preference (file, imports, directory, family) | call forms, receivers and bindings: a call on an object may link to any function or method of that name; their accuracy is not measured |

For every language: dynamic dispatch is linked to the statically bound declaration (`self.render_row()` in a base class goes to the base class's `render_row`, not to overrides); calls through function values, callbacks, reflection and dynamic imports have no static target and no edge.

## Interpreting results: confidence guidelines

### Higher confidence

- `exact` and `import` edges.
- `name-match` edges between functions in the same file or package.

### Verify

- `ambiguous` edges: one of the two targets is usually wrong.
- `name-match` edges for a method call on an untyped value (`x.load()` where nothing declares `x`): the method may belong to a library type.
- Callers and callees in languages without call forms (see the table above).

### Missing edges

A missing edge is now the more likely error, especially in untyped code: a method call on a value of unknown type whose name three or more classes share gets no edge; a call through a variable, parameter or callback never does. "No callers" does not mean "unused".

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
