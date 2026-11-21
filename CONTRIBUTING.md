# Contributing to Myceliums

Contributions are welcome! Whether it's a bug fix, new language support, or a feature idea, we appreciate your help.

**Please set your expectations first.** Myceliums is a personal project maintained
on a best-effort basis — there is no company behind it and no support commitment.
Issues may not get a timely response, and pull requests may take weeks to review.
That is not a reflection on your contribution; it is the honest capacity of a
single maintainer.

To avoid wasted effort: for anything beyond a small fix, **open an issue before
you write code** so we can agree on the approach. A well-scoped PR that nobody
asked for is still a PR that may get declined.

## Getting started

### Prerequisites

- Rust toolchain (stable) — includes `cargo`, `clippy`, `rustfmt`. The minimum
  supported version is the `rust-version` in `Cargo.toml` (currently 1.88); CI
  checks it in the `MSRV` job.
- `protoc` (Protocol Buffers compiler), needed by the LanceDB build
- Git

### Clone and build

```bash
git clone https://github.com/marcmantei/myceliums
cd myceliums
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Project structure

The workspace has seven crates:

| Crate | Purpose |
|---|---|
| `myceliums-core` | Parsing (23 languages), symbol extraction, search, graph algorithms |
| `myceliums-storage` | LanceDB persistence layer |
| `myceliums-mcp` | MCP server implementation |
| `myceliums-cypher` | Cypher query parser and executor |
| `myceliums-http` | Axum HTTP server for graph visualization |
| `myceliums-benchmarks` | Benchmark harness and retrieval-quality measurement |
| `myc` | CLI binary |

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you are expected to uphold it. Please report unacceptable
behavior as described there.

## How to contribute

1. **Open an issue first** for significant changes — this avoids duplicate work and lets us discuss the approach.
2. **Fork the repo** and create a feature branch from `main`.
3. **Keep PRs focused** — one feature or fix per PR.
4. **Write tests** for new functionality.
5. **Ensure CI checks pass** before submitting:
   ```bash
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   ```
   CI also runs `cargo deny check` (advisories, licenses, sources — policy in
   [`deny.toml`](deny.toml)), the test suite on macOS, and an MSRV build. A new
   dependency must pass `cargo deny check` locally (`cargo install cargo-deny
   --locked`).
6. **Follow the [pull request template](.github/pull_request_template.md)** — it
   restates the CI gates above and asks for [Conventional
   Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`,
   `chore:`, ...). See also the [roadmap](ROADMAP.md) for planned work.

## Adding a new language

New language support lives in `crates/myceliums-core/src/parser/`: `mod.rs` holds the public types, the language→grammar table and the dispatch; each language family has its own file (`go.rs`, `rust.rs`, `jvm.rs`, ...) with its tests alongside. The steps are:

1. Add the tree-sitter grammar dependency to `Cargo.toml` (workspace and `myceliums-core` only — `myc` gets grammars through core).
2. Extend the `SourceLanguage` enum with the new variant and map its file extensions (`from_extension`, `name`).
3. Add the grammar to `SourceLanguage::tree_sitter_language` — the single language→grammar table, also used by `myc doctor`.
4. Implement the `extract_*` methods (symbols, calls, imports) in a new `parser/<family>.rs` (or the existing family file) and call them from `SourceParser::parse`.
5. Add tests covering the new parser in the same file.

Look at the existing Go or Rust implementations as templates — they cover the full pattern including structs, functions, methods, and relationships.

## Adding a new embedding or reranker model

Local models live in a curated registry in `crates/myceliums-core/src/embeddings.rs` (`EMBEDDING_MODELS` / `RERANKER_MODELS`). A new local embedding model is a single registry entry — no other code changes — but it must meet these criteria:

1. Its weights are an ONNX graph (fp32) on HuggingFace under a permissive license (MIT, Apache-2.0, BSD). The entry pins a commit (`revision`) and lists every file the model loads with its size and SHA-256: the weights, and `tokenizer.json`, `tokenizer_config.json`, `config.json` and `special_tokens_map.json` (`tokenizer`); a file the revision does not have is pinned as `None` and never fetched. Take the LFS `sha256` from `https://huggingface.co/api/models/<repo>?blobs=true`; for files stored in git, hash the file downloaded from `https://huggingface.co/<repo>/resolve/<revision>/<file>`. An id is bound to its weights: new weights get a new id (`registry_ids_are_bound_to_their_weights` enforces it), because an index records the id, not the files.
2. Pooling and prefixes match the model's sentence-transformers configuration (`1_Pooling/config.json`, `config_sentence_transformers.json` or the model card) — a wrong pooling or a missing query prefix fails silently. `cargo test -p myceliums-core cached_models -- --ignored --nocapture` loads every cached registry model and checks that it ranks a matching snippet first.
3. The PR includes evaluation evidence from the real-repository set: dispatch the `Retrieval Quality (real repositories)` workflow with a `variants` list that includes the incumbent as variant `A` — the shipped default, `{"id":"A","model":"jina-embeddings-v2-base-code","template":"2","chunking":"on"}` (see the comment at the top of `.github/workflows/retrieval-quality-real-repos.yml`), and report the comparison job's tables — quality per split and repository, and the costs (index time, vector size, query latency, memory, download). The protocol is in `crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md` ("Embedding model, document template and chunking").

Rerankers still load through fastembed: a reranker entry must be in the pinned `fastembed` version's model list.

We deliberately keep this list small. If your model is served remotely, you don't need a code change at all: the `openai-compatible` provider in `.myceliums.toml` works with any server speaking the OpenAI embeddings API (Ollama, LM Studio, TEI, vLLM, cloud providers).

## Code style

- Run `cargo fmt` before committing.
- Follow existing patterns and conventions in the codebase.
- Avoid adding unnecessary dependencies.

## Reporting bugs

Please use [GitHub Issues](https://github.com/marcmantei/myceliums/issues) and include:

- `myc` version (`myc --version`)
- Operating system
- Steps to reproduce
- Expected vs. actual behavior

## License

By contributing, you agree that your contributions will be licensed under the [Apache-2.0 License](LICENSE).
