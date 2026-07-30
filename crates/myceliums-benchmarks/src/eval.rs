//! Retrieval-quality evaluation: how well does search actually find the right code?
//!
//! The harness answers one question with numbers instead of opinion — given a
//! query an agent would realistically type, does the engine return the symbol
//! that answers it, and how near the top?
//!
//! Four pieces, each with one job:
//!
//! - [`golden_set`] — the labelled ground truth (`golden/queries.json`).
//! - [`corpus`] — the fixture sources parsed into searchable symbols.
//! - [`metrics`] — recall@k and MRR as pure functions.
//! - [`evaluator`] — runs a [`evaluator::SearchMode`] over the golden set and scores it.
//!
//! Run it with `cargo run -p myceliums-benchmarks --bin retrieval-eval`.
//!
//! The evaluation is offline and deterministic: it parses in-repo fixtures,
//! downloads nothing, and its ranking depends only on the corpus and the query
//! text. Two runs over an unchanged tree produce byte-identical output.

pub mod corpus;
pub mod evaluator;
pub mod golden_set;
pub mod metrics;

/// Guards on the committed `golden/baseline.json` — tests only, no public API.
#[cfg(test)]
mod baseline;

pub use evaluator::{evaluate, ModeScore, QueryScore, SearchMode, RECALL_CUTOFFS};
pub use golden_set::{GoldenQuery, GoldenSet, QueryIntent, SymbolRef};
pub use metrics::{mean, mrr, recall_at_k, reciprocal_rank, RelevantSet};
