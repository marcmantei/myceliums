# myceliums-core

Code intelligence core for the [Myceliums](https://github.com/marcmantei/myceliums) knowledge engine.

This crate provides tree-sitter-based parsing, call-graph construction,
community detection, impact analysis, and hybrid search over codebases.

## Getting Started

Add `myceliums-core` to your project:

```sh
cargo add myceliums-core
```

Then analyze a repository:

```rust,no_run
use myceliums_core::{Analyzer, ProjectConfig};
use myceliums_storage::Store;
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Open a store (LanceDB-backed)
    let store = Store::open(Path::new("/tmp/myceliums-data"), "my-repo-id").await?;

    // 2. Build an analyzer for your repository
    let repo = PathBuf::from("/path/to/repo");
    let analyzer = Analyzer::new(store, repo);

    // 3. Run full analysis (parsing + call-graph + embeddings)
    let result = analyzer.analyze().await?;
    println!("Indexed {} symbols across {} files", result.symbol_count, result.file_count);

    // 4. Search the indexed symbols
    let symbols = analyzer.store().get_symbols().await?;
    let hits = myceliums_core::search_symbols(&symbols, "authenticate user");
    for hit in &hits {
        println!("{} (score {:.2})", hit.symbol.name, hit.score);
    }
    Ok(())
}
```

## Feature Flags

| Feature      | Default | Description                                                      |
|--------------|---------|------------------------------------------------------------------|
| `embeddings` | **yes** | Enables fastembed-based semantic search and cross-encoder reranking |

Disable `embeddings` for a lighter build that only supports BM25 text search:

```toml
[dependencies]
myceliums-core = { version = "0.2", default-features = false }
```

## Modules

| Module         | Description                                        |
|----------------|----------------------------------------------------|
| `analyzer`     | Repository walker, parser orchestration, indexing   |
| `parser`       | Tree-sitter grammars for 19 languages              |
| `search`       | BM25 keyword search over symbols                   |
| `hybrid_search`| Reciprocal-rank fusion of BM25 + vector search     |
| `embeddings`   | fastembed model loading, encoding, reranking        |
| `community`    | Graph-based community detection (Louvain)           |
| `impact`       | Git-diff-driven impact analysis                     |
| `process`      | Trace execution paths through the call graph        |
| `rename`       | Plan symbol renames across the codebase             |
| `cache`        | Incremental analysis cache via git HEAD             |
| `config`       | Per-project `.mycelium.toml` configuration          |
| `error`        | Structured error types (`MyceliumError`)            |

## Supported Languages

Rust, TypeScript, JavaScript, Python, Go, Java, C#, C, C++, Ruby, Kotlin,
Swift, PHP, Lua, Zig, PowerShell, Elixir, Scala, Objective-C.

## License

MIT
