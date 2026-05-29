# Large-Repo Benchmark Results

**Date:** 2026-03-24
**Machine:** Darwin arm64, Apple Silicon
**myc version:** 0.2.0

## Synthetic Large Project (5,010 files, 6 languages)

Languages: TypeScript (1,200), Python (1,200), Go (800), Rust (810), Java (600), C# (400)

| Metric | Value |
|---|---|
| Files generated | 5,010 |
| Files parsed | 4,987 |
| Symbols | 41,952 |
| Relationships | 45,028 |
| Processes detected | 3 |
| Analysis (skip-embeddings) | 1.6s |
| Cached analysis | 63ms |
| BM25 search | 141ms |
| Cypher query | 139ms |
| Impact detection | 107ms |
| Process tracing | 44ms |

### Key Takeaways

- **Analysis scales linearly:** 5,000 files with 42K symbols indexed in under 2 seconds
  (skip-embeddings mode). This is the mode used during interactive coding sessions.
- **Cache is near-instant:** When no files have changed, re-analysis completes in ~63ms,
  making it invisible to the user.
- **Queries are fast:** All query types (search, cypher, impact, processes) complete in
  under 150ms against a 42K-symbol graph, well within interactive latency requirements.

## Methodology

- **Analysis (skip-embeddings):** Full parse + graph build without vector embedding generation.
  This is the typical mode used during interactive sessions. BM25 search, Cypher queries,
  impact detection, and process tracing all work without embeddings.
- **Cached analysis:** Re-running `myc analyze` when the cache is still valid (no file changes).
- **BM25 search:** Keyword search across all indexed symbols (query: "process user").
- **Cypher query:** Structured graph query listing all Class nodes (LIMIT 30).
- **Impact detection:** Given a modified file (types.ts), trace which symbols are affected.
- **Process tracing:** Discover end-to-end execution flows through the codebase.

### Synthetic project structure

The synthetic project contains 5,010 files spread across 6 languages with:
- 20 modules per language package (api, auth, billing, cache, config, etc.)
- Cross-file imports and function calls between modules
- Classes with methods, standalone functions, interfaces/structs, and type definitions
- Realistic directory depth (e.g., `packages/java-core/src/main/java/com/example/services/`)

### How to reproduce

```bash
# Build the release binary
cargo build --release -p myc

# Run the benchmark script
./benchmarks/large-repo-benchmark.sh

# Or run with real-world repos too (clones django, TypeScript, go)
./benchmarks/large-repo-benchmark.sh --real
```
