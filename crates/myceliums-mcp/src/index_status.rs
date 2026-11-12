//! Index-health reporting shared by the search and analyze tools.

use myceliums_storage::Store;

/// Load the partial-index warning for a store, if the index is partially
/// embedded. Reads the accounting recorded at index time — no per-query vector
/// scan. Returns `None` when the index is complete or has no accounting.
///
/// A genuine load error is logged (so it is observable rather than silently
/// indistinguishable from "no warning") but does not fail the query.
pub(crate) async fn partial_index_warning(store: &Store) -> Option<String> {
    match myceliums_core::EmbeddingStats::load(store).await {
        Ok(stats) => stats.and_then(|stats| stats.partial_index_warning()),
        Err(e) => {
            tracing::warn!("Failed to load embedding accounting for partial-index warning: {e}");
            None
        }
    }
}

/// Load the embedding accounting for a store, for reporting in the `analyze`
/// response's cached path.
///
/// A genuine load error propagates. A *missing* record means a legacy index
/// built before accounting existed: treat it as fully embedded (using
/// `symbol_count` as the total) rather than "0 of N", which would falsely
/// imply a total embedding failure. Centralized so the cached and non-cached
/// query paths derive the reported counts the same way — the non-cached path
/// gets the equivalent record straight from `AnalysisResult::embedding_stats`.
pub(crate) async fn load_embedding_stats(
    store: &Store,
    symbol_count: usize,
) -> Result<myceliums_core::EmbeddingStats, rmcp::ErrorData> {
    let loaded = myceliums_core::EmbeddingStats::load(store)
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
    Ok(loaded
        .unwrap_or_else(|| myceliums_core::EmbeddingStats::complete(symbol_count, symbol_count)))
}

#[cfg(test)]
mod tests {
    use crate::format;

    /// A partial-index warning is prefixed onto the rendered search body as a
    /// banner the user cannot miss. Regression guard for issue #32: partial
    /// indexes were invisible at query time.
    ///
    /// The store round-trip that produces the warning
    /// (`EmbeddingStats::record` → `load` → `partial_index_warning`) is owned
    /// and tested by `myceliums-core`; here we assert only the MCP-specific
    /// surfacing — how a warning is banded onto a response body.
    #[test]
    fn partial_index_warning_is_banded_onto_search_body() {
        let warning = myceliums_core::EmbeddingStats {
            symbols_total: 3,
            symbols_embedded: 1,
            embedding_failures: 2,
        }
        .partial_index_warning()
        .expect("partial index must warn");
        assert!(warning.contains("1 of 3 symbols"));

        let body = format::with_index_warning(Some(warning), "results table".to_string());
        assert!(body.starts_with("⚠ "));
        assert!(body.contains("results table"));
    }

    /// A complete index yields no warning, so the search body is passed through
    /// untouched — no banner.
    #[test]
    fn complete_index_leaves_search_body_untouched() {
        let warning = myceliums_core::EmbeddingStats::complete(2, 2).partial_index_warning();
        assert_eq!(warning, None);

        let body = format::with_index_warning(warning, "results table".to_string());
        assert_eq!(body, "results table");
    }
}
