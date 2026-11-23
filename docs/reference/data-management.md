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
  models/              # Downloaded model files (embedding models and reranker)
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

The MCP `analyze` tool builds a new index in `data/.staging/build-<pid>-…/` and, once it is complete, swaps it with the live directory: in one system call where the platform supports it (Linux `renameat2(RENAME_EXCHANGE)`, macOS `renamex_np(RENAME_SWAP)`), otherwise by renaming the live directory to `data/.staging/old-<pid>-…/` and the new one into place (a failed second rename is undone). A journal file `data/.staging/swap-<pid>-…` names the directory being replaced; it stays as long as an `old-` directory exists. A failed or cancelled analysis deletes its `build-` directory.

**Index generations (#175).** On Linux and macOS a full analysis — the MCP `analyze` tool and `myc analyze` alike, including the session hook and the full runs of `--watch` — builds the new index in `data/.staging/build-…/`, then moves it to `data/.generations/<pid>-…/` (an index *generation*) and makes `data/<repo-id>` a symbolic link to it (target `.generations/<pid>-…`, relative, so the data directory can be moved); the swap replaces the link. Every reader opens the link's target, not the link, so it reads one generation from start to end. A generation is never rebuilt or truncated in place: dropping, recreating or emptying a table of a published generation is refused (`Store::is_published_generation`), and only appends — incremental updates of `myc analyze --watch`, background embeddings, compaction — add new Lance versions, under new file names. So a file below a generation keeps its content while the generation exists. The previous generation is deleted right after the swap (renamed to `<name>.deleted` first, so a reader still on it finds it whole or not at all); a read still on it fails and, the index having changed, is retried on the new one (`IndexChanged`). Before, readers resolved every file through `data/<repo-id>`, and a swap between Lance listing a manifest and reading it gave Lance one generation's manifest size and the next one's bytes (a panic in debug builds, garbage in release builds). **Limits:** an index built before this change is a plain directory; it is read as before and becomes a link at its next full analysis — a reader that opened it before that analysis can still meet the old race once. On Windows (no directory links without privileges), or where the file system refuses a link, the build directory is swapped in itself, as before, and the race remains. `delete` and `myc delete` remove the link and its generation; recovery deletes generations no link points at.

`data/.staging/`, `data/.generations/` and `data/.locks/` are Myceliums' own; repository ids never start with `.` (a project folder named `.foo` gets the id `_foo-<hash>`), so no repository can be confused with them. Ids registered before this rule stay readable; re-analyzing such a project registers it under the new id and replaces the old entry, whose directory then shows up as orphaned.

**Interrupted rebuilds are recovered automatically.** When a process is killed mid-build, or between the two renames, or an undo fails, its `build-`, `old-` and `swap-` entries stay in `data/.staging/`. Every `myc` command (except the grep hook), every MCP server start and every new rebuild checks for entries of rebuilds that no longer run: an `old-` directory is renamed back (the previous index returns) unless its journal records that the swap committed — whatever then sits at the live path (nothing, or an empty directory a reader recreated) is moved aside first — and everything else is deleted. Read paths open an index only if its directory exists, so they do not recreate one. Journal, swap and `committed` marker are each flushed to disk (`fsync` of the file and its directory) before the next step relies on them, so the order survives a power loss. A link left in `data/.staging/` is deleted together with the generation it points at, and a generation that no link in `data/` or `data/.staging/` points at, and whose rebuild no longer runs, is deleted too (not while a running rebuild has a swap journal). Rebuilds still running are never touched, and neither `data/.staging/`, `data/.generations/` nor `data/.locks/` counts as an orphaned repository. Liveness is judged by PID, so two processes sharing one data directory from different PID namespaces (containers) are not supported.

### Snapshots (`snapshots/<repo-id>/`)

Snapshots capture the state of the knowledge graph at a point in time: symbols and edges keyed by file, qualified name and kind, so two analyses of unchanged code compare equal. Every analysis (MCP `analyze` or `myc analyze`) records one in `snapshots/<repo-id>/`; the newest 10 per repository are kept, and `snapshots/<repo-id>.json` holds the latest. `detect_architecture_drift` compares them (by default the previous analysis with the current graph).

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

# Pre-download the embedding model
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

Remove the model cache (`~/.myceliums/models/`, or `FASTEMBED_CACHE_DIR` / `HF_HOME` when set). The models are re-downloaded on next use. Useful for freeing disk space or forcing a model update.

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

Myceliums uses lock files to prevent concurrent analysis of the same repository. A lock is acquired at the start of analysis and released when analysis completes. Locks live in `data/.locks/<repo-id>.lock` (PID and start time), outside the index directory, which a rebuild replaces as a whole. (Older releases wrote `data/<repo-id>/analysis.lock`; `myc doctor` still reports stale ones there.)

**Stale locks** can occur when a process crashes or is forcefully terminated during analysis. Stale locks are detected and auto-cleaned by:

- `myc doctor` (reports stale locks)
- `myc analyze`, `myc session` and the MCP `analyze` tool (auto-clean stale locks before starting)

The MCP `analyze` tool takes the same lock, so `myc analyze` and the MCP server never index the same repository at once; a refused MCP call is an `isError` result saying `Analysis already running`. The MCP `delete` tool refuses a repository that is being analyzed the same way.

A lock is considered stale if the process that created it is no longer running.

---

## Disk Usage Tips

| Component | Typical Size | How to Clean |
|-----------|-------------|--------------|
| Model cache | 0.2–0.7 GB per embedding model, 2.2 GB for the reranker | `myc clean --cache` |
| Small repo (< 1,000 symbols) | 5-20 MB | `myc clean <repo>` or `myc delete <repo>` |
| Medium repo (1,000-10,000 symbols) | 20-100 MB | `myc clean <repo>` or `myc delete <repo>` |
| Large repo (10,000+ symbols) | 100-500 MB | `myc clean <repo>` or `myc delete <repo>` |
| Analysis without embeddings | ~50% smaller | Use `--skip-embeddings` flag |

To check current disk usage, run `myc status` which reports per-repository and total storage sizes.
