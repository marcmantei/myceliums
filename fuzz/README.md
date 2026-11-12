# Fuzz targets

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for the code
that takes untrusted input: agent-supplied Cypher queries and arbitrary
repository files.

| Target | What it does |
|--------|--------------|
| `cypher_parse` | Lexes and parses arbitrary UTF-8 as Cypher; checks lexer spans. |
| `cypher_execute` | Parses and executes arbitrary UTF-8 against a small fixed graph (`crates/myceliums-cypher/tests/support/small_graph.rs`); checks `LIMIT`. |
| `extract_symbols` | First byte picks one of the 23 tree-sitter languages, the rest is the file; runs `SourceParser::parse` and `parse_with_dsl`. |

A target fails on any panic, stack overflow, or timeout.

This directory is its own Cargo workspace and is excluded from the main one, so
`cargo build --workspace` on stable never sees it. Faster, always-on coverage
of the same properties lives in the normal test suite as `proptest` tests
(`crates/myceliums-cypher/tests/proptest_cypher.rs`,
`crates/myceliums-core/tests/proptest_extractors.rs`).

## Running locally

```bash
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked

# from the repository root
cargo +nightly fuzz run -s none -a cypher_parse fuzz/corpus/cypher_parse fuzz/seeds/cypher_parse \
  -- -max_total_time=60 -dict=fuzz/cypher.dict
cargo +nightly fuzz run -s none -a cypher_execute fuzz/corpus/cypher_execute fuzz/seeds/cypher_execute \
  -- -max_total_time=60 -dict=fuzz/cypher.dict
cargo +nightly fuzz run -s none -a extract_symbols fuzz/corpus/extract_symbols fuzz/seeds/extract_symbols \
  -- -max_total_time=60 -max_len=4096
```

`-s none` skips AddressSanitizer: the native code worth sanitizing (the
tree-sitter grammars) is compiled by `cc` without sanitizer flags, so ASan would
only instrument safe Rust. `-a` keeps debug assertions and overflow checks on.

On macOS, add `RUSTFLAGS="-Clink-arg=-undefined -Clink-arg=dynamic_lookup"`:
`lance-arrow` is also built as a `cdylib`, and the macOS linker otherwise
rejects the coverage-instrumentation symbols that only the fuzz binary defines.
Linux needs nothing extra.

A crash is written to `fuzz/artifacts/<target>/`. Reproduce it with
`cargo +nightly fuzz run -s none -a <target> fuzz/artifacts/<target>/<file>`, fix
it, and add a regression test next to the code it exercises.

`fuzz/seeds/` holds a few hand-written starting inputs; `fuzz/corpus/` (the
growing corpus) and `fuzz/artifacts/` are git-ignored.

## CI

`.github/workflows/fuzz.yml` builds the targets once and runs each for 60
seconds on nightly, in a single job — weekly, on demand, and on pull requests
that touch the Cypher crate, the extractors, the Cargo manifests/lock, or this
directory. It is not a required check; crash reproducers are uploaded as a
workflow artifact.
