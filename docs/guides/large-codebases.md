# Working with Large Codebases

Myceliums scales from small scripts to large monorepos, but bigger projects need some tuning. This guide covers strategies for keeping analysis fast and memory usage reasonable.

## Skip Embeddings for Faster Analysis

Embedding generation is the most resource-intensive step. If you only need graph queries, BM25 search, or review context, skip it entirely:

```bash
myc analyze ./project --skip-embeddings
```

Everything except semantic search and hybrid search will still work:

- BM25 text search
- Cypher graph queries
- `get_review_context` (structural summaries)
- Community detection
- Process tracing
- Impact analysis

You can always generate embeddings later with `myc analyze --force` when you need semantic search.

## Set a Timeout

For CI pipelines, hooks, or automated sessions, set a timeout to prevent analysis from blocking indefinitely:

```bash
myc session . --yes --timeout 300
```

The timeout is in seconds (300 = 5 minutes). This is the default used by the Claude Code hook. If analysis does not complete within the timeout, myceliums saves partial results and exits cleanly.

## Configure Exclusions

Create a `.myceliums.toml` file in the project root to exclude directories and files that should not be indexed:

```toml
[analysis]
exclude = [
  "node_modules/**",
  "vendor/**",
  "*.min.js",
  "dist/**",
  "build/**",
  ".next/**",
  "coverage/**",
  "__pycache__/**",
  "*.generated.*"
]
max_file_size_kb = 256
```

The `max_file_size_kb` setting skips files larger than the specified size. This is useful for avoiding auto-generated files, bundled assets, or large data files that would bloat the graph without adding useful structure.

## Typical Resource Usage

These numbers are approximate and depend on language, file density, and whether embeddings are enabled.

| Codebase | Files | Analysis time | RAM | Disk |
|----------|-------|---------------|-----|------|
| Small | < 100 | 1-3s | < 200 MB | < 5 MB |
| Medium | 100-1,000 | 3-15s | 200-500 MB | 5-50 MB |
| Large | 1,000-10,000 | 15-120s | 500 MB - 2 GB | 50-200 MB |
| Very large | 10,000+ | 2-10 min | 2-8 GB | 200 MB+ |

Analysis time can be cut significantly by skipping embeddings (roughly 40-60% of total time for most projects).

## Batch Size Tuning

The `batch_size` setting in `.myceliums.toml` controls how many files are processed in a single batch during analysis. Lower values reduce peak memory usage at the cost of slightly longer analysis time:

```toml
[analysis]
batch_size = 50   # default is usually higher; lower this for memory-constrained environments
```

This is especially useful on machines with limited RAM or when running myceliums alongside other memory-intensive processes.

## Monorepo Tip

If your repository is very large (10,000+ files) and contains multiple independent sub-projects, consider analyzing them separately:

```bash
# Instead of analyzing the entire monorepo
myc analyze ./monorepo

# Analyze each sub-project independently
myc analyze ./monorepo/services/api
myc analyze ./monorepo/services/worker
myc analyze ./monorepo/packages/shared
```

Each sub-project gets its own graph, keeping analysis fast and memory usage predictable. Cross-project relationships will not be captured, but within each project the graph is complete.

## Quick Reference

| Goal | Command |
|------|---------|
| Fast analysis (no vectors) | `myc analyze . --skip-embeddings` |
| Bounded runtime | `myc session . --yes --timeout 300` |
| Re-generate everything | `myc analyze . --force` |
| Exclude paths | Add `exclude` to `.myceliums.toml` |
| Reduce memory | Lower `batch_size` in `.myceliums.toml` |
