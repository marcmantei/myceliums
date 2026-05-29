# Data Management

How Myceliums stores, manages, and cleans up analysis data.

## Storage Location

All Myceliums data is stored in `~/.myceliums/`. This directory is created automatically on first use.

## Directory Structure

```
~/.myceliums/
  config.toml          # Global configuration (see config.md)
  repos.json           # Repository registry (tracks all analyzed repos)
  data/
    <repo-id>/         # LanceDB tables for each analyzed repository
      symbols.lance/
      relationships.lance/
      communities.lance/
      processes.lance/
      ...
  snapshots/
    <repo-id>/         # Graph snapshots for drift detection
      <timestamp>.json
  decisions/           # Architecture Decision Records
    <repo-id>/
      <decision-id>.json
  service-maps/        # Community-to-service name mappings
    <repo-id>.json
  fastembed_cache/     # Downloaded embedding model files (~100 MB)
```

### Registry (`repos.json`)

The registry is a JSON file that tracks all analyzed repositories. Each entry contains:

- **id**: Unique repository identifier (derived from the absolute path)
- **name**: Human-readable repository name (derived from directory name)
- **path**: Absolute path to the repository on disk
- **analyzed_at**: ISO 8601 timestamp of the last analysis
- **symbol_count**: Number of symbols extracted
- **file_count**: Number of files analyzed
- **analyzed_commit**: Git commit hash at the time of analysis (used for cache invalidation)

### Data Directory (`data/<repo-id>/`)

Each repository's analysis data is stored in LanceDB format. This is a columnar storage format optimized for vector search and structured queries. The directory contains multiple `.lance` table directories for symbols, relationships, communities, processes, and other graph entities.

### Snapshots (`snapshots/<repo-id>/`)

Snapshots capture the state of the knowledge graph at a point in time. They are used for drift detection (`myc diff`, `detect_architecture_drift`) and historical comparison (`snapshot_diff`). A new snapshot is saved automatically after each analysis.

---

## Inspection Commands

### `myc status`

Shows a comprehensive overview of all Myceliums data:

- All tracked repositories with their sizes, symbol counts, and last analysis times
- Orphaned data directories (data dirs not tracked in the registry)
- Model cache size
- Total disk usage

```bash
myc status
```

### `myc doctor`

Health check for the Myceliums installation. Verifies:

- Registry integrity (valid JSON, consistent entries)
- Data directory integrity (all registered repos have corresponding data dirs)
- Orphaned data detection (data dirs without registry entries)
- Stale lock file detection
- Embedding model availability

```bash
# Run health checks
myc doctor

# Pre-download the embedding model (~100 MB)
myc doctor --download
```

### `myc list`

List all tracked repositories with their IDs, paths, and analysis timestamps.

```bash
myc list
myc list --json
```

---

## Cleanup Commands

### `myc clean <repo>`

Remove analysis data for a specific repository. Prompts for confirmation unless `--yes` is passed.

```bash
myc clean my-project
myc clean my-project --yes
```

### `myc clean --orphans`

Remove orphaned data directories. These are data directories inside `~/.myceliums/data/` that are not tracked in the registry. Orphans typically happen when analysis is interrupted before the registry is updated.

```bash
myc clean --orphans
myc clean --orphans --yes
```

### `myc clean --cache`

Remove the fastembed model cache (`~/.myceliums/fastembed_cache/`). The model (~100 MB) will be re-downloaded on next use. Useful for freeing disk space or forcing a model update.

```bash
myc clean --cache
```

### `myc clean --all`

Remove ALL Myceliums data, including the registry, all repository data, snapshots, model cache, and decisions. This is a destructive operation that requires confirmation.

```bash
myc clean --all
myc clean --all --yes
```

### `myc delete <repo>`

Remove a specific repository from both the registry and its data directory. Unlike `myc clean <repo>`, this also removes the registry entry.

```bash
myc delete my-project
```

---

## Branch Checkout Behavior

Analysis is keyed by repository path, not by branch. The same data directory is used regardless of which branch is checked out. When you switch branches:

1. The next `myc session` or `myc analyze` call compares the current git HEAD against the `analyzed_commit` stored in the registry.
2. If the commits differ, a cache miss is triggered and re-analysis runs automatically (or prompts in interactive mode).
3. Incremental re-indexing uses `git diff` between the analyzed commit and the current HEAD to identify changed files, avoiding a full re-parse.

This means switching between branches is efficient: only changed files are re-analyzed, not the entire codebase.

---

## Orphaned Data

Orphaned data directories occur when:

- Analysis is interrupted (e.g., process crash, Ctrl+C) after the data directory is created but before the registry is updated
- A repository is moved or deleted from disk without running `myc delete`

**Detection:** Both `myc status` and `myc doctor` report orphaned data directories.

**Cleanup:** Run `myc clean --orphans` to remove all orphaned directories.

---

## Lock Files

Myceliums uses lock files to prevent concurrent analysis of the same repository. A lock is acquired at the start of analysis and released when analysis completes.

**Stale locks** can occur when a process crashes or is forcefully terminated during analysis. Stale locks are detected and auto-cleaned by:

- `myc doctor` (reports stale locks)
- `myc analyze` and `myc session` (auto-clean stale locks before starting)

A lock is considered stale if the process that created it is no longer running.

---

## Disk Usage Tips

| Component | Typical Size | How to Clean |
|-----------|-------------|--------------|
| Embedding model cache | ~100 MB | `myc clean --cache` |
| Small repo (< 1,000 symbols) | 5-20 MB | `myc clean <repo>` or `myc delete <repo>` |
| Medium repo (1,000-10,000 symbols) | 20-100 MB | `myc clean <repo>` or `myc delete <repo>` |
| Large repo (10,000+ symbols) | 100-500 MB | `myc clean <repo>` or `myc delete <repo>` |
| Analysis without embeddings | ~50% smaller | Use `--skip-embeddings` flag |

To check current disk usage, run `myc status` which reports per-repository and total storage sizes.
