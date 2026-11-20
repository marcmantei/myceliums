# MCP Tools Reference

Complete reference for all tools available via the Myceliums MCP server (`myc mcp`). These tools are exposed to AI coding assistants through the Model Context Protocol.

## Toolsets

By default the server exposes only the **core** toolset: nine tools named after
the questions agents ask about code. Everything else is opt-in, by group:

```bash
myc mcp                                    # core only (default)
myc mcp --toolsets core,architecture       # core plus the architecture tools
myc mcp --toolsets all                     # every tool, deprecated aliases included
myc configure --set mcp.toolsets=graph,review   # default for `myc mcp` without --toolsets
```

`core` is always on; `--toolsets` adds groups to it and overrides the
`mcp.toolsets` config key. An unknown name is an error that lists the valid
ones.

| Toolset | Tools |
|---|---|
| `core` | `search_code`, `explain_symbol`, `find_callers`, `find_callees`, `impact_of_change`, `trace_flow`, `find_dead_code`, `list_repos`, `analyze` |
| `graph` | `get_file_symbols`, `find_path`, `cypher_query`, `get_processes`, `get_communities`, `get_community_detail`, `get_stats`, `get_schema` |
| `review` | `get_review_context`, `get_suggested_questions`, `rename_symbol` |
| `architecture` | `get_god_nodes`, `get_centrality_report`, `get_community_metrics`, `detect_circular_dependencies`, `get_dependencies`, `get_module_coupling`, `get_surprising_connections`, `quality_hotspots`, `architecture_lint`, `architecture_view`, `export_mermaid`, `detect_architecture_drift`, `get_ownership`, `get_git_context`, `get_contracts`, `map_service`, `get_service_map` |
| `knowledge` | `query_knowledge`, `get_rationale`, `get_knowledge_gaps`, `record_decision`, `get_decisions`, `link_decision` |
| `email` | `search_emails`, `get_conversation`, `get_person_context` |
| `cross_repo` | `isolate_intent`, `differentiate_intent`, `plan_adaptation` |
| `admin` | `delete`, `server_stats` |
| `deprecated` | the old names listed under [Deprecated aliases](#deprecated-aliases) |

**Loaded up front in Claude Code.** Claude Code defers MCP tools behind tool
search by default, and the agent-level pilot (#99) saw agents call a
myceliums tool in 0 of 90 sessions. The core tools therefore carry
`"_meta": {"anthropic/alwaysLoad": true}` in `tools/list`, which makes Claude
Code load them into context at session start
([docs](https://code.claude.com/docs/en/mcp)); the other groups stay deferred.
At user scope, `myc setup-claude` also sets `"alwaysLoad": true` on the
server entry when `mcp.toolsets` is the default, so Claude Code waits for the
server and the core tools are present in the first prompt
(`--no-always-load` writes `false`, which defers the core tools as well;
Claude Code 2.1.287 or later). A project's shared `.mcp.json` never carries
the key: the installer's `mcp.toolsets` says nothing about a teammate's, so
there the per-tool `_meta` does the work (see
[Claude Code integration](../editors/claude-code.md#mcp-server) for a
personal local-scope override). Do not set `"alwaysLoad": true` on the server
entry together with `--toolsets all`: that loads all 62 tools up front.

The core tools resolve names and walk the graph through
`myceliums_core::navigation`, like `myc callers`, `myc callees` and
`myc impact`, so the CLI and the MCP tools give the same answer for the same
name. Beyond core's resolution (qualified name, short name, qualified suffix
such as `Item.AsString`; functions and types before same-named variables),
the MCP tools accept `file_path:qualified_name` handles, list those handles
for an ambiguous name, and suggest similar names for an unknown one. Several
definitions with the same handle (overloads in one file) are ambiguous too,
as in the CLI; they are told apart as `file_path:qualified_name:line`.

## Tool contract

These rules hold for every tool; the per-tool sections below list only what is specific.

**Errors.** A failure the agent can act on is returned as a normal tool result
with `isError: true`. Its text names the problem and ends with a `Next:` line
naming the next step — for example `Symbol `parse` is ambiguous: 2 matches.` /
`Next: pass one of these as the symbol name: `src/a.ts:parse`, `src/b.ts:parse``, an unknown
`repo_id` (lists the known ids), a missing snapshot, or a file that is not in
the index. Bad arguments — a malformed `repo_id`, an unparsable or writing
Cypher query, a value outside an enum, a missing or empty required argument,
mutually exclusive options — are JSON-RPC `invalid_params` errors. Only genuine
server faults (I/O, storage) are `internal_error`.

**Output.** A tool whose answer is a report or prose returns it as plain text
content — no JSON envelope, no `structuredContent`, no output schema. A tool
whose answer is a record or a list (`find_callers`, `find_callees`,
`find_dead_code`, `list_repos`, `get_stats`, `get_file_symbols`,
`get_communities`, `get_community_detail`, and the deprecated `get_callers`,
`get_callees`, `get_symbol_definition`) returns typed `structuredContent`
with an `outputSchema` describing it, plus the same JSON as text content, as
the MCP specification recommends.

**Symbols.** Every symbol parameter accepts a short name, a qualified name, or
`file_path:qualified_name`. When a name matches several symbols — two files
that each define a top-level `parse` share both names — the call fails with
the candidates in the `file_path:qualified_name` form (with `:line` for
overloads in one file), and `rename_symbol` never guesses either.

**Output budget.** Every list-returning tool takes `limit` and `offset`. Each
tool has a default page size and a hard maximum (both stated in its input
schema; a larger `limit` is clamped, `offset` is capped at 100,000). Lists
that the core builds from hash maps are sorted before paging, so pages never
overlap or skip. When a list is cut, the result says so —
`Showing 1-20 of 312 — call again with offset=20 for more.` (structured
results carry the same text in a `notice` field) — so a cut is never silent.
Top-N rankings that cannot know their total say `more exist` instead.
`search_code` in hybrid mode pages within its top 100 results (with `rerank`,
within the top 50, which are reranked whatever the page). `impact_of_change`
pages one list of changed symbols (or dependents), affected symbols and files. `cypher_query` pages its rows
the same way (default 50, max 500), and `GROUP BY` results keep a fixed
order; in
multi-column rows a value longer than 60 characters is cut with a visible `…`
and a footer explains it.

**Freshness.** Every result read from a repository's graph starts with one
`Index:` line comparing the analyzed commit with the current `HEAD` and
counting files changed since indexing (modified or new untracked files newer
than the analysis start, and deleted files the index still holds). Only files
the analysis would index or holds count — the same selection as `analyze`: no
hidden files or directories (`.mcp.json`, `.claude/`), no `node_modules`,
`target`, `dist`, `build`, `venv`, only extensions it parses,
`.myceliums.toml`'s `include`/`exclude`/`skip_patterns` and size limit applied
(an indexed file that grew past the limit counts: the next analysis drops it) —
plus `.myceliums.toml` itself. The routing block `myc setup-claude` / `myc
setup-codex` keep in a Markdown file (CLAUDE.md / AGENTS.md) is not indexed, so
a Markdown file whose text outside the block is what the analysis indexed is no
change either: `myc analyze` followed by `myc setup-claude` leaves an index
that is up to date. The comparison uses a fingerprint of the indexed text that
the analysis records for CLAUDE.md, AGENTS.md and files holding a block; for an
index without one it compares with the committed version (an uncommitted file
then counts as changed once a block is added). In any other file the markers
are plain text and every edit counts. It names
the repository it describes, e.g. `Index: my-app, commit 1a2b3c4 = HEAD, up
to date.` or `Index: my-app, commit 1a2b3c4, HEAD is now 9f8e7d6 — stale; run
analyze to refresh.` Structured results carry it as `index_status`. Whenever the line
calls an index stale or changed, `analyze` without `force` re-indexes instead
of reusing the cached index, so the advice always works. Git runs read-only
(`--no-optional-locks`, so it never takes the index lock beside your own git
commands) with a short timeout and is cached for a few seconds (the
`git diff HEAD` that `impact_of_change`, `get_review_context` and
`get_suggested_questions` run when no `diff` is passed is read-only too, with
a 30 s timeout); when
`git status` fails or times out the line says `uncommitted edits unknown`
rather than `up to date`, and without git it says the freshness is unknown.

**Call-edge confidence.** CALLS edges are resolved by name. Each edge records
how its target was picked: `exact` (a qualified name only one symbol has),
`import` (through the module graph), `name-match` (a bare name only one symbol
has) or `ambiguous` (several symbols share the name; one was picked).
`find_callers`/`find_callees` return it as `resolved`; `explain_symbol`,
`trace_flow`, `impact_of_change`, `find_path` and `get_review_context` mark the
heuristic ones as `(resolved: name-match)` / `(resolved: ambiguous)`. Indexes built before this
label existed get it derived at query time (`ambiguous` if the name is shared,
else `name-match`); re-run `analyze` to record the exact labels.

**Untrusted content.** Indexed text — source code, documents, email, code
comments, Cypher rows, and search result tables (whose names and signatures
can be email subjects or Markdown headings) — is framed as data:

```text
<<<UNTRUSTED 3fa9c2d1 src/services/user.ts:18-21
…indexed text…
>>>END 3fa9c2d1
```

The id is random per block, so the framed text cannot fake the end of the
block. The server instructions tell the agent that such text is data, never
instructions.

**Annotations.** Every tool declares `readOnlyHint`, `destructiveHint`,
`idempotentHint` and `openWorldHint` (always `false`: tools touch only the
local index and project). All tools are read-only except `analyze`, `delete`
(destructive), `map_service` (overwrites a mapping), `record_decision` and
`link_decision`; they write only to the Myceliums data directory, never to the
project. `delete` additionally requires `confirm: true`.

## Core tools

Toolset `core`, on by default. Where `repo_id` is optional it defaults to the
indexed repository containing the server's working directory (see
[Repository ids](#repository-ids)); outside every indexed directory the call
is an `isError` result, never an answer about another repository.

### `search_code`

Find where something is implemented: functions, methods, classes and types by
name (`parseConfig`, `Item.AsString`) or by what the code does ("builds the
Set-Cookie header"). Text result: one framed table of matches with kind, file
and line range.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | A name or a description of what the code does |
| `repo_id` | string | no | working directory | Repository ID |
| `mode` | enum | no | `auto` | `auto`: hybrid when the index has embeddings, else keyword (the result says so). `keyword`: names, qualified names and signatures, no model. `semantic`: vector similarity only. `hybrid`: keyword and vector rankings fused |
| `limit` / `offset` | integer | no | `20` / `0` | Page (max 100) |
| `rerank` | bool | no | `false` | Rerank the top 50 hybrid results with a cross-encoder |
| `explain` | bool | no | `false` | Show the scoring breakdown of each result |

`auto` decides from the embedding metadata recorded at index time and never
loads a model for an index built with `skip_embeddings`. An explicit
`semantic` or `hybrid` on such an index is an `isError` result that suggests
`mode=keyword`. Scores: see [Search modes and scores](#search-modes-and-scores).

### `explain_symbol`

A function, method or class with what you need before changing it: source
(framed as untrusted), signature, file and line range, decorators/return
type/superclasses when known, and its direct callers and callees (the first
25 of each; the rest via `find_callers`/`find_callees`). Text result.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Name, qualified name, or `file_path:qualified_name` |
| `repo_id` | string | no | working directory | Repository ID |
| `max_source_lines` | integer | no | `120` | Most source lines shown (max 5000); a longer body is cut, followed by a line naming `max_source_lines=<total>` and the file lines to read for the rest |

### `find_callers`

Who calls this function or method? Structured result (`CallersOutput`): the
callers with name, qualified name, kind, file, line range, `depth`, `via` (for
depth > 1, the name of the symbol one step closer) and `resolved` (the CALLS
edge resolution), ordered by depth, then file and line, plus `total_count`,
`depth_limited`, `index_status` and a paging `notice`. The symbols, depths,
`via` names and order are those `myc callers --json` prints (pinned by an
end-to-end test).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | yes | | Name, qualified name, or `file_path:qualified_name` |
| `repo_id` | string | no | working directory | Repository ID |
| `depth` | integer | no | `1` | Hops to follow; 1 = direct callers only (max 10) |
| `limit` / `offset` | integer | no | `20` / `0` | Page (max 200) |

### `find_callees`

What does this function or method call? Same parameters and shape as
`find_callers` (`CalleesOutput`, list `callees`), following CALLS edges
forwards.

### `impact_of_change`

What could break if I change this? Give at most one of `symbol_name`,
`file_path` and `diff` (more is `invalid_params`). Text result.

* **Symbol or file:** every symbol that calls or imports it (for a file: any
  symbol the file defines), transitively up to `depth`, in sections *Direct
  dependents* (these must change too if a signature changes), *Indirect
  dependents* (with depth and `via`) and *Files with dependents* (nearest
  depth). IMPORTS edges are marked `[imports]`, name-matched CALLS edges
  `(resolved: …)`.
* **Diff (default):** the impact report of the uncommitted diff (`git diff
  HEAD` in the repository) or of the `diff` passed: changed symbols, symbols
  affected through calls and imports in both directions, files, and a risk
  score.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `symbol_name` | string | no | | The symbol you plan to change |
| `file_path` | string | no | | The file you plan to change (repository-relative) |
| `diff` | string | no | `git diff HEAD` | A unified diff |
| `repo_id` | string | no | working directory | Repository ID |
| `depth` | integer | no | `2` | Hops to follow (max 10) |
| `limit` / `offset` | integer | no | `50` / `0` | Page over report lines (max 500) |

### `trace_flow`

How does control get from one function to another? Follows CALLS edges from
`from_symbol` towards `to_symbol`. Text result: the shortest call chain (each
step with kind, file and line; name-matched edges marked), then every symbol
strictly between the two ends that lies on a chain of the shortest length —
or up to `extra_calls` longer — with the chain length and its distance from
the start. When `to_symbol` is not reachable within `max_depth` calls the
result says so and suggests what to try (swap the ends, a larger
`max_depth`, `find_callers`). The same symbol at both ends is
`invalid_params`.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `from_symbol` | string | yes | | Where the flow starts |
| `to_symbol` | string | yes | | Where the flow ends |
| `repo_id` | string | no | working directory | Repository ID |
| `max_depth` | integer | no | `12` | Longest chain searched, in calls (max 30) |
| `extra_calls` | integer | no | `0` | Also list symbols on chains up to this many calls longer than the shortest (max 5) |
| `limit` / `offset` | integer | no | `20` / `0` | Page over the listed symbols (max 200) |

### `find_dead_code`

Which functions and methods are never called? Lists the functions and methods
with no incoming CALLS edge. A candidate list, not proof: entry points,
callbacks, trait/interface implementations and calls the resolver missed look
unused too. Structured result (`DeadCodeOutput`).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | working directory | Repository ID |
| `path` | string | no | | Only this file, or files under this directory: repository-relative or absolute inside the repository, `./` and a trailing `/` allowed; a unique file suffix (`util.go`) names that file. A path that matches no indexed file or directory is an `isError` result, never an empty list |
| `exclude_patterns` | string | no | | Comma-separated substrings; names containing one are skipped |
| `limit` / `offset` | integer | no | `20` / `0` | Page (max 200) |

### `analyze`

Analyze a codebase and build its knowledge graph. Parses source files in 20+ languages plus Markdown, PDF and email, extracts symbols, resolves call relationships, detects communities, and traces execution flows. Uses cached analysis if fresh enough (set `force=true` to bypass cache). Writes only to the Myceliums data directory. A path that does not exist is `invalid_params`.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | string | yes | | Path to the project directory |
| `force` | bool | no | `false` | Force full re-analysis |
| `max_age_minutes` | integer | no | `60` | Maximum cache age in minutes |
| `skip_embeddings` | bool | no | `false` | Embed nothing, for faster analysis. Symbols the previous index already embedded keep their vectors; new and changed ones have none |

The response includes embedding accounting — `symbols_embedded` of `symbols_total`
symbols, plus an explicit failure line when any symbol could not be embedded. A
non-zero failure count means the index is partial and semantic/hybrid search will
omit the un-embedded symbols (see [Partial indexes](#partial-indexes)). The MCP
tool has no strictness knob; use the `myc analyze --strict-embeddings` CLI flag to
fail CI on a partial index.

**A re-index keeps existing embeddings.** A vector depends only on the model
and the symbol's embedding text, so symbols the previous index embedded with
the same model, and whose text is unchanged, take their vectors over instead of
being embedded again (`Embeddings: 1240/1243 symbols (1240 kept from the
previous index)`). That makes a re-index after a small edit embed only the
changed symbols, and it means a run that does not embed never drops the
embeddings it replaces: with `skip_embeddings`, unchanged symbols keep their
vectors and only new or changed ones are left without, reported as
`⚠ 3 of 1243 symbols have no vector (embeddings skipped) …`; when the model
fails to load or embed, only the symbols it had to embed are failures. Vectors
are carried over only from an index at the current vector geometry and
embedding-text template (otherwise the run embeds, or with `skip_embeddings`
leaves every symbol without a vector). A cached
answer for an index built with `skip_embeddings` reports `0/N` symbols and the
same warning instead of claiming a complete index.

**The previous index stays usable.** A full analysis builds the new index in a
staging directory beside the live one (`data/.staging/build-…`) and swaps it in only
when it is complete — in one system call on Linux and macOS — and the graph
cache is dropped at the swap. Until then every tool answers from the previous
index. A failed or cancelled analysis deletes the staging directory and leaves
the previous index as it was; the error says so. Cancelling the request stops
the analyzer at its next check (between files, between phases, before
embedding). A rebuild killed outright is recovered at the next server or `myc`
start (see [Data management](data-management.md#data-directory-datarepo-id)).

**One analysis per repository.** While a repository is being analyzed — by
this server or by another process such as `myc analyze` — another `analyze`
of it returns an `isError` result (`Analysis already running for …`) with a
`Next:` step; it does not wait.

**Off the request path.** The analysis runs on a blocking thread, so other
tool calls are answered while it runs.

**Progress.** When the request carries a `progressToken` in `_meta`, the
server sends `notifications/progress` with `total: 100`, a `progress` that only
increases, and a message: discovering files, `parsing files: n/m`, resolving
calls and embedding, `graph written`, detecting communities, tracing execution
flows, `replacing the previous index`, and `done` at 100. All of them arrive
before the result. Without a token, none are sent. A cached result (index still
fresh) sends none either.

### `list_repos`

List the analyzed repositories, most recently analyzed first. Structured
result (`ReposOutput`): `repos` with `id`, `name`, `path`, `analyzed_at`,
`analyzed_commit` (when the project is a git checkout), `symbols` and
`files`; `total_count`; and a `notice` when the list was cut or is empty.
Read-only; takes only `limit`/`offset`.

### Repository ids

Every `repo_id` parameter is checked before anything touches the disk. The id
must be registered (shown by `list_repos`) and must name a single directory:
it may not be `.` or `..` and may not contain a path separator or NUL (`/` on
macOS/Linux; `/`, `\` and `:` on Windows). A malformed id is an
`invalid_params` error; an unknown one is an `isError` result that lists the
known ids. Neither returns an empty result. Where `repo_id` is optional, the
indexed repository containing the server's working directory is used — the
deepest one, matched on whole path components (`/x/app-two` is not inside
`/x/app`). Outside every indexed directory the call is an `isError` result
(`No index covers <dir>` / `Next: run analyze with path=<dir>, or pass
repo_id`); there is no fallback to another repository. `analyze` derives ids from the folder name and replaces any such
character with `_`, so derived ids always pass.

---

## Search

How `search_code` ranks, and the opt-in email and knowledge search tools.

### Search modes and scores

`search_code` ranks with one of four modes:

* `keyword` — names, qualified names and signatures; needs no embedding
  model, works on every index.
* `semantic` — vector similarity only (below).
* `hybrid` — keyword and vector rankings fused (below).
* `auto` (default) — `hybrid` when the index has embeddings, else `keyword`
  with a first line saying so.

#### `semantic`

**Score semantics.** Results are ranked by **cosine similarity** in the range
`[-1, 1]` (higher = more similar). Embeddings are L2-normalized at both write and
query time, so this is the identical geometry used by the vector leg of
hybrid mode — the same query over the same index yields the same top-k
ordering in both modes.

**Exact search.** Since #117 every vector search is exact: a brute-force
scan over all embedded symbols, at every repository size, with no approximate
(ANN) index. Measured median latency of one search over 384-dimensional
vectors on one Apple-silicon Mac: 2.4 ms at 5,000 symbols and 5.2 ms at 20,000
(real embeddings of Django), 9.5 ms at 50,000 and 15 ms at 100,000 (synthetic
vectors). Cost grows linearly with the number of symbols, so a repository of a
million symbols would take on the order of 150 ms per query (extrapolated, not
measured). Earlier releases built an IVF-PQ index from 5,000 symbols that
returned only about 4 of the true 10 nearest neighbours; searches now bypass
such an index, and the next `myc analyze` deletes it.
[Measurements](../../crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md#ann-recall-what-the-vector-index-costs).

> **Partial-index warning.** Semantic mode can only return symbols that were
> embedded. If some symbols failed to embed during analysis, the response is
> prefixed with a warning (`⚠ index partially embedded: N of M symbols …`) so
> callers know results may be incomplete. See [Partial indexes](#partial-indexes).

#### `hybrid`

**Score semantics.** Each result reports up to three scores on **distinct,
non-interchangeable scales**:

- `combined_score` (a.k.a. *fusion* score) — the Reciprocal Rank Fusion score,
  a rank-based quantity on the order of `1/k`. Present for every result.
- `vector_score` — **cosine similarity** in `[-1, 1]`, the same geometry
  semantic mode exposes. The vector leg of hybrid search ranks by this.
- `bm25_score` — the raw BM25 lexical relevance score (unbounded, higher = more
  relevant).

When `rerank=true`, a cross-encoder model re-scores the fused candidates and its
score is reported in a separate `rerank_score` field. It does **not** overwrite
`combined_score`: fusion and rerank live on different scales, so both remain
observable and no tool ever returns two different scales under the same name.
Results are ordered by `rerank_score` when it is present. Unlike the other
scores it has no fixed range — it is the raw cross-encoder relevance logit, so
compare values only within a single result set, never across queries.

> **Partial-index warning.** Like semantic mode, the vector half of
> hybrid mode only sees embedded symbols. A partial index prepends a warning
> to the response. See [Partial indexes](#partial-indexes).

#### Partial indexes

Semantic and hybrid mode read embedding accounting recorded at
index time (no per-query vector scan). When `symbols_embedded < symbols_total` —
because embedding generation or storage failed for some symbols — the search
response is prefixed with:

```
⚠ index partially embedded: 900 of 1240 symbols have vectors (340 embedding failures); un-embedded symbols are invisible to semantic and hybrid search
```

The `analyze` tool response surfaces the same accounting (embedded/total symbols
and an explicit failure line). To make partial indexes a hard failure in CI, run
`myc analyze --strict-embeddings` (see [commands reference](commands.md#embedding-accounting)).

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

Get git ownership and history metadata for the symbols whose name contains `symbol_name` (case-insensitive). Returns last author, modification date, commit count, and age in days; no match is an `isError` result.

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

### `get_knowledge_gaps`

Detect structural weaknesses in the codebase: untested code (functions with no test callers), isolated modules (communities with few external connections), documentation gaps (files with many symbols but no rationale/doc nodes), and single points of failure (symbols that are the only bridge between communities). Use to prioritize testing, documentation, and refactoring efforts.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `category` | enum | no | all categories | `untested`, `isolated`, `undocumented` or `single_point_of_failure` |

### `get_centrality_report`

Compute centrality metrics (degree, betweenness, closeness, eigenvector) for all symbols in the call graph. Returns the top-N symbols ranked by the chosen metric. Betweenness identifies bridge/bottleneck symbols, closeness measures how central a symbol is, eigenvector highlights symbols connected to other important symbols.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `metric` | enum | no | `betweenness` | Sort by: `degree`, `betweenness`, `closeness` or `eigenvector` |
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
| `rules` | enum list | no | all rules | Rule IDs: `circular_dependency`, `god_node`, `high_fan_out`, `unstable_dependency` (a comma-separated string is still accepted) |
| `god_node_threshold` | integer | no | `20` | God node degree threshold |

### `architecture_view`

Generate a service-level architecture diagram from the knowledge graph. Communities become service nodes, cross-community edges become connections. Returns both structured JSON and a Mermaid diagram string.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |

### `detect_architecture_drift`

What changed in the code structure since the previous analysis? Every
`analyze` (MCP tool or CLI) records a snapshot of the graph; the newest 10 per
repository are kept. This tool compares two recorded states — by default the
analysis before the current one against the current graph, or any two
snapshot ids — and returns a drift score (`100` = unchanged, falling with the
share of symbols and edges added or removed) and the paged list of added and
removed symbols and relationships. Snapshots key symbols by file, qualified
name and kind, so re-analyzing unchanged code compares equal. With no earlier
analysis recorded (an index built before this version has none) the result is
an `isError` that says to run `analyze` again after the code changes. It
replaces `get_graph_diff` and `snapshot_diff`, which could not work: the only
snapshot was overwritten by the analysis that built the current index, it was
keyed by symbol uids that every analysis generates anew, and nothing recorded
the versioned snapshots `snapshot_diff` compared.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | working directory | Repository ID |
| `from_snapshot` | string | no | previous analysis | Baseline snapshot ID |
| `to_snapshot` | string | no | current graph | Snapshot ID to compare against the baseline (without `from_snapshot`: the next older snapshot is the baseline) |
| `limit` / `offset` | integer | no | `50` / `0` | Page over changes (max 500) |

### `export_mermaid`

Export the knowledge graph as a Mermaid diagram. Supports flowchart (call graph), class (class hierarchy), and graph (community-grouped) views. Returns a Mermaid-syntax string.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `diagram_type` | enum | no | `flowchart` | `flowchart`, `class` or `graph` |

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
| `status` | enum | no | `proposed` | `proposed`, `accepted`, `deprecated` or `superseded` |
| `context` | string | yes | | Context and motivation |
| `decision` | string | yes | | The decision made |
| `consequences` | string | no | | Expected consequences |

### `get_decisions`

List Architecture Decision Records (ADRs) for a repository. Optionally filter by status.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | no | most recent | Repository ID |
| `status` | enum | no | all | Only this status (same values as `record_decision`) |

### `link_decision`

Link an Architecture Decision Record to a code symbol. Creates a traceability connection between the decision and the code it affects.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID |
| `decision_id` | string | yes | | ADR id, or a unique prefix of it (`get_decisions` lists ids cut to the shortest unique prefix, at least 8 characters) |
| `symbol_name` | string | yes | | Symbol name to link to |

---

## Advanced

### `delete`

Permanently delete a repository's index (not the project files). Without
`confirm: true` the call is refused with an `isError` result and nothing is
deleted; so is a call while the repository is being analyzed.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `repo_id` | string | yes | | Repository ID to delete |
| `confirm` | bool | no | `false` | Must be `true`; deletion is irreversible (re-run `analyze` to rebuild) |

### `server_stats`

Tool-call statistics of this server process since it started, one line per
tool, busiest first: calls, `isError` results, protocol errors, cancelled
calls, p50/p95/max latency in ms, result bytes and approximate tokens
(bytes / 4). The numbers live in memory on this machine only and are never
sent anywhere; the call that reads them is not yet counted. No parameters.
Toolset `admin`. The same numbers can be written to a file with
`myc mcp --stats-file` (see [Tool-call statistics](commands.md#tool-call-statistics)).
Example (the numbers are illustrative):

```text
Tool calls since this server started 812 s ago: 41 (local only, never sent anywhere).
Latency percentiles cover the last 1024 calls of each tool.

tool | toolset | calls | isError | protocol errors | cancelled | p50 ms | p95 ms | max ms | result bytes | ~tokens
search_code | core | 22 | 1 | 0 | 0 | 38.2 | 120.5 | 131.0 | 61440 | 15360
```

### `cypher_query`

Execute a read-only Cypher query against the knowledge graph. Supports `MATCH`, `RETURN`, `WHERE`, `ORDER BY`, `LIMIT`, `SKIP`, `CONTAINS`, `IS NULL`. Write operations and unparsable queries are `invalid_params` errors. Result rows are paged and framed as untrusted content.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | | Cypher query string |
| `repo_id` | string | yes | | Repository ID |
| `limit` | integer | no | `50` | Rows per page (max 500) |
| `offset` | integer | no | `0` | Rows to skip, for the next page |

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
| `direction` | enum | no | `source_to_target` | `source_to_target` or `target_to_source` |
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

---

## Deprecated aliases

Toolset `deprecated`, part of `all`. **The default profile (`core`) does not
expose these names**: a client that calls one gets "tool not found". To keep
them for this release, add the group — `myc mcp --toolsets core,deprecated`,
or once for every `myc mcp`: `myc configure --set mcp.toolsets=deprecated`
(core is always on) — and move to the replacements below. Each description
starts with `DEPRECATED: use <replacement>`. They are removed in the release
after the one that introduced the core toolset.

| Old name | Use instead | Notes |
|---|---|---|
| `context_search` | `search_code` with `mode=keyword` | Same ranking |
| `search_documents` | `search_code` with `mode=keyword` | It was `context_search` under another name; it never was a BM25 document search |
| `semantic_search` | `search_code` with `mode=semantic` | Default page size stays 10 |
| `hybrid_search` | `search_code` with `mode=hybrid` (or `auto`) | |
| `symbol_context` | `explain_symbol` | Same output; `repo_id` is now optional |
| `get_symbol_definition` | `explain_symbol` | Old structured output kept until removal |
| `get_callers` | `find_callers` | Same output; `max_depth` (default 3) became `depth` (default 1) |
| `get_callees` | `find_callees` | As above |
| `detect_impact` | `impact_of_change` | Diff mode of the new tool |
| `get_graph_diff` | `detect_architecture_drift` | Now compares with the previous analysis |
| `snapshot_diff` | `detect_architecture_drift` | Same `from_snapshot`/`to_snapshot` parameters |
