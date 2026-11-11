//! Retrieval-quality evaluation: how well does search actually find the right code?
//!
//! The harness answers one question with numbers instead of opinion — given a
//! query an agent would realistically type, does the engine return the symbol
//! that answers it, and how near the top?
//!
//! - [`golden_set`] — the labelled ground truth (`golden/queries.json`).
//! - [`corpus`] — the fixture sources parsed into searchable symbols.
//! - [`models`] — the embedding index the model-backed modes search, built and
//!   queried through the production code paths; weights download only on
//!   explicit opt-in.
//! - [`metrics`] — recall@k, hit@k, reciprocal rank, nDCG@k, R-precision.
//! - [`evaluator`] — runs a [`evaluator::SearchMode`] over the golden set and scores it.
//! - [`bootstrap`] — paired bootstrap confidence intervals for differences.
//! - [`report`] — the report/baseline file format and mode-vs-mode comparisons.
//! - [`gate`] — the CI decision rule against the recorded baseline.
//!
//! Run it with `cargo run -p myceliums-benchmarks --bin retrieval-eval`; add
//! `-- --allow-model-download` (or set `MYC_EVAL_MODELS=1`) to score the
//! semantic, hybrid and hybrid+rerank modes as well.
//!
//! The lexical mode is offline and deterministic: it parses in-repo fixtures and
//! its ranking depends only on the corpus and the query text. The model-backed
//! modes are reproducible on one platform; see `benchmarks/METHODOLOGY.md` for
//! what that does and does not guarantee across platforms.

pub mod bootstrap;
pub mod corpus;
pub mod evaluator;
pub mod gate;
pub mod golden_set;
pub mod metrics;
pub mod models;
pub mod report;

/// Guards on the committed `golden/baseline.json` — tests only, no public API.
#[cfg(test)]
mod baseline;

pub use evaluator::{
    evaluate, Metric, MetricValues, ModeScore, QueryScore, Retrieval, SearchMode, RANKING_DEPTH,
    RECALL_CUTOFFS,
};
pub use golden_set::{GoldenQuery, GoldenSet, QueryIntent, SymbolRef};
pub use metrics::{
    hit_at_k, mean, mrr, ndcg_at_k, r_precision, recall_at_k, reciprocal_rank, RelevantSet,
};
