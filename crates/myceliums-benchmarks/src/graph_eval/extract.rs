//! What myceliums extracts from a fixture: run the production analyzer into a
//! scratch store and read back symbols, CALLS edges and doc MENTIONS edges.
//!
//! This is the same `Analyzer::analyze` path `myc analyze` takes, with
//! embeddings off — they play no part in symbols or call edges.

use anyhow::{Context, Result};
use myceliums_core::{Analyzer, DocMentionStats};
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Symbols, CALLS, OVERRIDES and doc MENTIONS edges of one analyzed tree.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub symbols: Vec<CodeSymbol>,
    pub calls: Vec<Relationship>,
    /// Which methods override which (#190): dispatch is derived from these
    /// and the CALLS edges. Not part of the CALLS scores.
    pub overrides: Vec<Relationship>,
    /// Wall time of the analysis, in seconds.
    pub analysis_secs: f64,
    /// MENTIONS edges from docs and doc comments (#180).
    pub mentions: Vec<Relationship>,
    /// What became of the doc mentions (#180).
    pub doc_stats: Option<DocMentionStats>,
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
    let started = std::time::Instant::now();
    analyzer
        .analyze()
        .await
        .with_context(|| format!("analyzing {}", root.display()))?;
    let analysis_secs = started.elapsed().as_secs_f64();
    let store = analyzer.store();
    let symbols = store.get_symbols().await.context("reading symbols")?;
    let (calls, rest): (Vec<Relationship>, Vec<Relationship>) = store
        .get_relationships()
        .await
        .context("reading relationships")?
        .into_iter()
        .filter(|r| {
            matches!(
                r.kind,
                RelationshipKind::Calls | RelationshipKind::Overrides | RelationshipKind::Mentions
            )
        })
        .partition(|r| r.kind == RelationshipKind::Calls);
    let (overrides, mentions) = rest
        .into_iter()
        .partition(|r| r.kind == RelationshipKind::Overrides);
    let doc_stats = DocMentionStats::load(store).await.ok().flatten();
    Ok(Extraction {
        symbols,
        calls,
        overrides,
        mentions,
        doc_stats,
        analysis_secs,
    })
}

/// Analyses of real checkouts, each tree analyzed once per run: the Java
/// CALLS sets and the doc-mention sets share one analysis of
/// spring-framework.
#[derive(Default)]
pub struct ExtractionCache(HashMap<PathBuf, Extraction>);

impl ExtractionCache {
    /// The extraction of `root`, analyzing it on first use.
    pub async fn get(&mut self, root: &Path) -> Result<&Extraction> {
        if !self.0.contains_key(root) {
            let extraction = extract(root).await?;
            self.0.insert(root.to_path_buf(), extraction);
        }
        Ok(&self.0[root])
    }
}
