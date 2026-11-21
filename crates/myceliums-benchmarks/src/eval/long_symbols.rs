//! The long-symbol slice of the real-repository set (#157).
//!
//! Chunking can only matter for symbols longer than the model's window, and on
//! the whole set such symbols are a minority — a gain on them would drown in
//! the queries chunking cannot touch. The slice is the queries with at least
//! one label whose body exceeds [`LONG_SYMBOL_BYTES`]: the content budget at
//! which template 1 cuts a 512-token model, so exactly the labels whose tail
//! one vector per symbol never saw. The rule reads only the corpus and the
//! labels, so every model and template is scored on the same queries.

use super::corpus::{symbol_ref, Corpus};
use super::evaluator::{MetricValues, ModeScore, QueryScore, SearchMode};
use super::real_repos::{RealRepoSet, Split};
use super::report::round;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Body length above which a labelled symbol makes its query part of the
/// slice: `(512 − 64) × 4` bytes, template 1's content budget for a
/// 512-token model.
pub const LONG_SYMBOL_BYTES: usize = 1_792;

/// Which queries form the slice and how every mode scored on them.
#[derive(Debug, Clone, Serialize)]
pub struct LongSymbolReport {
    /// The membership rule, in words.
    pub rule: String,
    /// [`LONG_SYMBOL_BYTES`].
    pub threshold_bytes: usize,
    /// Slice queries and their split.
    pub queries: BTreeMap<String, Split>,
    /// Per slice query, its long labels with their body length in bytes.
    pub long_labels: BTreeMap<String, Vec<(String, usize)>>,
    /// Long labels the index embedded with body chunks — `null` for an index
    /// without chunking. Labels that fit the window after template 2's
    /// normalisation, and containers, have none.
    pub long_labels_chunked: Option<usize>,
    /// Mode means by scope (`all`, `dev`, `held-out`), then mode.
    pub means: BTreeMap<String, BTreeMap<String, MetricValues>>,
}

/// The slice of `set` over `corpora`, with the means of `scores`.
/// `chunked` holds the symbols (as `file::qualified_name`) that have body
/// chunks in the index, `None` when it does not chunk.
pub fn report(
    set: &RealRepoSet,
    corpora: &[Corpus],
    scores: &BTreeMap<SearchMode, Vec<QueryScore>>,
    modes: &[SearchMode],
    assignment: &BTreeMap<String, Split>,
    chunked: Option<&HashSet<String>>,
) -> Result<LongSymbolReport> {
    let mut length: HashMap<_, usize> = HashMap::new();
    for corpus in corpora {
        for symbol in corpus.symbols() {
            let entry = length.entry(symbol_ref(symbol)).or_default();
            *entry = (*entry).max(symbol.content.len());
        }
    }
    let mut queries = BTreeMap::new();
    let mut long_labels = BTreeMap::new();
    for query in &set.queries {
        let long: Vec<(String, usize)> = query
            .relevant
            .iter()
            .filter_map(|label| {
                let bytes = *length.get(&label.symbol_ref())?;
                (bytes > LONG_SYMBOL_BYTES).then(|| (label.symbol_ref().to_string(), bytes))
            })
            .collect();
        if !long.is_empty() {
            let split = assignment.get(&query.id).copied().unwrap_or(Split::Dev);
            queries.insert(query.id.clone(), split);
            long_labels.insert(query.id.clone(), long);
        }
    }
    let mut means = BTreeMap::new();
    for (scope, keep) in [
        ("all", None),
        ("dev", Some(Split::Dev)),
        ("held-out", Some(Split::HeldOut)),
    ] {
        let ids: Vec<&str> = queries
            .iter()
            .filter(|(_, split)| keep.is_none_or(|k| k == **split))
            .map(|(id, _)| id.as_str())
            .collect();
        if ids.is_empty() {
            continue;
        }
        let mut by_mode = BTreeMap::new();
        for &mode in modes {
            let Some(mode_scores) = scores.get(&mode) else {
                continue;
            };
            let picked: Vec<QueryScore> = mode_scores
                .iter()
                .filter(|q| ids.contains(&q.id.as_str()))
                .cloned()
                .collect();
            by_mode.insert(
                mode.id().to_string(),
                ModeScore::from_queries(mode, picked).mean.map(round),
            );
        }
        means.insert(scope.to_string(), by_mode);
    }
    let long_labels_chunked = chunked.map(|chunked| {
        long_labels
            .values()
            .flatten()
            .filter(|(label, _)| chunked.contains(label))
            .count()
    });
    Ok(LongSymbolReport {
        long_labels_chunked,
        rule: format!(
            "queries with at least one label whose body exceeds {LONG_SYMBOL_BYTES} bytes \
             (template 1's content budget for a 512-token model)"
        ),
        threshold_bytes: LONG_SYMBOL_BYTES,
        queries,
        long_labels,
        means,
    })
}

/// The slice section of the run summary.
pub fn markdown(report: &LongSymbolReport) -> String {
    let dev = report
        .queries
        .values()
        .filter(|s| **s == Split::Dev)
        .count();
    let labels: usize = report.long_labels.values().map(Vec::len).sum();
    let mut out = format!(
        "\n### Long-symbol slice (#157)\n\n{} queries ({} dev, {} held-out): {}. {} long \
         labels{}.\n",
        report.queries.len(),
        dev,
        report.queries.len() - dev,
        report.rule,
        labels,
        report
            .long_labels_chunked
            .map(|n| format!(", {n} of them embedded with body chunks"))
            .unwrap_or_default()
    );
    if let Some(all) = report.means.get("all") {
        out.push_str("\n| Mode | nDCG@10 | MRR@10 | hit@10 | recall@10 |\n| --- | ---: | ---: | ---: | ---: |\n");
        for mode in SearchMode::all() {
            if let Some(v) = all.get(mode.id()) {
                out.push_str(&format!(
                    "| `{}` | {:.4} | {:.4} | {:.4} | {:.4} |\n",
                    mode.id(),
                    v.ndcg_at_10,
                    v.mrr,
                    v.hit_at_10,
                    v.recall_at_10
                ));
            }
        }
    }
    out
}
