//! BM25-inspired text search over code symbols.
//!
//! Provides a simple, dependency-free keyword search that scores symbols by
//! name, signature, content, and metadata using TF-IDF with BM25
//! normalization. Text is tokenized on word boundaries **and** identifier
//! boundaries (`snake_case`/`camelCase`) via [`crate::tokenize`], so term
//! frequency and document length are token-exact: `"cat"` no longer matches
//! `concatenate`, and `"user name"` matches `get_user_name`.

use crate::tokenize::tokenize;
use myceliums_storage::CodeSymbol;
use serde::Serialize;
use std::collections::HashMap;

/// Per-term scoring breakdown for explain mode.
#[derive(Debug, Clone, Serialize)]
pub struct TermScore {
    /// The query term.
    pub term: String,
    /// Raw term frequency (token-exact count) in the document.
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
    /// Document length in tokens (name + signature + content + metadata).
    pub doc_len: f64,
    /// Average document length in tokens across all symbols.
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
    let query_terms: Vec<String> = tokenize(query);
    if query_terms.is_empty() {
        return vec![];
    }

    // Tokenize each document once into a bag of token counts. Token-exact
    // matching means "cat" no longer matches "concatenate", and identifier
    // splitting means "user name" matches `get_user_name`.
    let docs: Vec<DocTokens> = symbols.iter().map(DocTokens::from_symbol).collect();

    let avg_len: f64 =
        docs.iter().map(|d| d.length).sum::<f64>() / docs.len().max(1) as f64;
    let k1 = 1.2;
    let b = 0.75;
    let n = symbols.len() as f64;

    // Document frequency per query term (token-exact).
    let term_doc_freq: Vec<f64> = query_terms
        .iter()
        .map(|term| docs.iter().filter(|d| d.counts.contains_key(term)).count() as f64)
        .collect();

    let mut results: Vec<SearchResult> = symbols
        .iter()
        .zip(docs.iter())
        .filter_map(|(sym, doc)| {
            let dl = doc.length;
            let mut score = 0.0;
            let mut term_scores = Vec::new();

            for (i, term) in query_terms.iter().enumerate() {
                let tf = doc.counts.get(term).copied().unwrap_or(0) as f64;
                if tf == 0.0 {
                    continue;
                }
                let df = term_doc_freq[i];
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_len));
                let contribution = idf * tf_norm;
                score += contribution;

                if explain {
                    term_scores.push(TermScore {
                        term: term.clone(),
                        tf,
                        idf,
                        tf_norm,
                        contribution,
                        matched_in: doc.fields_for(term),
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

/// A code symbol's tokenized text: a bag of token counts, the token-based
/// document length, and per-field token sets for explain traces.
///
/// Building this once per symbol avoids re-tokenizing the same text for every
/// query term.
struct DocTokens {
    /// Token → occurrence count across all searchable fields.
    counts: HashMap<String, u32>,
    /// Document length in tokens (BM25 length normalization input).
    length: f64,
    /// Distinct tokens per field, for explain-mode `matched_in`.
    name_tokens: HashMap<String, ()>,
    signature_tokens: HashMap<String, ()>,
    content_tokens: HashMap<String, ()>,
    metadata_tokens: HashMap<String, ()>,
}

impl DocTokens {
    fn from_symbol(sym: &CodeSymbol) -> Self {
        let name = tokenize(&sym.name);
        let signature = tokenize(&sym.signature);
        let content = tokenize(&sym.content);
        let metadata = tokenize(&metadata_text(sym));

        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut length = 0u32;
        for field in [&name, &signature, &content, &metadata] {
            for tok in field {
                *counts.entry(tok.clone()).or_insert(0) += 1;
                length += 1;
            }
        }

        DocTokens {
            counts,
            length: length as f64,
            name_tokens: name.into_iter().map(|t| (t, ())).collect(),
            signature_tokens: signature.into_iter().map(|t| (t, ())).collect(),
            content_tokens: content.into_iter().map(|t| (t, ())).collect(),
            metadata_tokens: metadata.into_iter().map(|t| (t, ())).collect(),
        }
    }

    /// Which fields contain `term` (already tokenized/lowercased).
    fn fields_for(&self, term: &str) -> Vec<String> {
        let mut matched = Vec::new();
        if self.name_tokens.contains_key(term) {
            matched.push("name".to_string());
        }
        if self.signature_tokens.contains_key(term) {
            matched.push("signature".to_string());
        }
        if self.content_tokens.contains_key(term) {
            matched.push("content".to_string());
        }
        if self.metadata_tokens.contains_key(term) {
            matched.push("metadata".to_string());
        }
        matched
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn symbol(name: &str, signature: &str, content: &str) -> CodeSymbol {
        CodeSymbol {
            uid: format!("uid-{name}"),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 2,
            signature: signature.to_string(),
            content: content.to_string(),
            repo_id: "repo-1".to_string(),
            metadata: None,
        }
    }

    fn names(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.symbol.name.as_str()).collect()
    }

    #[test]
    fn cat_does_not_match_concatenate() {
        // Substring matching would score `concatenate`; token-exact must not.
        let symbols = vec![
            symbol("concatenate", "fn concatenate(a, b)", "join two strings"),
            symbol("feed_cat", "fn feed_cat()", "give the cat food"),
        ];
        let results = search_symbols(&symbols, "cat");
        assert_eq!(
            names(&results),
            vec!["feed_cat"],
            "`cat` must match only the symbol with a whole `cat` token"
        );
    }

    #[test]
    fn user_name_matches_get_user_name() {
        // Identifier splitting lets a natural-language query hit an identifier.
        let symbols = vec![
            symbol("get_user_name", "fn get_user_name() -> String", "self.name.clone()"),
            symbol("compute_hash", "fn compute_hash() -> u64", "hash the bytes"),
        ];
        let results = search_symbols(&symbols, "user name");
        assert!(
            !results.is_empty(),
            "`user name` should match `get_user_name`"
        );
        assert_eq!(results[0].symbol.name, "get_user_name");
    }

    #[test]
    fn camel_case_query_matches_snake_case_symbol() {
        let symbols = vec![
            symbol("get_user_name", "fn get_user_name()", ""),
            symbol("noise", "fn noise()", "unrelated"),
        ];
        // Query tokens split the same way regardless of the caller's casing.
        let results = search_symbols(&symbols, "getUserName");
        assert_eq!(results[0].symbol.name, "get_user_name");
    }

    #[test]
    fn empty_query_returns_nothing() {
        let symbols = vec![symbol("foo", "fn foo()", "bar")];
        assert!(search_symbols(&symbols, "").is_empty());
        assert!(search_symbols(&symbols, "   ").is_empty());
    }

    #[test]
    fn explain_reports_token_tf_and_fields() {
        let symbols = vec![symbol(
            "parse_user",
            "fn parse_user(user: User)",
            "parse the user record",
        )];
        let results = search_symbols_explain(&symbols, "user");
        let explain = results[0].explain.as_ref().expect("explain trace");
        let term = &explain.term_scores[0];
        assert_eq!(term.term, "user");
        // Token-exact TF counts every occurrence across all fields:
        //   name       `parse_user`               -> user x1
        //   signature  `fn parse_user(user: User)` -> user x3 (parse_user, user, User)
        //   content    `parse the user record`     -> user x1
        // Total tf == 5. Substring matching would have conflated these; the
        // point of this test is that identifier splitting counts each token.
        assert_eq!(term.tf, 5.0);
        assert!(term.matched_in.contains(&"name".to_string()));
        assert!(term.matched_in.contains(&"signature".to_string()));
        assert!(term.matched_in.contains(&"content".to_string()));
    }

    #[test]
    fn doc_length_is_token_based() {
        // A symbol with many tokens but few characters must not be penalised
        // by character-based length. Two single-token docs have equal length.
        let symbols = vec![symbol("a", "", ""), symbol("b", "", "")];
        let results = search_symbols_explain(&symbols, "a");
        let explain = results[0].explain.as_ref().unwrap();
        assert_eq!(explain.doc_len, 1.0);
        assert_eq!(explain.avg_doc_len, 1.0);
    }
}
