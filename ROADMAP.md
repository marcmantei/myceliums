# Roadmap

This document sketches where Myceliums is headed. It is a direction of travel,
not a commitment — priorities shift as the project and its users learn. For the
authoritative list of in-flight work, see the [issue tracker].

## Near-term

- **Persisted-vector hybrid search.** Combine lexical and semantic ranking over
  the stored embeddings so a single query blends exact-token precision with
  meaning-based recall, without re-embedding at query time.
- **Retrieval evaluation.** A repeatable harness (built on `myceliums-benchmarks`)
  that measures retrieval quality against labelled query/answer sets, so ranking
  changes can be judged empirically rather than by feel.
- **Integration tests.** End-to-end coverage of `analyze → store → query` across
  representative repositories, guarding against regressions that unit tests miss.

## Mid-term

- **Chunked embeddings.** Split large symbols into overlapping chunks before
  embedding so long functions and documents are retrievable by the passage that
  actually matches, improving recall on big files.
- **Service-layer consolidation.** Unify the read/query paths shared by the CLI,
  MCP server, and HTTP server behind a single service layer, reducing
  duplication and drift between the three front-ends.
- **DSL-based language onboarding.** Grow the DSL-driven extractor path so new
  languages can be added declaratively instead of by hand-coding an extractor.
  The DSL already exists (`myceliums-core/src/dsl.rs`) but is gated to Python and
  Go and is **off by default** (`use_dsl = false`, see
  `myceliums-core/src/config.rs`). The mid-term goal is to mature it enough to
  enable by default and extend to more languages.

## Non-goals

These are explicitly out of scope; pursuing them would dilute the tool's focus.

- **Being a general-purpose graph database.** Cypher support is a deliberately
  small, **read-only** subset for querying code graphs — not a competitor to
  Neo4j. Mutating queries (`CREATE`, `DELETE`, `SET`, ...) are rejected by design.
- **Precise, compiler-grade call resolution.** Caller/callee edges are
  heuristic, name-based approximations. Full type-aware resolution per language
  is not a goal; see
  [call-resolution-limitations](docs/guides/call-resolution-limitations.md).
- **Runtime or dynamic analysis.** Myceliums models code as written (static
  structure). Profiling, tracing, and other runtime behaviour are out of scope.
- **A hosted, multi-tenant SaaS.** Myceliums is a local-first tool. Team features
  exist for shared metadata, not as the foundation of a managed service.

[issue tracker]: https://github.com/marcmantei/myceliums/issues
