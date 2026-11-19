//! What myceliums extracts from a fixture: run the production analyzer into a
//! scratch store and read back symbols and CALLS edges.
//!
//! This is the same `Analyzer::analyze` path `myc analyze` takes, with
//! embeddings off — they play no part in symbols or call edges.

use anyhow::{Context, Result};
use myceliums_core::Analyzer;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, Store};
use std::path::Path;

/// Symbols and CALLS edges of one analyzed tree.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub symbols: Vec<CodeSymbol>,
    pub calls: Vec<Relationship>,
}

/// Analyze `root` into a temporary store and return what was extracted.
///
/// Symbol paths are relative to `root`. Nothing is written outside the
/// temporary directory, which is removed on return.
pub async fn extract(root: &Path) -> Result<Extraction> {
    let scratch = tempfile::tempdir().context("creating a scratch store directory")?;
    let store = Store::open(&scratch.path().join("db"), "graph-eval")
        .await
        .context("opening the scratch store")?;
    let analyzer = Analyzer::new(store, root.to_path_buf()).set_skip_embeddings(true);
    analyzer
        .analyze()
        .await
        .with_context(|| format!("analyzing {}", root.display()))?;
    let store = analyzer.store();
    let symbols = store.get_symbols().await.context("reading symbols")?;
    let calls = store
        .get_relationships()
        .await
        .context("reading relationships")?
        .into_iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    Ok(Extraction { symbols, calls })
}
