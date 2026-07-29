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

- Rust toolchain (stable) — includes `cargo`, `clippy`, `rustfmt`
- Git

### Clone and build

```bash
git clone https://github.com/marcmantei/myceliums
cd myceliums
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
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
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```
6. **Follow the [pull request template](.github/pull_request_template.md)** — it
   restates the CI gates above and asks for [Conventional
   Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`,
   `chore:`, ...). See also the [roadmap](ROADMAP.md) for planned work.

## Adding a new language

New language support lives in `crates/myceliums-core/src/parser.rs`. The steps are:

1. Add the tree-sitter grammar dependency to `Cargo.toml` (workspace and `myceliums-core`).
2. Extend the `SourceLanguage` enum with the new variant.
3. Implement the `extract_*` methods for the language (symbols, calls, imports).
4. Map file extensions to the new language.
5. Add tests covering the new parser.

Look at the existing Go or Rust implementations as templates — they cover the full pattern including structs, functions, methods, and relationships.

## Adding a new embedding or reranker model

Local models live in a curated registry in `crates/myceliums-core/src/embeddings.rs` (`EMBEDDING_MODELS` / `RERANKER_MODELS`). A new local model is a single registry entry — no other code changes — but it must meet these criteria:

1. It is supported by the pinned `fastembed` version (dimension and download repo are derived from fastembed's model list, so unsupported models cannot be added).
2. The entry declares the correct query/passage prefixes (E5-style models need `query: `/`passage: ` — check the model card).
3. The PR includes brief benchmark evidence (e.g. MTEB scores or a before/after retrieval comparison) explaining what the model adds over the existing entries.

We deliberately keep this list small. If your model isn't in fastembed or is served remotely, you don't need a code change at all: the `openai-compatible` provider in `.myceliums.toml` works with any server speaking the OpenAI embeddings API (Ollama, LM Studio, TEI, vLLM, cloud providers).

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
