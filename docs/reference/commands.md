# CLI Command Reference

Complete reference for all `myc` CLI commands. Run `myc --help` or `myc <command> --help` for built-in usage info.

## Analysis

### `myc analyze`

Analyze a codebase and build its knowledge graph. Parses source files, extracts symbols, resolves call relationships, detects communities (Louvain/Leiden), and traces execution flows.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<path>` | positional, required | | Path to the project directory |
| `--force` | bool | `false` | Force full re-analysis even if cache is fresh |
| `--max-age` | integer (minutes) | `60` | Maximum cache age in minutes before triggering re-analysis |
| `--skip-embeddings` | bool | `false` | Skip embedding generation (much faster, BM25 and Cypher still work) |
| `--strict-embeddings` | bool | `false` | Exit non-zero if any symbol fails to embed (for CI — see [Embedding accounting](#embedding-accounting)) |
| `--watch` | bool | `false` | Watch for file changes and re-index incrementally |
| `--no-git-check` | bool | `false` | Allow analyzing directories without a `.git` repository |

```bash
# Analyze a project
myc analyze /path/to/project

# Force fresh analysis, skipping embeddings for speed
myc analyze /path/to/project --force --skip-embeddings

# Watch mode for continuous re-indexing
myc analyze /path/to/project --watch

# CI: fail the build if the index is only partially embedded
myc analyze /path/to/project --strict-embeddings
```

#### Embedding accounting

Analysis reports how completely the index was embedded:

```
  Symbols:       1240
  Embeddings:    1240 (1240/1240 symbols)
```

The `Embeddings` line shows `embedded/total` symbols. Embedding a symbol can
fail (model load errors, provider timeouts, storage errors); when it does, the
symbol has **no vector** and is **invisible to `semantic-search` and hybrid
`search`** — those modes can only return symbols that were embedded. A partial
index is reported explicitly:

```
  Embeddings:    900 (900/1240 symbols)
  ⚠ Embedding failures: 340 — 900 of 1240 symbols have no vector and are invisible to semantic/hybrid search
```

- **`--strict-embeddings`** turns any embedding failure into a non-zero exit
  code, so CI can fail the build instead of silently shipping a half-empty
  index. Without the flag, failures are reported but the command still succeeds.
- The accounting is persisted in the index. At query time, `semantic-search`
  and hybrid `search` prepend a **partial-index warning** whenever the index is
  incomplete, so a stale or half-built index never answers with unwarranted
  confidence. (The warning is read from index metadata — no per-query vector
  scan.)

### `myc session`

Interactive session setup: checks cache freshness, prompts for analysis if data is stale or missing. Designed for editor integrations that run at session start.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<path>` | positional, optional | current directory | Path to the project directory |
| `--yes` | bool | `false` | Skip interactive prompt and auto-analyze if needed |
| `--timeout` | integer (seconds) | `300` | Maximum runtime in seconds for auto mode (0 = no limit) |
| `--no-git-check` | bool | `false` | Allow analyzing directories without a `.git` repository |

```bash
# Interactive session in current directory
myc session

# Non-interactive, auto-analyze with 5-minute timeout
myc session /path/to/project --yes --timeout 300
```

---

## Search

### `myc search`

Search symbols in a repository using BM25 text matching. Finds functions, classes, methods, and other code entities by name or content.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>` | positional, required | | Search query string |
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `-l, --limit` | integer | `20` | Maximum results to show |
| `--hybrid` | bool | `false` | Use hybrid search (BM25 + vector with RRF) |
| `--rerank` | bool | `false` | Apply cross-encoder reranking to hybrid search results |
| `--explain` | bool | `false` | Show scoring breakdown and graph paths for each result |

```bash
# Basic search
myc search "authentication handler"

# Hybrid search with reranking and explanations
myc search "parse config" --hybrid --rerank --explain --limit 10

# Search in a specific repo
myc search "database connection" --repo my-project
```

### `myc semantic-search`

Search for symbols using semantic similarity via vector embeddings. Returns symbols most similar in meaning to a natural language query. Requires embeddings to have been generated during analysis.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>` | positional, required | | Search query (natural language) |
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `-l, --limit` | integer | `10` | Maximum results to show |

```bash
myc semantic-search "function that validates user input"
```

### `myc knowledge`

Query across all knowledge sources. Finds which emails, docs, and code mention a symbol or keyword.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>` | positional, required | | Query term (symbol name or keyword) |
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `-l, --limit` | integer | `20` | Maximum results to return |

```bash
myc knowledge "processPayment" --limit 10
```

---

## Graph Queries

### `myc query`

Execute a Cypher query against the knowledge graph. Supports `MATCH`, `RETURN`, `WHERE`, `ORDER BY`, `LIMIT`, `SKIP`, `CONTAINS`, `IS NULL`. Write operations are blocked.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>` | positional, required | | Cypher query string |
| `-r, --repo` | string, optional | most recent | Repository ID or path |

```bash
# Find all functions in a file
myc query "MATCH (f:Function) WHERE f.file_path CONTAINS 'auth' RETURN f.name, f.file_path LIMIT 20"

# Find call relationships
myc query "MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a.name, b.name LIMIT 10"
```

### `myc communities`

Show detected communities (clusters of related symbols) for a repository.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |

```bash
myc communities my-project
```

### `myc processes`

Show traced execution flows for a repository. Each process represents a chain of function calls from an entry point.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |
| `--entry` | string, optional | | Filter by entry point name (case-insensitive substring match) |
| `--filter` | string, optional | | Filter by keyword in process description/flow (case-insensitive substring match) |
| `--limit` | integer, optional | all | Limit number of processes to display |
| `--min-steps` | integer, optional | | Show only processes with N or more steps |

```bash
# All processes
myc processes my-project

# Filter by entry point
myc processes my-project --entry handleRequest --min-steps 3
```

### `myc impact`

Detect impact of current changes via git diff. Traces changed symbols through the call graph to find indirectly affected code.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `-d, --depth` | integer | `2` | Graph traversal depth |
| `--diff` | string, optional | runs `git diff HEAD` | Diff string or path to a `.diff`/`.patch` file |

```bash
# Impact of uncommitted changes
myc impact

# Impact with deeper traversal
myc impact --depth 4

# Impact from a patch file
myc impact --diff changes.patch
```

### `myc path`

Find the shortest path between two symbols in the knowledge graph using BFS across all relationship types.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<from>` | positional, required | | Start symbol name or qualified name |
| `<to>` | positional, required | | End symbol name or qualified name |
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `-m, --max-depth` | integer | `10` | Maximum BFS depth |

```bash
myc path "handleRequest" "saveToDatabase" --max-depth 5
```

### `myc stats`

Show statistics for a repository: symbol counts by kind, file count, relationship count, languages, and communities.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |

```bash
myc stats my-project
```

### `myc rename`

Preview or apply renaming a symbol across the codebase. In preview mode, returns a rename plan with all edits needed without modifying files.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<symbol_name>` | positional, required | | Name of the symbol to rename |
| `<new_name>` | positional, required | | New name for the symbol |
| `-r, --repo` | string, optional | most recent | Repository ID or path |
| `--apply` | bool | `false` | Apply the rename (default is preview only) |

```bash
# Preview a rename
myc rename processPayment handlePayment

# Apply the rename
myc rename processPayment handlePayment --apply
```

### `myc diff`

Compare current graph against a stored snapshot to show drift. Shows added/removed symbols and relationships since the last snapshot.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |

```bash
myc diff my-project
```

### `myc report`

Generate a `GRAPH_REPORT.md` with god nodes, surprising connections, and community summary.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |
| `-o, --output` | path, optional | `GRAPH_REPORT.md` in current directory | Output file path |

```bash
myc report my-project
myc report my-project --output /tmp/report.md
```

---

## Setup

### `myc init`

Initialize a `.myceliums.toml` config file in the current directory.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--default` | bool | `false` | Create the config with defaults without prompting |

```bash
myc init
myc init --default
```

### `myc setup`

Auto-detect and set up all installed editors, or configure a specific editor. Installs MCP server configuration and (optionally) AI instructions.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--editor` | string, optional | auto-detect all | Specific editor to configure. Options: `claude`, `windsurf`, `zed`, `continue`, `vscode`, `jetbrains`, `cursor`, `gemini`, `codex`, `copilot`, `aider`, `kiro`, `spacebot`, `openclaw` |
| `--uninstall` | bool | `false` | Remove myceliums from all (or specified) editors |

```bash
# Auto-detect and configure all editors
myc setup

# Configure only Claude Code
myc setup --editor claude

# Remove from all editors
myc setup --uninstall
```

### Editor-specific setup commands

Each editor also has a dedicated setup command. These are equivalent to `myc setup --editor <name>`.

| Command | Editor |
|---------|--------|
| `myc setup-claude` | Claude Code |
| `myc setup-windsurf` | Windsurf |
| `myc setup-zed` | Zed |
| `myc setup-continue` | Continue |
| `myc setup-vscode` | VS Code |
| `myc setup-jetbrains` | JetBrains IDEs |
| `myc setup-cursor` | Cursor |
| `myc setup-gemini` | Gemini CLI |
| `myc setup-codex` | OpenAI Codex CLI |
| `myc setup-copilot` | GitHub Copilot CLI |
| `myc setup-aider` | Aider |
| `myc setup-kiro` | Kiro |
| `myc setup-spacebot` | Spacebot |
| `myc setup-openclaw` | OpenClaw |

All accept the `--uninstall` flag to remove the integration.

### `myc configure`

Manage global configuration stored in `~/.myceliums/config.toml`.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-s, --set` | string, optional | | Set a configuration value (format: `key=value`) |
| `-r, --reset` | bool | `false` | Reset configuration to defaults |

```bash
# View current config
myc configure

# Set a value
myc configure --set defaults.max_results=50

# Reset everything
myc configure --reset
```

### `myc doctor`

Check the health of the Myceliums installation. Verifies registry integrity, data directory state, orphaned data, and stale locks.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--download` | bool | `false` | Pre-download the fastembed model (~100 MB) |

```bash
myc doctor
myc doctor --download
```

---

## Data Management

### `myc list`

List all analyzed repositories tracked in the registry.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | bool | `false` | Output as JSON |

```bash
myc list
myc list --json
```

### `myc delete`

Delete a repository's analysis data from storage and the registry.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |

```bash
myc delete my-project
```

### `myc status`

Show an overview of all myceliums data, storage usage, and health. Displays analyzed repos, sizes, orphaned data directories, and model cache.

No flags.

```bash
myc status
```

### `myc clean`

Clean up myceliums data: specific repos, orphaned data, caches, or everything.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, optional | | Repository ID or path to clean |
| `--orphans` | bool | `false` | Remove orphaned data directories not tracked in the registry |
| `--all` | bool | `false` | Remove ALL myceliums data |
| `--cache` | bool | `false` | Remove the fastembed model cache |
| `--yes` | bool | `false` | Skip confirmation prompts |

```bash
# Clean a specific repo
myc clean my-project

# Remove orphaned data
myc clean --orphans

# Remove model cache
myc clean --cache

# Nuclear option (removes everything)
myc clean --all --yes
```

### `myc export`

Export graph data (symbols, relationships, communities) in various formats.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |
| `-o, --output` | path, optional | stdout | Output file path |
| `-f, --format` | string | `json` | Output format: `json`, `graphml`, or `svg` |
| `--width` | integer (pixels) | `1200` | SVG canvas width |
| `--height` | integer (pixels) | `800` | SVG canvas height |

```bash
# Export as JSON
myc export my-project -o graph.json

# Export as GraphML for Gephi/yEd
myc export my-project -f graphml -o graph.graphml

# Export as SVG
myc export my-project -f svg -o graph.svg --width 1600 --height 1000
```

### `myc wiki`

Export the knowledge graph as an Obsidian-compatible wiki. Each symbol becomes a markdown file with wikilinks to related symbols.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<repo>` | positional, required | | Repository ID or path |
| `-o, --output` | path, required | | Output directory for the wiki files |
| `--format` | string, optional | | Generate Obsidian vault structure (with `.obsidian` config) |

```bash
myc wiki my-project -o ./obsidian-vault --format obsidian
```

---

## Advanced

### `myc mcp`

Start the MCP (Model Context Protocol) server. By default uses stdio transport. See [MCP Tools Reference](mcp-tools.md) for the full list of available tools.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--http` | string, optional | stdio | Run as HTTP server (e.g., `0.0.0.0:9999` or `127.0.0.1:3000`) |

```bash
# Stdio transport (used by editor integrations)
myc mcp

# HTTP transport
myc mcp --http 127.0.0.1:9999
```

### `myc hook`

Manage git hooks for automatic knowledge-graph rebuilding on commit/checkout.

| Subcommand | Description |
|------------|-------------|
| `myc hook install` | Install `post-commit` and `post-checkout` hooks in the current git repo |
| `myc hook uninstall` | Remove myceliums git hooks from the current git repo |

```bash
myc hook install
myc hook uninstall
```

### `myc serve`

Start the interactive graph visualization server. Opens a web-based UI to explore the knowledge graph.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-p, --port` | integer | `8888` | Port to listen on |
| `-r, --repo` | string, optional | auto-detects from current directory | Repository ID or path |

```bash
myc serve
myc serve --port 3000 --repo my-project
```

### `myc email`

Manage email connections and sync for cross-domain knowledge queries.

| Subcommand | Description |
|------------|-------------|
| `myc email connect` | Configure an IMAP email connection |
| `myc email sync` | Sync new emails from configured IMAP connections |
| `myc email disconnect` | Remove an IMAP connection |

#### `myc email connect`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--host` | string, required | | IMAP server hostname (e.g., `imap.gmail.com`) |
| `--user` | string, required | | Login username (usually the full email address) |
| `--port` | integer | `993` | IMAP server port |

#### `myc email sync`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--account` | string, optional | all accounts | Specific account to sync |

#### `myc email disconnect`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<account>` | positional, required | | Account ID to remove |

```bash
myc email connect --host imap.gmail.com --user me@example.com
myc email sync
myc email disconnect my-account-id
```

### `myc format-hook`

Format MCP tool output for Claude Code PostToolUse hooks. Reads JSON from stdin. This is an internal command used by the Claude Code integration.

No flags.
