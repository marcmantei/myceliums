# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries before
0.3.2 were reconstructed from git history; numbers in parentheses are issues or
pull requests.

## [Unreleased]

### Added
- Retrieval-quality measurement against a golden dataset, reported on every PR (#76, #34).
- Supply-chain checks in CI: `cargo-deny` (advisories, licenses, sources), Dependabot for Cargo and GitHub Actions, an MSRV job (`rust-version = "1.88"`), and the test suite on macOS (#90).
- Release assets now include a CycloneDX SBOM per target and a signed build-provenance attestation (#90).

### Changed
- Vector scores use one similarity scale across search paths, and the ANN index is used (#68, #71).
- MCP handlers go through a `GraphService` layer (#37, #53).
- The Homebrew tap sync can be re-run manually and is idempotent (#74).
- CI pins every third-party action to a commit SHA, runs with read-only token permissions by default, and builds with `--locked` (#90).
- `myc` is installed from crates.io; the unpublished Docker image is no longer advertised (#72, #73).

### Fixed
- `--strict-embeddings` is honored in the session bootstrap path (#69).
- The `embeddings` feature gate lost in #43 is restored (#70).
- Benchmark baselines are content-addressed instead of tied to a git commit (#86).
- Lint findings in test code, now that clippy covers `--all-targets` (#81–#85, #87).

### Removed
- The `benchmark` job in `ci.yml`, which could never run (#90).

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
[0.3.0]: https://github.com/marcmantei/myceliums/releases/tag/v0.3.0
