//! Embedding accounting — first-class tracking of how completely an index was
//! embedded.
//!
//! Embedding generation can partially fail (model load errors, provider
//! timeouts, storage errors). Historically those failures degraded to a
//! `warn!` and the analysis still reported success, leaving a half-empty index
//! that answered queries with no hint that vectors were missing.
//!
//! [`EmbeddingStats`] gives embeddings the same first-class accounting the
//! parsing pipeline already has for skipped files: how many symbols were
//! candidates for embedding, how many were actually embedded, and how many
//! failed. The record is persisted in the index (`index_meta` table) so query
//! paths can surface a partial-index warning without scanning vectors.

use serde::{Deserialize, Serialize};

use myceliums_storage::Store;

/// How completely an index was embedded.
///
/// `symbols_embedded + embedding_failures` need not equal `symbols_total`:
/// when embeddings are skipped entirely both counts are zero while
/// `symbols_total` reflects the symbols that *could* have been embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingStats {
    /// Symbols that were candidates for embedding.
    pub symbols_total: usize,
    /// Symbols for which a vector was successfully generated and stored.
    pub symbols_embedded: usize,
    /// Symbols whose embedding generation or storage failed.
    pub embedding_failures: usize,
}

impl EmbeddingStats {
    /// Key under which the record is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "embedding_stats";

    /// A fully-embedded run: every candidate symbol got a vector.
    pub fn complete(symbols_total: usize, symbols_embedded: usize) -> Self {
        Self {
            symbols_total,
            symbols_embedded,
            embedding_failures: 0,
        }
    }

    /// True when at least one candidate symbol was not embedded — the index is
    /// partial and query results may omit un-embedded symbols.
    pub fn is_partial(&self) -> bool {
        self.symbols_embedded < self.symbols_total
    }

    /// A human-readable warning describing the partial state, or `None` when
    /// the index is complete. Suitable for attaching to search responses.
    pub fn partial_index_warning(&self) -> Option<String> {
        if self.is_partial() {
            Some(format!(
                "index partially embedded: {} of {} symbols have vectors\
                 ({} embedding failures); un-embedded symbols are invisible to \
                 semantic and hybrid search",
                self.symbols_embedded, self.symbols_total, self.embedding_failures
            ))
        } else {
            None
        }
    }

    /// Persist the record inside the index so query paths can read it back.
    pub async fn record(&self, store: &Store) -> anyhow::Result<()> {
        let json = serde_json::to_string(self)?;
        store.set_index_meta(Self::META_KEY, &json).await?;
        Ok(())
    }

    /// Read the record from an index, or `None` when it was never written
    /// (e.g. an index built before embedding accounting existed).
    pub async fn load(store: &Store) -> anyhow::Result<Option<Self>> {
        match store.get_index_meta(Self::META_KEY).await? {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_index_is_not_partial() {
        let stats = EmbeddingStats::complete(10, 10);
        assert!(!stats.is_partial());
        assert_eq!(stats.partial_index_warning(), None);
    }

    #[test]
    fn partial_index_reports_warning() {
        let stats = EmbeddingStats {
            symbols_total: 10,
            symbols_embedded: 7,
            embedding_failures: 3,
        };
        assert!(stats.is_partial());
        let warning = stats.partial_index_warning().expect("partial => warning");
        assert!(warning.contains("7 of 10"));
        assert!(warning.contains("3 embedding failures"));
    }

    #[test]
    fn empty_index_is_not_partial() {
        let stats = EmbeddingStats::complete(0, 0);
        assert!(!stats.is_partial());
    }

    #[tokio::test]
    async fn stats_round_trip_through_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "test-repo").await.unwrap();

        // A fresh index has no accounting yet.
        assert_eq!(EmbeddingStats::load(&store).await.unwrap(), None);

        let stats = EmbeddingStats {
            symbols_total: 12,
            symbols_embedded: 9,
            embedding_failures: 3,
        };
        stats.record(&store).await.unwrap();

        let loaded = EmbeddingStats::load(&store)
            .await
            .unwrap()
            .expect("stats were recorded");
        assert_eq!(loaded, stats);
        assert!(loaded.is_partial());
        assert!(loaded
            .partial_index_warning()
            .unwrap()
            .contains("9 of 12 symbols"));
    }

    #[tokio::test]
    async fn complete_index_records_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "test-repo").await.unwrap();

        EmbeddingStats::complete(5, 5).record(&store).await.unwrap();

        let loaded = EmbeddingStats::load(&store).await.unwrap().unwrap();
        assert!(!loaded.is_partial());
        assert_eq!(loaded.partial_index_warning(), None);
    }
}
