# MCP Tools Reference

Complete reference for all tools available via the Myceliums MCP server (`myc mcp`). These tools are exposed to AI coding assistants through the Model Context Protocol.

## Search

Tools for finding code entities, symbols, and content across the knowledge graph.

### `context_search`

Search for functions, classes, and symbols in the knowledge graph. Preferred over grep for finding code entities. Returns structured results with file locations, types, and relevance scores.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Search query string |
| `repo_id` | string | no | most recent | Repository ID |
| `limit` | integer | no | `20` | Maximum results to return |
| `explain` | bool | no | `false` | Show scoring breakdown and graph paths for each result |

### `semantic_search`

Search for symbols using semantic similarity (vector embeddings). Returns symbols most similar in meaning to the query. Requires embeddings to have been generated during analysis.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Search query (natural language) |
| `repo_id` | string | no | most recent | Repository ID |
| `limit` | integer | no | `10` | Maximum results to return |
| `explain` | bool | no | `false` | Show scoring breakdown |

> **Partial-index warning.** `semantic_search` can only return symbols that were
> embedded. If some symbols failed to embed during analysis, the response is
> prefixed with a warning (`⚠ index partially embedded: N of M symbols …`) so
> callers know results may be incomplete. See [Partial indexes](#partial-indexes).

### `hybrid_search`

Search using hybrid BM25 + vector semantic search with Reciprocal Rank Fusion for better search quality. Combines text matching and semantic similarity.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Search query string |
| `repo_id` | string | no | most recent | Repository ID |
| `limit` | integer | no | `20` | Maximum results to return |
| `rerank` | bool | no | `false` | Apply cross-encoder reranking to results |
| `explain` | bool | no | `false` | Show scoring breakdown and graph paths |

> **Partial-index warning.** Like `semantic_search`, the vector half of
> `hybrid_search` only sees embedded symbols. A partial index prepends a warning
> to the response. See [Partial indexes](#partial-indexes).

#### Partial indexes

Both `semantic_search` and `hybrid_search` read embedding accounting recorded at
index time (no per-query vector scan). When `symbols_embedded < symbols_total` —
because embedding generation or storage failed for some symbols — the search
response is prefixed with:

```
⚠ index partially embedded: 900 of 1240 symbols have vectors (340 embedding failures); un-embedded symbols are invisible to semantic and hybrid search
```

The `analyze` tool response surfaces the same accounting (embedded/total symbols
and an explicit failure line). To make partial indexes a hard failure in CI, run
`myc analyze --strict-embeddings` (see [commands reference](commands.md#embedding-accounting)).

### `search_documents`

Search through all analyzed code content using BM25 text search. Functionally equivalent to `context_search`.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Search query string |
| `repo_id` | string | no | most recent | Repository ID |
| `limit` | integer | no | `20` | Maximum results to return |
| `explain` | bool | no | `false` | Show scoring breakdown |

### `search_emails`

Search indexed emails by keyword, with optional person and date filters. Returns matching Email symbols with subject, sender, date, and body snippet.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Search keyword to match in email subject or body |
| `repo_id` | string | no | most recent | Repository ID |
| `person` | string | no | | Filter by person email address |
| `date` | string | no | | Filter by date (ISO 8601 prefix, e.g., `"2026-04"`) |
| `limit` | integer | no | `20` | Maximum results to return |

### `query_knowledge`

Query cross-domain knowledge: find emails and documents that mention code symbols. Returns source citations with exact line numbers and context snippets. Useful for discovering how code is discussed in documentation and communication channels.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Natural language query or symbol name |
| `repo_id` | string | no | auto-detects | Repository ID or path |
| `include_sources` | bool | no | `true` | Include source citations with line numbers and context |
| `limit` | integer | no | `20` | Maximum results to return |

---

## Symbol Navigation

Tools for exploring individual symbols, their relationships, and source code.

### `symbol_context`

Get a symbol's full context: source code, callers, and callees. Use this to understand how a function is used before modifying it. Reveals dependencies that file reading alone cannot.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Name or qualified name of the symbol |
| `repo_id` | string | yes | | Repository ID |

### `get_symbol_definition`

Get the complete definition and source code of a symbol. Use to understand what a function or class does before using or modifying it.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Symbol name or qualified name |
| `repo_id` | string | yes | | Repository ID |

### `get_callers`

Find all functions that call a given symbol, with optional depth limit. Uses BFS traversal for transitive callers. Use to understand impact of changes or find usage patterns.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Symbol name or qualified name |
| `repo_id` | string | yes | | Repository ID |
| `max_depth` | integer | no | `3` | Maximum transitive depth |

### `get_callees`

Find all functions called by a given symbol, with optional depth limit. Use to understand dependencies and call chains.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Symbol name or qualified name |
| `repo_id` | string | yes | | Repository ID |
| `max_depth` | integer | no | `3` | Maximum transitive depth |

### `get_file_symbols`

List all symbols defined in a specific file with kinds and signatures. Use for file-level navigation and understanding file contents.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `file_path` | string | yes | | File path to list symbols for |
| `repo_id` | string | yes | | Repository ID |

### `find_path`

Find the shortest path between two symbols in the knowledge graph using BFS across all relationship types (CALLS, CONTAINED_BY, IMPORTS, etc.). Answers "how are these two things connected?" directly.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `from_symbol` | string | yes | | Start symbol name or qualified name |
| `to_symbol` | string | yes | | End symbol name or qualified name |
| `repo_id` | string | no | most recent | Repository ID |
| `max_depth` | integer | no | `10` | Maximum BFS depth |

### `rename_symbol`

Preview renaming a symbol across the codebase. Returns a rename plan with all edits needed. Does not modify files.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Name of the symbol to rename |
| `new_name` | string | yes | | New name for the symbol |
| `repo_id` | string | yes | | Repository ID |

### `get_git_context`

Get git ownership and history metadata for a symbol. Returns last author, modification date, commit count, and age in days.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Symbol name to look up |
| `repo_id` | string | yes | | Repository ID |

### `get_rationale`

Get design rationale comments (`NOTE:`, `HACK:`, `WHY:`, `TODO:`, `FIXME:`, `IMPORTANT:`) linked to a symbol or file. Use to understand why code was written a certain way. Provide either `symbol_name` or `file_path`, not both.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `symbol_name` | string | no | | Symbol name or qualified name |
| `file_path` | string | no | | File path to get all rationale comments for |

---

## Graph Analysis

Tools for understanding codebase structure, architecture, and quality at a higher level.

### `get_communities`

List all detected communities with summary stats and top symbols. Use to understand code organization and find related modules.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |

### `get_community_detail`

Get full details of a community: member symbols, internal relationships, and entry points. Use to understand community structure before refactoring.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `community_id` | string | yes | | Community UID or label |

### `get_processes`

Get execution flows showing how functions chain together (e.g., request handler, validation, database, response). Use to understand architecture and data flow before refactoring.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `entry` | string | no | | Filter by entry point name (case-insensitive substring) |
| `filter` | string | no | | Filter by keyword in process description/flow |
| `limit` | integer | no | all | Limit number of processes |
| `min_steps` | integer | no | | Show only processes with N or more steps |

### `get_stats`

Get codebase statistics: symbol counts by kind, files, relationships, languages, and communities. Use to understand overall codebase structure.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |

### `get_god_nodes`

Identify the highest-degree symbols (god nodes) in the call graph. Returns the top-N most connected symbols ranked by total incoming + outgoing CALLS edges. High-coupling nodes (degree > threshold) are flagged as architectural bottlenecks.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `top_n` | integer | no | `10` | Number of top nodes to return |
| `coupling_threshold` | integer | no | `20` | Degree above which nodes are flagged as high-coupling |

### `get_surprising_connections`

Detect surprising cross-community CALLS edges: connections between symbols in different Leiden communities that rarely interact. Ranked by surprise score (0 to 1); higher means more isolated/unexpected coupling. Use to find hidden architectural dependencies.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `min_surprise_score` | float | no | `0.1` | Minimum surprise score to include |
| `limit` | integer | no | `50` | Maximum connections to return |

### `find_dead_code`

Find symbols with no incoming function calls (potential dead code). Exclude common entry points with `exclude_patterns`. Use before cleanup/refactoring.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `exclude_patterns` | string | no | | Comma-separated patterns for known entry points to exclude |
| `limit` | integer | no | all | Limit number of results |

### `get_knowledge_gaps`

Detect structural weaknesses in the codebase: untested code (functions with no test callers), isolated modules (communities with few external connections), documentation gaps (files with many symbols but no rationale/doc nodes), and single points of failure (symbols that are the only bridge between communities). Use to prioritize testing, documentation, and refactoring efforts.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `category` | string | no | all categories | Filter: `"untested"`, `"isolated"`, `"undocumented"`, `"single_point_of_failure"` |

### `get_centrality_report`

Compute centrality metrics (degree, betweenness, closeness, eigenvector) for all symbols in the call graph. Returns the top-N symbols ranked by the chosen metric. Betweenness identifies bridge/bottleneck symbols, closeness measures how central a symbol is, eigenvector highlights symbols connected to other important symbols.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `metric` | string | no | `"betweenness"` | Sort by: `"degree"`, `"betweenness"`, `"closeness"`, `"eigenvector"` |
| `top_n` | integer | no | `15` | Number of top nodes to return |

### `get_community_metrics`

Compute quality metrics for the Leiden community partitioning: overall modularity score (higher = better separation), per-community cohesion (internal edge density), and inter-community coupling (edge counts between community pairs). Use to assess code architecture quality.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `detect_circular_dependencies`

Detect circular dependencies using Tarjan's strongly connected components algorithm. Returns groups of symbols that form dependency cycles. Use to find architectural issues like mutual imports or call cycles.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `include_calls` | bool | no | `true` | Include CALLS edges in cycle detection |
| `include_imports` | bool | no | `true` | Include IMPORTS edges in cycle detection |
| `min_cycle_size` | integer | no | `2` | Minimum number of symbols in a cycle to report |

### `get_dependencies`

Compute file-level dependencies: direct imports, transitive closure (all files reachable via import chains), and reverse dependents (files that import this file). Use before refactoring to understand the full impact of moving or deleting a file.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `file_path` | string | yes | | File path to analyze |
| `max_depth` | integer | no | unlimited | Maximum transitive depth |

### `get_module_coupling`

Compute module-level coupling metrics (afferent Ca, efferent Ce, instability I) for all files or directories. Instability ranges from 0 (maximally stable, many dependents) to 1 (maximally unstable, depends on many others). Use to find fragile modules.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `group_by_directory` | bool | no | `false` | Group by directory instead of individual files |
| `limit` | integer | no | `30` | Maximum results to return |

### `quality_hotspots`

Identify refactoring hotspots by combining graph centrality, git churn, and module instability into a composite score. High-scoring symbols are architecturally critical AND frequently changed, making them prime candidates for refactoring.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `top_n` | integer | no | `20` | Number of top hotspots to return |

---

## Code Review

Tools for reviewing code changes and understanding their impact.

### `detect_impact`

Analyze the impact of code changes before committing. Traces changed symbols through the call graph to find indirectly affected code. Use this proactively when modifying functions to catch unintended side effects.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `diff` | string | no | runs `git diff HEAD` | Diff string |
| `depth` | integer | no | `2` | Graph traversal depth |

### `get_review_context`

Get a compact structural summary of code changes for efficient review. Analyzes a diff to identify changed symbols, their callers and callees, and affected communities. Returns signatures instead of full source to minimize token usage.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `diff` | string | no | runs `git diff HEAD` | Diff string |
| `repo_id` | string | no | most recent | Repository ID |
| `depth` | integer | no | `1` | Graph traversal depth for blast radius |
| `include_source` | bool | no | `false` | Include full source code of changed symbols |

### `get_suggested_questions`

Auto-generate contextual code review questions based on code graph structure and git diff. Returns ranked questions about potential issues like missing test coverage, high caller counts, or API contract violations.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `diff` | string | no | runs `git diff HEAD` | Diff string |
| `limit` | integer | no | `5` | Maximum number of questions to return |

---

## Architecture

Tools for high-level architectural analysis, linting, and visualization.

### `architecture_lint`

Run architecture quality checks: circular dependencies, god nodes, high fan-out, unstable dependencies. Returns findings with severity levels and affected entities.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `rules` | string | no | all rules | Comma-separated rule IDs: `circular_dependency`, `god_node`, `high_fan_out`, `unstable_dependency` |
| `god_node_threshold` | integer | no | `20` | God node degree threshold |

### `architecture_view`

Generate a service-level architecture diagram from the knowledge graph. Communities become service nodes, cross-community edges become connections. Returns both structured JSON and a Mermaid diagram string.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `detect_architecture_drift`

Detect architectural drift by comparing the current knowledge graph against the last saved snapshot. Returns a drift score (0 to 100, higher = less drift) and details on structural changes.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `get_graph_diff`

Compare the current knowledge graph against the last stored snapshot to detect architectural drift. Shows new/removed symbols and relationships since the last analysis that saved a snapshot.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `snapshot_diff`

Compare two graph snapshots to see architectural changes over time. Shows added/removed symbols and relationships. Defaults to comparing the two most recent snapshots.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `from_snapshot` | string | no | second-to-latest | Snapshot ID to compare FROM |
| `to_snapshot` | string | no | latest | Snapshot ID to compare TO |

### `export_mermaid`

Export the knowledge graph as a Mermaid diagram. Supports flowchart (call graph), class (class hierarchy), and graph (community-grouped) views. Returns a Mermaid-syntax string.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `diagram_type` | string | no | `"flowchart"` | Diagram type: `"flowchart"`, `"class"`, or `"graph"` |

### `get_contracts`

Detect API contracts (OpenAPI, Protobuf) in the repository and match endpoints to handler symbols. Returns linked and unlinked endpoints.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `get_ownership`

Resolve file ownership from CODEOWNERS rules. Parses `.github/CODEOWNERS` or `CODEOWNERS` and matches symbols to their owners.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |

### `map_service`

Assign a human-readable service name to a community. Use with `architecture_view` to create meaningful service labels.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `community_label` | string | yes | | Community label to map |
| `service_name` | string | yes | | Human-readable service name |

### `get_service_map`

List all community-to-service name mappings for a repository.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `get_schema`

Get property definitions and schema information for entity types or edge types in the ontology. Returns detailed information about what properties are expected.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `entity_types` | string | yes | | Comma-separated entity type names (e.g., `"Function,Class,Method"`) |
| `include_edges` | bool | no | `false` | Include edge type schemas |

---

## Decision Records

Tools for recording and managing Architecture Decision Records (ADRs).

### `record_decision`

Create an Architecture Decision Record (ADR). Records architectural decisions with context, rationale, and consequences. Link to code symbols with `link_decision`.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `title` | string | yes | | ADR title |
| `status` | string | no | `"proposed"` | Status: `"proposed"`, `"accepted"`, `"deprecated"`, `"superseded"` |
| `context` | string | yes | | Context and motivation |
| `decision` | string | yes | | The decision made |
| `consequences` | string | no | | Expected consequences |

### `get_decisions`

List Architecture Decision Records (ADRs) for a repository. Optionally filter by status.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `status` | string | no | all | Filter by status |

### `link_decision`

Link an Architecture Decision Record to a code symbol. Creates a traceability connection between the decision and the code it affects.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `decision_id` | string | yes | | ADR ID to link |
| `symbol_name` | string | yes | | Symbol name to link to |

---

## Advanced

### `analyze`

Analyze a codebase and build its knowledge graph. Parses source files, extracts symbols, resolves call relationships, detects communities, and traces execution flows. Uses cached analysis if fresh enough (set `force=true` to bypass cache).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | string | yes | | Path to the project directory |
| `force` | bool | no | `false` | Force full re-analysis |
| `max_age_minutes` | integer | no | `60` | Maximum cache age in minutes |
| `skip_embeddings` | bool | no | `false` | Skip embedding generation for faster analysis |

The response includes embedding accounting — `symbols_embedded` of `symbols_total`
symbols, plus an explicit failure line when any symbol could not be embedded. A
non-zero failure count means the index is partial and semantic/hybrid search will
omit the un-embedded symbols (see [Partial indexes](#partial-indexes)). The MCP
tool has no strictness knob; use the `myc analyze --strict-embeddings` CLI flag to
fail CI on a partial index.

### `delete`

Delete a repository's analysis data.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID to delete |

### `cypher_query`

Execute a Cypher query against the knowledge graph. Supports `MATCH`, `RETURN`, `WHERE`, `ORDER BY`, `LIMIT`, `SKIP`, `CONTAINS`, `IS NULL`. Write operations are blocked.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Cypher query string |
| `repo_id` | string | yes | | Repository ID |

### `isolate_intent`

Isolate the symbols implementing a specific intent/feature in a repository. Uses hybrid search to find seed symbols, then expands via call graph traversal with community-aware pruning. Returns the IntentSlice: seed symbols, expanded symbols, internal relationships, and structural metadata.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `intent` | string | yes | | Natural language description of the intent/feature |
| `repo_id` | string | yes | | Repository ID |
| `max_symbols` | integer | no | `50` | Maximum symbols to include in the slice |
| `depth` | integer | no | `2` | Call graph expansion depth |

### `differentiate_intent`

Compare how two repositories implement the same intent/feature. Isolates the relevant symbols in each repo, aligns them via embedding similarity, and reports structural differences.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `intent` | string | yes | | Natural language description of the intent |
| `source_repo_id` | string | yes | | Source repository ID (compare FROM) |
| `target_repo_id` | string | yes | | Target repository ID (compare TO) |
| `similarity_threshold` | float | no | `0.65` | Similarity threshold for symbol alignment (0.0 to 1.0) |
| `max_symbols` | integer | no | `50` | Maximum symbols per slice |

### `plan_adaptation`

Generate an actionable adaptation plan for migrating one repository's approach to another. Compares intent implementations, then produces ordered steps with dependency tracking, effort estimates, and risk analysis.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `intent` | string | yes | | Natural language description of the intent |
| `source_repo_id` | string | yes | | Repository whose approach to adapt FROM |
| `target_repo_id` | string | yes | | Repository whose approach to adapt TO |
| `direction` | string | no | `"source_to_target"` | `"source_to_target"` or `"target_to_source"` |
| `max_symbols` | integer | no | `50` | Maximum symbols per slice |

### `get_conversation`

Get a full email thread by conversation symbol UID. Returns all emails in the thread with their relationships.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `conversation_uid` | string | yes | | Conversation symbol UID |
| `repo_id` | string | no | most recent | Repository ID |

### `get_person_context`

Get all emails involving a specific person (sent by or received by). Returns the person's email activity summary.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `person` | string | yes | | Person email address or name |
| `repo_id` | string | no | most recent | Repository ID |
| `limit` | integer | no | `50` | Maximum results to return |
