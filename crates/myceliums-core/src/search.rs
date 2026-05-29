//! BM25-inspired text search over code symbols.
//!
//! Provides a simple, dependency-free keyword search that scores symbols by
//! name, signature, and content using TF-IDF with BM25 normalization.

use myceliums_storage::CodeSymbol;
use serde::Serialize;

/// Per-term scoring breakdown for explain mode.
#[derive(Debug, Clone, Serialize)]
pub struct TermScore {
    /// The query term.
    pub term: String,
    /// Raw term frequency in the document.
    pub tf: f64,
    /// Inverse document frequency across all symbols.
    pub idf: f64,
    /// BM25-normalized term frequency.
    pub tf_norm: f64,
    /// This term's contribution to the total score (`idf * tf_norm`).
    pub contribution: f64,
    /// Which fields the term was found in (`"name"`, `"signature"`, `"content"`).
    pub matched_in: Vec<String>,
}

/// Explain trace for a single search result.
#[derive(Debug, Clone, Serialize)]
pub struct SearchExplain {
    /// Per-term breakdown of how the score was computed.
    pub term_scores: Vec<TermScore>,
    /// Document length (combined length of name + signature + content).
    pub doc_len: f64,
    /// Average document length across all symbols.
    pub avg_doc_len: f64,
}

/// A single search result with its relevance score.
pub struct SearchResult {
    /// The matched code symbol.
    pub symbol: CodeSymbol,
    /// BM25 relevance score (higher is more relevant).
    pub score: f64,
    /// Optional scoring breakdown (populated when using [`search_symbols_explain`]).
    pub explain: Option<SearchExplain>,
}

/// Simple BM25-inspired text search over symbols.
/// Uses name, signature, and content fields.
pub fn search_symbols(symbols: &[CodeSymbol], query: &str) -> Vec<SearchResult> {
    search_symbols_impl(symbols, query, false)
}

/// BM25 search with explain traces showing per-term scoring breakdown.
pub fn search_symbols_explain(symbols: &[CodeSymbol], query: &str) -> Vec<SearchResult> {
    search_symbols_impl(symbols, query, true)
}

fn search_symbols_impl(symbols: &[CodeSymbol], query: &str, explain: bool) -> Vec<SearchResult> {
    let query_terms: Vec<&str> = query.split_whitespace().collect();
    if query_terms.is_empty() {
        return vec![];
    }

    let avg_len: f64 = symbols.iter().map(doc_len).sum::<f64>() / symbols.len().max(1) as f64;
    let k1 = 1.2;
    let b = 0.75;
    let n = symbols.len() as f64;

    // IDF per term
    let term_doc_freq: Vec<f64> = query_terms
        .iter()
        .map(|term| symbols.iter().filter(|s| contains_term(s, term)).count() as f64)
        .collect();

    let mut results: Vec<SearchResult> = symbols
        .iter()
        .filter_map(|sym| {
            let dl = doc_len(sym);
            let mut score = 0.0;
            let mut term_scores = Vec::new();

            for (i, term) in query_terms.iter().enumerate() {
                let tf = term_freq(sym, term);
                if tf == 0.0 {
                    continue;
                }
                let df = term_doc_freq[i];
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_len));
                let contribution = idf * tf_norm;
                score += contribution;

                if explain {
                    let lower = term.to_lowercase();
                    let mut matched_in = Vec::new();
                    if sym.name.to_lowercase().contains(&lower) {
                        matched_in.push("name".to_string());
                    }
                    if sym.signature.to_lowercase().contains(&lower) {
                        matched_in.push("signature".to_string());
                    }
                    if sym.content.to_lowercase().contains(&lower) {
                        matched_in.push("content".to_string());
                    }
                    if metadata_text(sym).to_lowercase().contains(&lower) {
                        matched_in.push("metadata".to_string());
                    }
                    term_scores.push(TermScore {
                        term: term.to_string(),
                        tf,
                        idf,
                        tf_norm,
                        contribution,
                        matched_in,
                    });
                }
            }

            if score > 0.0 {
                let explain_trace = if explain {
                    Some(SearchExplain {
                        term_scores,
                        doc_len: dl,
                        avg_doc_len: avg_len,
                    })
                } else {
                    None
                };
                Some(SearchResult {
                    symbol: sym.clone(),
                    score,
                    explain: explain_trace,
                })
            } else {
                None
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

fn doc_len(sym: &CodeSymbol) -> f64 {
    let base = sym.name.len() + sym.signature.len() + sym.content.len();
    let meta_len = metadata_text(sym).len();
    (base + meta_len) as f64
}

fn contains_term(sym: &CodeSymbol, term: &str) -> bool {
    let lower = term.to_lowercase();
    sym.name.to_lowercase().contains(&lower)
        || sym.signature.to_lowercase().contains(&lower)
        || sym.content.to_lowercase().contains(&lower)
        || metadata_text(sym).to_lowercase().contains(&lower)
}

fn term_freq(sym: &CodeSymbol, term: &str) -> f64 {
    let lower = term.to_lowercase();
    let meta = metadata_text(sym);
    let text = format!(
        "{} {} {} {}",
        sym.name.to_lowercase(),
        sym.signature.to_lowercase(),
        sym.content.to_lowercase(),
        meta.to_lowercase()
    );
    text.matches(&lower).count() as f64
}

/// Extract searchable text from symbol metadata (decorators, return type, etc.).
fn metadata_text(sym: &CodeSymbol) -> String {
    let Some(ref json_str) = sym.metadata else {
        return String::new();
    };
    // Parse the JSON metadata and extract searchable fields
    let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(decorators) = meta.get("decorators").and_then(|v| v.as_array()) {
        for d in decorators {
            if let Some(s) = d.as_str() {
                parts.push(s.to_string());
            }
        }
    }
    if let Some(rt) = meta.get("return_type").and_then(|v| v.as_str()) {
        parts.push(rt.to_string());
    }
    if let Some(supers) = meta.get("superclasses").and_then(|v| v.as_array()) {
        for s in supers {
            if let Some(name) = s.as_str() {
                parts.push(name.to_string());
            }
        }
    }
    if let Some(vis) = meta.get("visibility").and_then(|v| v.as_str()) {
        parts.push(vis.to_string());
    }
    parts.join(" ")
}
