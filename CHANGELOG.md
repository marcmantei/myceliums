# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries before
0.3.2 were reconstructed from git history; numbers in parentheses are issues or
pull requests.

## [Unreleased]

### Breaking
- **`myc mcp` now exposes the core toolset by default: 9 tools instead of 55** (#97). `search_code`, `explain_symbol`, `find_callers`, `find_callees`, `impact_of_change`, `trace_flow`, `find_dead_code`, `list_repos` and `analyze` are named after the questions agents ask and carry `_meta["anthropic/alwaysLoad"]`, so Claude Code loads them up front instead of deferring them behind tool search (the #99 pilot: 0 of 90 sessions called a myceliums tool). The other tools are opt-in by group — `graph`, `review`, `architecture`, `knowledge`, `email`, `cross_repo`, `admin` — with `myc mcp --toolsets core,architecture,...` or `myc configure --set mcp.toolsets=...`; `--toolsets all` restores every tool. Clients that call a non-core tool by name must add its group.
- Duplicate tools are merged; the old names stay as deprecated aliases in the `deprecated` group (part of `all`) for one release, each description naming its replacement: `context_search`, `search_documents` (which was `context_search` under another name, not a BM25 document search), `semantic_search`, `hybrid_search` → `search_code` with `mode`; `symbol_context`, `get_symbol_definition` → `explain_symbol`; `get_callers`, `get_callees` → `find_callers`, `find_callees`; `detect_impact` → `impact_of_change`; `get_graph_diff`, `snapshot_diff` → `detect_architecture_drift` (#97).
- Text tools return their answer as plain text content; it is no longer wrapped as `{"text": "..."}` JSON and duplicated in `structuredContent`, and these tools have no `outputSchema`. Tools with a natural record or list shape (`find_callers`, `find_callees`, `find_dead_code`, `list_repos`, `get_stats`, `get_file_symbols`, `get_communities`, `get_community_detail`) return typed `structuredContent` with an output schema. `list_repos` is now structured. The `TextOutput` type is gone from the `myceliums-mcp` crate, and `run_mcp_server`/`run_mcp_http_server` take the `Toolsets` to serve (#97).

### Added
- Agent integration kit (#126): `myc callers`, `myc callees`, `myc impact <symbol|file>` and `myc context` print compact, token-budgeted answers with file:line and index freshness (`--json` for harnesses; ambiguous names list candidates); `myc hook-grep` is a never-blocking Claude Code PreToolUse hook that answers a grep for a known symbol with the matching `myc` call; `myc setup-claude` also writes a versioned CLAUDE.md routing block and a `code-explorer` subagent, each with an opt-out flag; `myc setup-codex` writes the routing block into AGENTS.md; `examples/agent-sdk-prefetch/` prefetches a context pack for a Claude Agent SDK agent; guide in `docs/guides/agent-integration.md`. Effects of the layers are not measured yet (#125).
- `myc setup-claude` marks the core tools for up-front loading: the `myceliums` entry gets `"alwaysLoad": true` when `mcp.toolsets` is the default (left out when groups are added; `--no-always-load` writes `false`). At user scope the entry is registered with `claude mcp add-json` so the key is kept. The routing block (v2) and the `code-explorer` subagent (v2) name the core tools (#97).
- The MCP tools resolve names and walk callers, callees, dependents and call chains through `myceliums_core::navigation`, like `myc callers/callees/impact`: qualified suffixes (`Item.AsString`) resolve, functions and types win over same-named variables, and an unknown name suggests similar ones. Core gains `walk_calls_reached`, `dependents_reached` and `call_chains` (#97).
- `search_code` mode `auto` (default): hybrid search when the index has embeddings, keyword search otherwise, without loading a model for an index built with `skip_embeddings` (#97).
- `impact_of_change` answers for a symbol or a file — every caller and importer, transitively, with the files to update — as well as for a diff; `trace_flow` lists the shortest call chain between two symbols and every symbol on such a chain; `find_callers`/`find_callees` report `via` for indirect hits; `find_dead_code` takes a `path` and an optional `repo_id` (#97).
- Agent benchmark (`evals/agent/`): 30 code-understanding tasks on pinned fzf, hono and FastAPI checkouts, run by a headless Claude Code agent with and without the myceliums MCP server and graded against hand-verified answers, plus a tool-selection eval over the live `tools/list`. The first pilot run and its report are committed (#99).
- Retrieval-quality measurement against a golden dataset, reported on every PR (#76, #34).
- The retrieval evaluation scores semantic, hybrid and hybrid+rerank search through the shipped code paths, adds hit@k, nDCG@10 and R-precision with per-intent and per-query results, and reports paired bootstrap confidence intervals. CI caches the model weights, scores all four modes and fails only on a regression of at least 0.02 whose 95% interval lies below zero — per comparison, not corrected for multiple comparisons (#92).
- `ann-recall` measures the recall of the IVF-PQ vector index against exact search (#92).
- Supply-chain checks in CI: `cargo-deny` (advisories, licenses, sources), Dependabot for Cargo and GitHub Actions, an MSRV job (`rust-version = "1.88"`), and the test suite on macOS (#90).
- Release assets now include a CycloneDX SBOM per target and a signed build-provenance attestation (#90).

### Changed
- MCP tool contract (#96): failures the agent can act on (unknown symbol or repo, ambiguous name, missing snapshot, file not indexed, `delete` without `confirm: true`) are `isError` results that end with a `Next:` step; bad arguments are `invalid_params`; only server faults are `internal_error`. Every list tool takes `limit`/`offset` with a default and a hard maximum and says `Showing 1-20 of 312 — call again with offset=20` when it cuts; `cypher_query` pages its rows and marks cut cells with `…`. Every graph result starts with an `Index:` line (analyzed commit vs `HEAD`, files changed since indexing, read with `git --no-optional-locks`); when it calls an index stale, `analyze` without `force` re-indexes. `git diff HEAD` for impact analysis also runs with `--no-optional-locks` and a 30 s timeout (`myceliums_core::git_read`). Symbols can be named `file_path:qualified_name`; a name shared by several symbols is an error listing those handles, also for `rename_symbol`. Lists the core builds from hash maps are sorted before paging. CALLS edges record how they were resolved (`exact`, `import`, `name-match`, `ambiguous`) and tools show it. Indexed source, document and email text is framed in `<<<UNTRUSTED <id>` … `>>>END <id>` blocks. All tools carry read-only/destructive/idempotent/open-world annotations; enum-like parameters are enums and numeric ones have bounds in the schema; stale descriptions and the server instructions (which called every tool read-only) are corrected. Without `repo_id`, tools now fall back to the most recently analyzed repo, as documented, instead of the last one by name.
- `myc setup-claude` installs into the project by default (`.mcp.json`, `.claude/settings.json`, `CLAUDE.md`, `.claude/agents/`), so a team can commit it; `--scope user` keeps the previous user-wide install, which `myc setup` and the wizard use. Uninstall removes exactly the myceliums parts at that scope (#126).
- The CLI (including `myc serve`) honours `MYCELIUMS_DATA_DIR` like the MCP server, so CLI commands and hooks read the same index as a server started in the same environment (#126).
- Vector scores use one similarity scale across search paths, and the ANN index is used (#68, #71).
- **Vector search is exact at every repository size — re-run `myc analyze` once to delete old ANN indexes.** Releases before this one built an IVF-PQ index from 5,000 symbols that returned only about 4 of the true 10 nearest neighbours. Searches now bypass any such index and the next `myc analyze` (or any re-embedding) drops it; no approximate index is built any more. Exact search measured 5.2 ms per query at 20,000 symbols (real embeddings) and 15 ms at 100,000 (synthetic). `analysis.ann_threshold` is deprecated and ignored. `ann-recall --sweep` compares index settings on synthetic and real embeddings, and `cargo test` fails if production search stops being exact (#117).
- MCP handlers go through a `GraphService` layer (#37, #53).
- The MCP crate is split into modules by tool group, and every read tool and resource is served from `GraphService`'s per-repository graph cache: the graph tables are read from disk once, not on every call; `analyze` and `delete` invalidate the cache, and index changes made by another process are detected. The HTTP transport shares one cache across sessions. The `tools/list` output is pinned by a snapshot test (#93).
- The Homebrew tap sync can be re-run manually and is idempotent (#74).
- CI pins every third-party action to a commit SHA, runs with read-only token permissions by default, and builds with `--locked` (#90).
- The index fingerprint covers the version of the embedding-text template and its budget constants. Indexes written by earlier releases are re-embedded on the next full `myc analyze`; until then, incremental re-indexing with embeddings is refused rather than mixing two templates in one index (#92).
- `myc` is installed from crates.io; the unpublished Docker image is no longer advertised (#72, #73).

### Fixed
- Graph diffs never worked: every analysis overwrote the only snapshot, keyed by symbol uids that each analysis generates anew, and nothing wrote the versioned snapshots `snapshot_diff` compared. Each analysis now records a versioned snapshot (the newest 10 are kept) keyed by file, qualified name and kind, and `detect_architecture_drift` compares the previous analysis with the current graph by default (#97).
- The Claude Code PostToolUse hook read tool results as `{"text": ...}` JSON and their first line, which since #96 is the `Index:` line; it now reads plain text, skips the freshness line, and knows the new tool names (#97).
- Cypher `GROUP BY` on a property (`RETURN s.kind, count(s)`) grouped every row under `null`, and groups came out in a different order on every run; they are now keyed by the evaluated property and keep first-seen order (#96).
- The MCP `detect_impact` and `rename_symbol` outputs listed only affected files and `Rename: ? -> …` because they read JSON keys that do not exist; they now render the changed and affected symbols and each edit (#96).
- The PostToolUse hook that `myc setup-claude` installs now matches myceliums tools: Claude Code compares a matcher of only letters and `_` as an exact tool name, so the old `mcp__myceliums__` matched nothing; it is now `mcp__myceliums__.*` (#126).
- `myc setup-claude` refuses to rewrite a settings or MCP config file it cannot parse instead of replacing it with its own content (#126).
- `--strict-embeddings` is honored in the session bootstrap path (#69).
- The `embeddings` feature gate lost in #43 is restored (#70).
- Benchmark baselines are content-addressed instead of tied to a git commit (#86).
- Lint findings in test code, now that clippy covers `--all-targets` (#81–#85, #87).
- Hybrid search returns tied results in a fixed order instead of a per-process random one (#92).
- The manual benchmarks workflow no longer hides failing benches behind `continue-on-error` (#92).

### Removed
- The `benchmark` job in `ci.yml`, which could never run (#90).
- The `verified_metrics` and `agent_tasks` benches and the hard-coded scenario estimates behind them, which measured nothing (#92).

### Security
- The composite GitHub Action passes inputs to its scripts through environment variables instead of interpolating them into shell code (#90).
- Dependencies updated for RustSec advisories in `h2`, `rustls`, `rustls-webpki`, `webpki-roots`, `tokio`, `zstd`, `crossbeam-epoch`, `arrow-data` and `anyhow` (#90).

## [0.3.2] - 2026-07-29

### Fixed
- Embedding failures during indexing are tracked, so a partial index is visible instead of silent (#43).

### Removed
- The `x86_64-unknown-linux-musl` release target: ONNX Runtime publishes no musl binaries (#61, #62).

## [0.3.1] - 2026-07-29

### Added
- Configurable embedding-provider registry with index fingerprinting (#27, #35).
- End-to-end tests for the analyzer, MCP handlers and Cypher (#33).
- Contributor documentation and governance files (#46).

### Changed
- Relicensed to Apache-2.0 (#54).
- `query` and `impact` resolve to the indexed repository of the current directory by default (#23).
- `hybrid_search` uses persisted vectors instead of re-embedding (#28).

### Fixed
- Release builds for `aarch64-unknown-linux-gnu` (#2, #54).
- Embeddings, community detection, `MEMBER_OF` edges and an API-key leak found in an audit (#16).
- Single quotes escaped in LanceDB filter predicates (#18).
- Cypher fails loudly on unsupported clauses, compares strings correctly and caps rows (#20).
- `rename_symbol` is scope-aware and skips comments and strings (#25).
- Process tracing keeps branching structure and has depth limits (#26).
- BM25 term frequency is tokenized and embedding truncation bounded (#36).
- HTTP search and neighbor endpoints return backend errors as 5xx (#40).
- Setup and config file-write errors propagate instead of leaving an empty config (#42).
- The home-directory safety guard refuses ancestors and fails closed (#24).
- Benchmark figures that were not measured were removed or labeled as estimates (#19, #21, #22).

## [0.3.0] - 2026-05-29

### Added
- Initial public release: tree-sitter parsing for 23 languages, call-graph construction, community detection, hybrid search, Cypher-like queries, an MCP server and the `myc` CLI.

[Unreleased]: https://github.com/marcmantei/myceliums/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/marcmantei/myceliums/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/marcmantei/myceliums/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/marcmantei/myceliums/tree/v0.3.0
