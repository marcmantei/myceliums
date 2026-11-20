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
| `--skip-embeddings` | bool | `false` | Skip embedding generation (much faster, BM25 and Cypher still work). Symbols the previous index embedded keep their vectors; new and changed ones have none |
| `--strict-embeddings` | bool | `false` | Exit non-zero unless the index is fully embedded (for CI — see [Embedding accounting](#embedding-accounting)). Conflicts with `--skip-embeddings` |
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

- **A re-analysis keeps existing vectors.** Symbols whose embedding text is
  unchanged take their vector over from the index being replaced (same model
  only), shown as `Embeddings: 1240 (1243/1243 symbols, 1240 kept from the
  previous index)`. So only changed symbols are embedded again, and a run that
  does not embed — `--skip-embeddings`, the `myc session` default, or a model
  that fails — leaves the unchanged symbols' vectors in place instead of
  dropping them (#151).
- **`--strict-embeddings`** turns an incompletely embedded index into a non-zero
  exit code, so CI can fail the build instead of silently shipping a half-empty
  index. Without the flag, failures are reported but the command still succeeds.
  It cannot be combined with `--skip-embeddings` — asserting full coverage while
  generating no vectors is contradictory, so the CLI rejects the pair.
- **The assertion covers cached indexes too.** `--strict-embeddings` describes
  the index you end up *using*, not just the run that built it. When a fresh
  cache short-circuits re-analysis, the check is re-applied to the coverage
  recorded in the index. An index with no recorded accounting (built before
  embedding accounting existed, or with `--skip-embeddings`) cannot be verified,
  so strict mode fails and asks for `--force` rather than assuming success.
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
| `--strict-embeddings` | bool | `false` | Generate embeddings during bootstrap and fail unless the index is fully embedded |
| `--no-git-check` | bool | `false` | Allow analyzing directories without a `.git` repository |

```bash
# Interactive session in current directory
myc session

# Non-interactive, auto-analyze with 5-minute timeout
myc session /path/to/project --yes --timeout 300
```

Session bootstrap skips embedding generation by default so startup stays fast;
`myc analyze . --force` adds semantic vectors later. Pass `--strict-embeddings`
to opt into the slower, verified path — the session then generates vectors and
fails if the resulting index is not fully embedded.

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

Without a target: detect the impact of current changes via git diff, tracing changed symbols through the call graph to find indirectly affected code. With a target (symbol or file): list every symbol that calls or imports it, transitively up to `--depth`, and the files they are in.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<target>` | positional, optional | | Symbol (`parse_query`, `Item.AsString`) or file (`src/query.rs`). Conflicts with `--diff` |
| `-r, --repo` | string, optional | most recent; with a target: the index covering the cwd | Repository ID or path |
| `-d, --depth` | integer | `2` | Graph traversal depth |
| `--diff` | string, optional | runs `git diff HEAD` | Diff string or path to a `.diff`/`.patch` file |
| `--json` | bool | `false` | Output as JSON |
| `--budget` | integer (tokens) | `2000` | With a target: budget for the text output in estimated tokens; `--json` returns the same items |

```bash
# Impact of uncommitted changes
myc impact

# Impact with deeper traversal
myc impact --depth 4

# Impact from a patch file
myc impact --diff changes.patch

# What depends on a symbol / on a file
myc impact parse_query
myc impact src/query.rs --json
```

### `myc callers` / `myc callees`

Compact, one-shot answers for agents and hooks: the callers (or callees) of a symbol with file:line, ending with the index freshness (`index: abc1234 = HEAD`). An ambiguous name lists its candidate definitions with file:line (exit code 2); an unknown name suggests similar ones (exit code 1). Without `--repo`, the index covering the working directory is used, never another repository.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<symbol>` | positional, required | | Name, qualified name or qualified suffix (`Item.AsString`, `auth::login`) |
| `-d, --depth` | integer | `1` | Follow calls transitively up to this many hops; each line then shows its depth and the symbol it was reached through |
| `-r, --repo` | string, optional | index covering the cwd | Repository ID or path |
| `--json` | bool | `false` | Output as JSON (`symbol`, `total`, `omitted`, `results`, `freshness`) |
| `--budget` | integer (tokens) | `2000` | Budget for the text output in estimated tokens (1 token ≈ 4 characters); `--json` returns the same items, so its JSON is larger |

```bash
myc callers parse_query
myc callers Item.AsString --depth 3
myc callees handle_request --json
```

### `myc context`

A token-budgeted context pack for a task: symbols the text names, then lexical search hits, each with its signature and direct callers and callees, plus the files involved. `--json` is the prefetch API for custom agent harnesses (see [agent integration](../guides/agent-integration.md)).

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>...` | positional, required | | The task or question as free text (quotes optional) |
| `-r, --repo` | string, optional | index covering the cwd | Repository ID or path |
| `--hybrid` | bool | `false` | Rank with BM25 + vector search (needs embeddings; loads the model, which takes seconds) |
| `--json` | bool | `false` | Output as JSON |
| `--budget` | integer (tokens) | `2000` | Upper bound for the pack's text rendering in estimated tokens (`--json` holds the same entries); the most relevant entry is always kept |

```bash
myc context how are sessions refreshed
myc context --json --budget 1500 -- "Why does the login redirect twice?"
```

### `myc hook-grep`

Claude Code PreToolUse hook installed by `myc setup-claude`. Reads the hook JSON from stdin; when a Grep call or a grep/rg/git grep command searches for a function, method or type the index defines, prints `{"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "..."}}` naming the matching `myc` command. Prints nothing otherwise. Never blocks, always exits 0.

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

Compare the current graph with the previous analysis to show drift: added and removed symbols and relationships. Every `myc analyze` records a snapshot (the newest 10 are kept); the baseline is the analysis before the one that built the current index, chosen the same way as by the MCP `detect_architecture_drift` tool. With only one analysis recorded it says so.

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

Each editor also has a dedicated setup command. These are equivalent to `myc setup --editor <name>`, except `myc setup-claude`, which installs into the project by default:

| `myc setup-claude` flag | Default | Description |
|------|---------|-------------|
| `--scope project\|user` | `project` | `project`: `.mcp.json`, `.claude/settings.json`, `CLAUDE.md`, `.claude/agents/` in the repository; `user`: `~/.claude.json`, `~/.claude/` (what `myc setup --editor claude` does) |
| `--project <dir>` | git top level of the cwd | Project root for `--scope project` |
| `--no-routing` | | Skip (and remove) the CLAUDE.md routing block |
| `--no-redirect-hook` | | Skip (and remove) the PreToolUse grep-redirect hook |
| `--no-agent` | | Skip (and remove) the `code-explorer` subagent |
| `--no-always-load` | | User scope: write `"alwaysLoad": false`, so every myceliums tool stays behind Claude Code's tool search. Without it the user entry gets `"alwaysLoad": true` when `mcp.toolsets` is the default (core only), and no `alwaysLoad` when it adds groups (the core tools still load up front through their `_meta`). Project scope never writes the key into the shared `.mcp.json`; see [Claude Code](../editors/claude-code.md#mcp-server) for a personal local-scope override |
| `--uninstall` | | Remove everything myceliums installed at that scope |

`myc setup-codex` also writes the routing block into the project's `AGENTS.md` when run inside a git repository (`--no-routing` skips it).

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
| `--http` | string, optional | stdio | Run as Streamable HTTP server on this address (e.g. `127.0.0.1:9999`). Non-loopback addresses are refused without `--allow-remote`. |
| `--allow-remote` | flag | off | Allow a non-loopback `--http` address (e.g. `0.0.0.0:9999`). Every request must then send `Authorization: Bearer <token>`. |
| `--toolsets` | comma-separated list | `mcp.toolsets` config, else `core` | Tool groups to expose: `core` (always on), `graph`, `review`, `architecture`, `knowledge`, `email`, `cross_repo`, `admin`, `deprecated`, or `all`. An unknown name is an error. See [Toolsets](mcp-tools.md#toolsets). |
| `--log-format` | `text` or `json` | `text` | Format of the log lines on stderr; `json` writes one JSON object per line. See [Logging](#logging). |
| `--stats-file` | path, optional | off | Write the [tool-call statistics](#tool-call-statistics) as JSON to this file. |

```bash
# Stdio transport (used by editor integrations), core toolset
myc mcp

# Core plus the architecture and review tools
myc mcp --toolsets core,architecture,review

# HTTP transport, loopback only
myc mcp --http 127.0.0.1:9999

# HTTP transport for remote clients (bearer token required)
MYC_MCP_TOKEN=$(openssl rand -hex 32) myc mcp --http 0.0.0.0:9999 --allow-remote

# JSON logs and a statistics file
myc mcp --log-format json --stats-file ~/.myceliums/mcp-stats.json
```

#### Logging

`myc mcp` logs to **stderr** only; with the stdio transport, stdout carries
the MCP messages and nothing else. Without `RUST_LOG`, the server logs at
INFO for its own crate (`myceliums_mcp`), at WARN for the analysis core
(`myceliums_core`: skipped files, index problems) and errors only for
everything else;
`RUST_LOG` replaces that default (for example `RUST_LOG=debug`, or
`RUST_LOG=error` to silence the per-call lines). Colour codes are written
only when stderr is a terminal.

Every tool call runs in one `tool_call` span (target `myceliums_mcp::calls`)
and ends with one line — INFO `tool call finished` for a result, WARN
`tool call failed` for a JSON-RPC error or a call dropped unanswered:

```text
2026-11-19T02:09:17.379749Z  INFO tool_call{tool=find_callers toolset=core repo=my-app-1a2b3c4d duration_ms=597.33 result_bytes=170 approx_tokens=43 outcome=ok}: tool call finished
```

| Field | Meaning |
|-------|---------|
| `tool`, `toolset` | The tool called and its [toolset](mcp-tools.md#toolsets) (`-` for an unknown name) |
| `repo` | The `repo_id` argument; absent when the tool uses the working directory's repository, and for `analyze` (its `path` is a directory, not a repository id) |
| `duration_ms` | Time from receiving the request to its answer |
| `result_bytes` | Bytes of result text the agent reads (the text blocks; the JSON of `structuredContent` when there are none; the message of a protocol error) |
| `approx_tokens` | `result_bytes / 4`, rounded up — a rough size, not a tokenizer count |
| `outcome` | `ok`, `is_error` (an `isError` result), `protocol_error` (`invalid_params`, `internal_error`, unknown tool) or `cancelled` (the request was dropped before it answered) |

With `--log-format json`, each line is a JSON object; the call fields are in
`span`:

```json
{"timestamp":"2026-11-19T02:09:19.803141Z","level":"INFO","fields":{"message":"tool call finished"},"span":{"approx_tokens":81,"duration_ms":0.27,"outcome":"ok","result_bytes":321,"tool":"list_repos","toolset":"core","name":"tool_call"}}
```

#### Tool-call statistics

The server counts its tool calls in memory: per tool the number of calls,
`isError` results, protocol errors and cancelled calls, p50/p95/max latency,
and the result bytes with their approximate tokens. Nothing is sent over the
network — there is no telemetry. Two ways to read the numbers, both opt-in:

- the `server_stats` tool in the `admin` toolset (`myc mcp --toolsets admin`),
  see [`server_stats`](mcp-tools.md#server_stats);
- `--stats-file <path>`: the same numbers as JSON, rewritten every 30 seconds
  while calls come in and once more when the server stops (stdin closed, or
  Ctrl-C for `--http`). The file is replaced atomically (write to
  `<path>.tmp`, then rename).

Memory stays bounded: one entry per tool of the catalog (any other name is
counted under `(unknown tool)`), and the percentiles are computed over the
last 1024 calls of each tool. The counters start at zero with each server
process; with `--http` they cover all sessions together.

Access control for the HTTP transport:

- **Loopback by default.** The `Host` header must be `localhost`, `127.0.0.1`
  or `[::1]` (this blocks DNS rebinding).
- **Origin.** A request whose `Origin` header is present and is not a loopback
  origin is rejected with 403, as the MCP specification requires. CORS headers
  are only sent to loopback origins.
- **Bearer token.** With `--allow-remote`, a token is required: the value of
  `MYC_MCP_TOKEN` (surrounding whitespace is trimmed; at least 32 characters
  in this mode), or a generated one that is printed once at startup. On
  loopback, the token is enforced only when `MYC_MCP_TOKEN` is set.
- The transport is plain HTTP. Put a TLS-terminating proxy in front of it on
  untrusted networks.

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

The server listens on `127.0.0.1` only and rejects requests whose `Host` is
not loopback (DNS rebinding) or whose `Origin` is a foreign site, so other web
pages cannot call its API (for example `POST /api/llm/config`).

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
