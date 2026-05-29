# Frequently Asked Questions

## Installation and Setup

### 1. `myc analyze` takes very long on first run

The first analysis downloads the fastembed model (~100 MB). This is a one-time cost and the model is cached in `~/.myceliums/` for all future runs.

If you want to skip embedding generation entirely (faster, but disables semantic search), run:

```bash
myc analyze ./my-project --skip-embeddings
```

### 2. Can I use myceliums without a git repository?

Yes. Pass the `--no-git-check` flag:

```bash
myc analyze ./my-project --no-git-check
```

Without git, the cache layer uses file timestamps instead of git diffs to detect changes. Watch mode also works in non-git directories.

### 3. How do I set up myceliums for CI/CD?

Use `--skip-embeddings` for speed and `--timeout` to prevent runaway jobs:

```bash
myc analyze ./repo --skip-embeddings --timeout 300
```

This gives you the full knowledge graph (symbols, dependencies, communities) without the overhead of generating vector embeddings.

---

## Configuration

### 4. How do I exclude files or directories?

Add exclude patterns to your `.myceliums.toml` in the project root:

```toml
[analysis]
exclude = [
  "vendor/**",
  "generated/**",
  "**/*.min.js",
]
```

Common directories like `node_modules/`, `.git/`, and `target/` are excluded by default.

### 5. What is the difference between `min_community_size` and `resolution`?

These are two separate knobs for community detection:

- **`min_community_size`** filters out communities below a certain number of symbols. Set it higher to hide small, noisy clusters.
- **`resolution`** controls the granularity of the community algorithm. Higher values produce more (smaller) communities. Lower values merge symbols into fewer (larger) groups.

Configure both in `.myceliums.toml`:

```toml
[communities]
min_community_size = 3
resolution = 1.5
```

---

## Usage

### 6. What is the difference between `search`, `semantic-search`, and `hybrid`?

Each search mode uses a different strategy:

| Command | Method | Best for |
|---------|--------|----------|
| `myc search` | BM25 keyword matching | Exact names, known identifiers |
| `myc semantic-search` | Vector embeddings | Natural language queries, fuzzy concepts |
| `myc hybrid-search` | Both combined via RRF | General-purpose, best overall recall |

Example:

```bash
myc search "validateUserInput"
myc semantic-search "where does authentication happen"
myc hybrid-search "rate limiting middleware"
```

### 7. How do I query the knowledge graph with Cypher?

Use the `query` subcommand with a Cypher string:

```bash
myc query 'MATCH (s:Function) WHERE s.name = "login" RETURN s'
```

More examples:

```bash
# Find all structs that have a field named "id"
myc query 'MATCH (s:Struct)-[:HAS_FIELD]->(f) WHERE f.name = "id" RETURN s.name, s.file'

# Find callers of a function
myc query 'MATCH (caller)-[:CALLS]->(target:Function {name: "processOrder"}) RETURN caller.name'
```

See [docs/reference/cypher-queries.md](reference/cypher-queries.md) for the full schema and more query patterns.

### 8. Does myceliums save tokens automatically?

No. The AI assistant must actively call myceliums tools (like `context_search` or `symbol_context`) instead of reading files with grep and cat. Myceliums returns structured results with exact locations, types, and relationships in a single call, which is far more token-efficient than multiple rounds of file reading.

The setup wizard can add instructions to your editor configuration that guide the AI to prefer myceliums tools. See [docs/guides/token-savings.md](guides/token-savings.md) for details on how this works and how to measure the savings.

---

## Performance and Storage

### 9. How much disk space does myceliums use?

Roughly 5 to 200 MB depending on codebase size and whether embeddings are enabled. Check your current usage with:

```bash
myc status
```

### 10. Analysis is slow on a large codebase

Three things to try:

1. **Skip embeddings** if you only need the graph, not semantic search:
   ```bash
   myc analyze ./my-project --skip-embeddings
   ```

2. **Exclude irrelevant directories** in `.myceliums.toml`:
   ```toml
   [analysis]
   exclude = ["docs/**", "test/fixtures/**", "**/*.generated.*"]
   ```

3. **Lower the max file size** to skip large vendored or generated files:
   ```toml
   [analysis]
   max_file_size_kb = 256
   ```

### 11. How do I remove old repository data?

```bash
# Remove data for a specific repo
myc clean my-old-project

# Remove orphaned data (repos that no longer exist on disk)
myc clean --orphans

# Remove everything and start fresh
myc clean --all
```

---

## Integration

### 12. Which editors are supported?

Myceliums supports 14 editors through MCP integration:

Claude Code, Cursor, VS Code, Windsurf, Zed, JetBrains IDEs, Continue, Gemini CLI, Codex, GitHub Copilot, Aider, Kiro, Spacebot, and OpenClaw.

Run `myc setup` to auto-detect which ones are installed and configure them all at once.

### 13. What is the SessionStart hook?

This is a Claude Code-specific feature. When enabled, Claude Code runs `myc session` automatically when the editor starts. This keeps the knowledge graph fresh without you having to remember to re-analyze.

The hook includes a 5-minute timeout to prevent blocking editor startup on very large codebases.

You can enable it during `myc setup` or add it manually to your Claude Code hooks configuration.

### 14. Can I use myceliums with multiple projects?

Yes. Each project gets its own entry in `~/.myceliums/repos.json`. Just navigate to a project directory and run `myc analyze .` there. Myceliums tracks each project independently.

Switch between projects by navigating to the project directory. The CLI and MCP tools automatically use the data for the current working directory.

---

## Troubleshooting

### 15. `myc doctor` reports issues

Run the suggestions that `myc doctor` outputs. The most common fixes are:

- **Stale or orphaned data:** Run `myc clean --orphans` to remove entries for repos that no longer exist on disk.
- **Permission errors:** Check that `~/.myceliums/` is writable by your user.
- **Outdated index:** Re-run `myc analyze .` in the affected project to rebuild the knowledge graph.

If `myc doctor` passes but you still see unexpected behavior, try a full reset:

```bash
myc clean --all
myc analyze ./my-project
```
