# Contributing to Myceliums

Contributions are welcome! Whether it's a bug fix, new language support, or a feature idea, we appreciate your help.

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

The workspace has six crates:

| Crate | Purpose |
|---|---|
| `myceliums-core` | Parsing (23 languages), symbol extraction, search, graph algorithms |
| `myceliums-storage` | LanceDB persistence layer |
| `myceliums-mcp` | MCP server implementation |
| `myceliums-cypher` | Cypher query parser and executor |
| `myceliums-http` | Axum HTTP server for graph visualization |
| `myc` | CLI binary |

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

## Adding a new language

New language support lives in `crates/myceliums-core/src/parser.rs`. The steps are:

1. Add the tree-sitter grammar dependency to `Cargo.toml` (workspace and `myceliums-core`).
2. Extend the `SourceLanguage` enum with the new variant.
3. Implement the `extract_*` methods for the language (symbols, calls, imports).
4. Map file extensions to the new language.
5. Add tests covering the new parser.

Look at the existing Go or Rust implementations as templates — they cover the full pattern including structs, functions, methods, and relationships.

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

By contributing, you agree that your contributions will be licensed under the [AGPL-3.0 License](LICENSE).
