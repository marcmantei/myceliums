//! `myc search`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::{Context, Result};
use myceliums_core::{
    attach_graph_edges, embedder_for_index, hybrid_search_explain, hybrid_search_with,
    rerank_results, search_symbols_ranked, semantic_search, HybridParams, TestPolicy,
};
use myceliums_storage::{RepoInfo, Store};

/// A warning when the index predates the test/product classification (#146)
/// and `tests` would reorder test code: it ranks like product code until the
/// repository is re-analyzed.
fn unclassified_warning(repo_info: &RepoInfo, tests: &TestPolicy) -> Option<&'static str> {
    let stale =
        repo_info.classification_version < myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION;
    (stale && !tests.is_neutral()).then_some(
        "This index was built before test code was classified, so tests rank like product \
         code. Run `myc analyze` to rank them below it.",
    )
}

/// The partial-index warning for a store, if the index is only partially
/// embedded. Reads accounting recorded at index time — no per-query scan.
///
/// A genuine load error is logged (so it is not silently indistinguishable
/// from "no warning") but does not fail the surrounding query: an unreadable
/// accounting record should not block search results, it should only cost the
/// advisory warning.
pub(crate) async fn partial_index_warning(store: &Store) -> Option<String> {
    match myceliums_core::EmbeddingStats::load(store).await {
        Ok(stats) => stats.and_then(|stats| stats.partial_index_warning()),
        Err(e) => {
            tracing::warn!("Failed to load embedding accounting for partial-index warning: {e}");
            None
        }
    }
}

pub(crate) async fn cmd_search(
    query: &str,
    repo: Option<&str>,
    limit: usize,
    use_hybrid: bool,
    use_rerank: bool,
    use_explain: bool,
    tests: TestPolicy,
) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    if let Some(w) = unclassified_warning(&repo_info, &tests) {
        eprintln!("⚠ {w}");
    }
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;
    let store = graph.store();

    let symbols = graph.symbols().await?;

    if use_hybrid {
        let embedder = embedder_for_index(&store).await?;
        let params = HybridParams::with_tests(tests);
        let mut results = if use_explain {
            hybrid_search_explain(&embedder, &symbols, &*store, query, limit, &params).await?
        } else {
            hybrid_search_with(&embedder, &symbols, &*store, query, limit, &params).await?
        };
        // The vector search read the store directly: check that it saw the
        // index generation the symbol table came from.
        graph.verify()?;

        // Apply reranking if requested, using the reranker recorded at indexing time
        if use_rerank {
            let reranker_id = embedder.meta().reranker.clone();
            results = rerank_results(query, results, reranker_id.as_deref(), &tests).await?;
        }

        // Attach graph edges for explain mode
        if use_explain {
            let relationships = graph.relationships().await?;
            let uid_to_name: std::collections::HashMap<&str, &str> = symbols
                .iter()
                .map(|s| (s.uid.as_str(), s.name.as_str()))
                .collect();
            attach_graph_edges(&mut results, &relationships, &uid_to_name);
        }

        if results.is_empty() {
            println!("No results found for '{}'", query);
            return Ok(());
        }

        let search_type = if use_rerank {
            "Hybrid search (reranked) results"
        } else {
            "Hybrid search results"
        };
        // Advisory warning goes to stderr so it never corrupts the result
        // data on stdout (which may be piped or consumed by hooks).
        if let Some(w) = partial_index_warning(&store).await {
            eprintln!("⚠ {w}");
            println!();
        }
        println!("{} for '{}' in {}:", search_type, query, repo_info.name);
        println!();

        for (i, result) in results.iter().enumerate() {
            let sym = &result.symbol;
            let sources = match (result.bm25_rank, result.vector_rank) {
                (Some(br), Some(vr)) => format!("BM25#{} + Vec#{}", br, vr),
                (Some(br), None) => format!("BM25#{}", br),
                (None, Some(vr)) => format!("Vec#{}", vr),
                (None, None) => "?".to_string(),
            };
            println!(
                "  {}. {} ({}) — {}:{}-{}  [rrf: {:.6}] [{}]",
                i + 1,
                sym.qualified_name,
                sym.kind,
                sym.file_path,
                sym.start_line,
                sym.end_line,
                result.combined_score,
                sources,
            );
            if !sym.signature.is_empty() {
                let sig = sym.signature.lines().next().unwrap_or("");
                if sig.len() > 80 {
                    println!("     {:.80}...", sig);
                } else {
                    println!("     {}", sig);
                }
            }
            if use_explain {
                print_explain_trace(&result.explain);
            }
        }
    } else {
        let results = search_symbols_ranked(&symbols, query, use_explain, &tests);

        if results.is_empty() {
            println!("No results found for '{}'", query);
            return Ok(());
        }

        println!("Search results for '{}' in {}:", query, repo_info.name);
        println!();

        for (i, result) in results.iter().take(limit).enumerate() {
            let sym = &result.symbol;
            println!(
                "  {}. {} ({}) — {}:{}-{}  [score: {:.2}]",
                i + 1,
                sym.qualified_name,
                sym.kind,
                sym.file_path,
                sym.start_line,
                sym.end_line,
                result.score
            );
            if !sym.signature.is_empty() {
                let sig = sym.signature.lines().next().unwrap_or("");
                if sig.len() > 80 {
                    println!("     {:.80}...", sig);
                } else {
                    println!("     {}", sig);
                }
            }
            if use_explain {
                if let Some(ref explain) = result.explain {
                    println!("     --- explain ---");
                    println!(
                        "     doc_len: {:.0}  avg_doc_len: {:.0}",
                        explain.doc_len, explain.avg_doc_len
                    );
                    for ts in &explain.term_scores {
                        println!(
                            "     term '{}': tf={:.0} idf={:.4} tf_norm={:.4} -> {:.4} (in: {})",
                            ts.term,
                            ts.tf,
                            ts.idf,
                            ts.tf_norm,
                            ts.contribution,
                            ts.matched_in.join(", ")
                        );
                    }
                }
            }
        }

        let total = results.len();
        if total > limit {
            println!();
            println!("  ... and {} more results", total - limit);
        }
    }

    Ok(())
}

fn print_explain_trace(explain: &Option<myceliums_core::HybridExplain>) {
    if let Some(ref explain) = explain {
        println!("     --- explain ---");
        if let Some(ref bm25) = explain.bm25 {
            println!(
                "     BM25: doc_len={:.0}  avg_doc_len={:.0}",
                bm25.doc_len, bm25.avg_doc_len
            );
            for ts in &bm25.term_scores {
                println!(
                    "       term '{}': tf={:.0} idf={:.4} tf_norm={:.4} -> {:.4} (in: {})",
                    ts.term,
                    ts.tf,
                    ts.idf,
                    ts.tf_norm,
                    ts.contribution,
                    ts.matched_in.join(", ")
                );
            }
        }
        if !explain.graph_edges.is_empty() {
            println!("     Graph paths:");
            for edge in &explain.graph_edges {
                println!(
                    "       {} --[{}]--> {}",
                    edge.source, edge.kind, edge.target
                );
            }
        }
    }
}

pub(crate) async fn cmd_semantic_search(
    query: &str,
    repo: Option<&str>,
    limit: usize,
    tests: TestPolicy,
) -> Result<()> {
    let (_, ri) = resolve_repo_or_latest(repo)?;
    if let Some(w) = unclassified_warning(&ri, &tests) {
        eprintln!("⚠ {w}");
    }
    // Only vector search: the store handle is enough, no graph snapshot.
    let store = graph_service().open_store(&ri.id).await?;

    let embedder = embedder_for_index(&store)
        .await
        .context("Failed to load embedding model")?;
    let query_vector = embedder
        .embed_query(query)
        .await
        .context("Failed to embed query")?;

    let results = semantic_search(&*store, &query_vector, limit, &tests).await?;

    if results.is_empty() {
        println!(
            "No semantic results found for '{}'. Have you run 'myc analyze' first?",
            query
        );
        return Ok(());
    }

    println!("Semantic search results for '{}' in {}:\n", query, ri.name);
    // Advisory warning goes to stderr so it never corrupts the result data on
    // stdout (which may be piped or consumed by hooks).
    if let Some(w) = partial_index_warning(&store).await {
        eprintln!("⚠ {w}\n");
    }
    for (i, (sym, score)) in results.iter().enumerate() {
        println!(
            "  {}. {} ({}) -- {}:{}-{}  [similarity: {:.4}]",
            i + 1,
            sym.qualified_name,
            sym.kind,
            sym.file_path,
            sym.start_line,
            sym.end_line,
            score
        );
        if !sym.signature.is_empty() {
            let sig = sym.signature.lines().next().unwrap_or("");
            if sig.len() > 80 {
                println!("     {:.80}...", sig);
            } else {
                println!("     {}", sig);
            }
        }
    }

    Ok(())
}
