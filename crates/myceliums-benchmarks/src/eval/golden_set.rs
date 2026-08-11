//! The golden dataset: queries paired with human-labelled relevant symbols.
//!
//! A golden set is the ground truth against which retrieval quality is
//! measured. It is deliberately data, not code: `golden/queries.json` is
//! versioned in the crate so that changing a label is a reviewable diff, and so
//! that a regression in the numbers can always be traced to either a code
//! change or a label change — never to an invisible one.
//!
//! Labelling criteria are documented in `benchmarks/METHODOLOGY.md`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// The golden set shipped in-tree, embedded so the evaluator runs offline
/// without depending on the current working directory.
const EMBEDDED_GOLDEN_SET: &str = include_str!("../../golden/queries.json");

/// Dataset layouts this loader understands. Bumped only on a breaking change
/// to the file's shape, so an old harness fails loudly against a new dataset
/// instead of silently mis-reading labels.
const SUPPORTED_SCHEMA_VERSION: &str = "1.0";

/// A reference to one symbol in the corpus, stable across re-indexing.
///
/// Symbol UIDs are regenerated (as fresh UUIDs) on every parse, so they cannot
/// identify a labelled answer across runs. The pair `(file, qualified_name)` is
/// stable for as long as the fixture file is unchanged, which is exactly the
/// property a golden label needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolRef {
    /// Fixture-relative path of the file the symbol lives in.
    pub file: String,
    /// Fully qualified symbol name (`UserService.get_user`, `formatName`, ...).
    pub symbol: String,
}

impl SymbolRef {
    /// Build a reference from a file path and qualified symbol name.
    pub fn new(file: impl Into<String>, symbol: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            symbol: symbol.into(),
        }
    }
}

impl std::fmt::Display for SymbolRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.file, self.symbol)
    }
}

/// What a query is testing, so failures can be read by category rather than
/// one query at a time.
///
/// # Vocabulary
///
/// The wire values are **kebab-case** (`exact-name`, `paraphrase`,
/// `behavioural`, `conceptual`) and use **British spelling**, matching the
/// prose in this crate and the rest of the engine's own source. The set is
/// closed: serde rejects any value outside it, so a typo in `queries.json`
/// fails at load rather than silently creating a fifth category that no report
/// aggregates. `intent_vocabulary_is_kebab_case` pins the convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum QueryIntent {
    /// The query names the symbol almost exactly (`formatName`).
    ExactName,
    /// The query uses different words for the same idea ("tidy up a person's name").
    Paraphrase,
    /// The query describes what the code does, not what it is called.
    Behavioural,
    /// The query names a concept spread over several symbols ("user CRUD").
    Conceptual,
}

impl QueryIntent {
    /// Short label used in reports.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ExactName => "exact-name",
            Self::Paraphrase => "paraphrase",
            Self::Behavioural => "behavioural",
            Self::Conceptual => "conceptual",
        }
    }
}

/// One labelled query: the text an agent would type, and the symbols that
/// genuinely answer it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenQuery {
    /// Stable identifier, used to name the query in reports and diffs.
    pub id: String,
    /// The query text, exactly as it is handed to the search engine.
    pub query: String,
    /// What this query is probing.
    pub intent: QueryIntent,
    /// Symbols a human judged relevant. Never empty.
    pub relevant: Vec<SymbolRef>,
    /// Why these symbols and not others — the labelling rationale.
    pub rationale: String,
}

/// The full dataset: versions, a corpus root, and the labelled queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenSet {
    /// Layout version of the file itself; see [`SUPPORTED_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Content version, bumped whenever labels change, and recorded in the
    /// baseline so a score can never be compared across different ground truth.
    pub dataset_version: String,
    /// Corpus root, relative to the repository root.
    pub corpus_root: String,
    /// The labelled queries.
    pub queries: Vec<GoldenQuery>,
}

impl GoldenSet {
    /// Load the golden set that ships with this crate.
    ///
    /// This is the path the evaluator uses: no filesystem lookup, no working
    /// directory assumption, so `cargo run` behaves the same everywhere.
    pub fn embedded() -> Result<Self> {
        let set: Self = serde_json::from_str(EMBEDDED_GOLDEN_SET)
            .context("embedded golden_set.json is not valid GoldenSet JSON")?;
        set.validate()?;
        Ok(set)
    }

    /// Content digest of the golden set exactly as it ships.
    ///
    /// Hashes the embedded file's own bytes rather than a re-serialisation, so
    /// the value cannot drift with serde field ordering or JSON formatting.
    ///
    /// This is what the baseline records to prove it was measured against the
    /// ground truth it is compared to. `dataset_version` states the same claim
    /// but is typed by hand, so it stays correct only as long as nobody forgets
    /// to bump it; the digest cannot be forgotten.
    pub fn embedded_digest() -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(EMBEDDED_GOLDEN_SET.as_bytes()))
    }

    /// Load a golden set from a JSON file, for experimenting with alternative
    /// label sets without rebuilding.
    pub fn from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading golden set {}", path.display()))?;
        let set: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing golden set {}", path.display()))?;
        set.validate()?;
        Ok(set)
    }

    /// Reject datasets that would produce meaningless metrics.
    ///
    /// An unlabelled or duplicated query silently distorts every aggregate, so
    /// it is a hard error rather than a warning.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            bail!(
                "golden set schema version '{}' is not supported (this harness reads '{}')",
                self.schema_version,
                SUPPORTED_SCHEMA_VERSION
            );
        }
        if self.queries.is_empty() {
            bail!("golden set contains no queries");
        }
        let mut seen = BTreeSet::new();
        for query in &self.queries {
            if query.relevant.is_empty() {
                bail!("query '{}' has no relevant symbols labelled", query.id);
            }
            if !seen.insert(query.id.as_str()) {
                bail!("duplicate query id '{}'", query.id);
            }
        }
        Ok(())
    }

    /// Number of labelled queries.
    pub fn len(&self) -> usize {
        self.queries.len()
    }

    /// True when the set has no queries.
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_set_loads_and_validates() {
        let set = GoldenSet::embedded().expect("embedded golden set must be valid");
        assert!(
            set.len() >= 30,
            "issue #34 asks for 30-60 queries, found {}",
            set.len()
        );
        assert!(set.len() <= 60, "golden set larger than agreed bound");
    }

    #[test]
    fn query_ids_describe_the_query() {
        // Ids are descriptive kebab-case slugs prefixed by their intent, not an
        // opaque sequence. A slug survives insertion and retirement without
        // renumbering, so there is no gap for a reader to interpret — and a
        // query that scores badly names itself in the report, instead of
        // sending the reader back to the dataset to find out what "q37" was.
        let set = GoldenSet::embedded().unwrap();
        for query in &set.queries {
            assert!(
                query
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not kebab-case",
                query.id
            );
            assert!(
                query.id.starts_with(query.intent.label()),
                "{} should be prefixed with its intent {}",
                query.id,
                query.intent.label()
            );
        }
    }

    #[test]
    fn every_query_is_labelled_and_explained() {
        let set = GoldenSet::embedded().unwrap();
        for query in &set.queries {
            assert!(
                !query.query.trim().is_empty(),
                "{} has empty text",
                query.id
            );
            assert!(
                !query.rationale.trim().is_empty(),
                "{} has no labelling rationale",
                query.id
            );
        }
    }

    #[test]
    fn every_intent_category_is_covered() {
        let set = GoldenSet::embedded().unwrap();
        let intents: BTreeSet<_> = set.queries.iter().map(|q| q.intent).collect();
        for intent in [
            QueryIntent::ExactName,
            QueryIntent::Paraphrase,
            QueryIntent::Behavioural,
            QueryIntent::Conceptual,
        ] {
            assert!(intents.contains(&intent), "no {:?} queries", intent);
        }
    }

    #[test]
    fn intent_vocabulary_is_kebab_case() {
        // One convention, pinned: kebab-case on the wire, British spelling.
        // `label()` feeds reports and the serde representation feeds the
        // dataset, so the two must not drift apart.
        for (intent, expected) in [
            (QueryIntent::ExactName, "exact-name"),
            (QueryIntent::Paraphrase, "paraphrase"),
            (QueryIntent::Behavioural, "behavioural"),
            (QueryIntent::Conceptual, "conceptual"),
        ] {
            assert_eq!(intent.label(), expected);
            assert_eq!(
                serde_json::to_string(&intent).unwrap(),
                format!("\"{expected}\""),
                "the dataset value and the report label must agree"
            );
            assert!(
                expected.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{expected} is not kebab-case"
            );
        }
    }

    #[test]
    fn an_unknown_intent_is_rejected() {
        // A typo must fail at load rather than quietly becoming a fifth
        // category that no aggregate reports on.
        assert!(serde_json::from_str::<QueryIntent>("\"exact_name\"").is_err());
        assert!(serde_json::from_str::<QueryIntent>("\"behavioral\"").is_err());
    }

    #[test]
    fn unlabelled_queries_are_rejected() {
        let set = GoldenSet {
            schema_version: SUPPORTED_SCHEMA_VERSION.into(),
            dataset_version: "test".into(),
            corpus_root: "tests/fixtures".into(),
            queries: vec![GoldenQuery {
                id: "q1".into(),
                query: "anything".into(),
                intent: QueryIntent::ExactName,
                relevant: vec![],
                rationale: "none".into(),
            }],
        };
        assert!(set.validate().is_err());
    }

    #[test]
    fn unsupported_schema_versions_are_rejected() {
        let mut set = GoldenSet::embedded().unwrap();
        set.schema_version = "99.0".into();
        assert!(
            set.validate().is_err(),
            "a future schema must fail loudly, not be read with today's rules"
        );
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let query = GoldenQuery {
            id: "dup".into(),
            query: "anything".into(),
            intent: QueryIntent::ExactName,
            relevant: vec![SymbolRef::new("a.py", "f")],
            rationale: "r".into(),
        };
        let set = GoldenSet {
            schema_version: SUPPORTED_SCHEMA_VERSION.into(),
            dataset_version: "test".into(),
            corpus_root: "tests/fixtures".into(),
            queries: vec![query.clone(), query],
        };
        assert!(set.validate().is_err());
    }
}
