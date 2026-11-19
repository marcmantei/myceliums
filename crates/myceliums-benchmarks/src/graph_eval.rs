//! Graph-accuracy evaluation: are the extracted symbols and CALLS edges right?
//!
//! The retrieval evaluation ([`crate::eval`]) asks whether search finds the
//! right code; this one asks whether the graph underneath is correct. It runs
//! the production analyzer over small fixture projects in five languages
//! (`tests/fixtures/graph-accuracy/`) and scores what it extracts against a
//! hand-labelled gold set (`graph-gold/*.json`):
//!
//! - symbol precision and recall,
//! - CALLS-edge precision, recall and F1 per language,
//! - the same per resolution label (`exact`, `import`, `name-match`,
//!   `ambiguous`) — does a label mean what it says?
//! - every false positive and negative, by name.
//!
//! Two setups: each fixture analyzed alone (`isolated`), and all of them as
//! one repository (`polyglot`), where same-named symbols of different
//! languages can collide.
//!
//! - [`gold`] — the labels and their content digest.
//! - [`extract`] — the analyzer run.
//! - [`score`] — matching and counting.
//! - [`report`] — report/baseline format and Markdown.
//!
//! Run it with `cargo run -p myceliums-benchmarks --bin graph-eval`. It is
//! offline and needs no model weights. See `graph-gold/README.md` for the
//! labelling rules.

pub mod extract;
pub mod gold;
pub mod report;
pub mod score;

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

use gold::{GoldSet, FIXTURE_DIR};
use report::{platform, RunReport, SetupReport, ISOLATED, POLYGLOT, SCHEMA_VERSION};

/// Analyze the fixtures under `fixtures_root` (`tests/fixtures`) in both
/// setups and score them against `gold`.
pub async fn run(gold: &GoldSet, fixtures_root: &Path) -> Result<RunReport> {
    let root = fixtures_root.join(FIXTURE_DIR);

    let mut isolated = Vec::new();
    for language in &gold.languages {
        let extraction = extract::extract(&fixtures_root.join(&language.fixture)).await?;
        isolated.push((
            language.language.clone(),
            score::score_language(language, &extraction, ""),
        ));
    }

    let shared = extract::extract(&root).await?;
    let polyglot: Vec<_> = gold
        .languages
        .iter()
        .map(|language| {
            let prefix = format!(
                "{}/",
                language
                    .fixture
                    .strip_prefix(FIXTURE_DIR)
                    .unwrap_or(&language.fixture)
                    .trim_start_matches('/')
            );
            (
                language.language.clone(),
                score::score_language(language, &shared, &prefix),
            )
        })
        .collect();

    let reviewed_by = gold
        .languages
        .iter()
        .map(|l| l.reviewed_by.clone())
        .reduce(|a, b| if a == b { a } else { None })
        .flatten();
    let labelled_by = gold
        .languages
        .first()
        .map(|l| l.labelled_by.clone())
        .unwrap_or_default();

    Ok(RunReport {
        schema_version: SCHEMA_VERSION.to_string(),
        regenerate_with: None,
        timestamp: None,
        commit_sha: None,
        dataset_digest: gold.digest(fixtures_root)?,
        labelled_by,
        reviewed_by,
        platform: platform(),
        setups: BTreeMap::from([
            (ISOLATED.to_string(), SetupReport::new(&isolated)),
            (POLYGLOT.to_string(), SetupReport::new(&polyglot)),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::corpus::fixtures_root;

    /// The committed baseline must describe the current labels and fixtures;
    /// otherwise its numbers belong to a different dataset.
    #[test]
    fn baseline_matches_the_current_gold_digest() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("graph-gold/baseline.json");
        let baseline = RunReport::read(&path).unwrap();
        let gold = GoldSet::embedded().unwrap();
        assert_eq!(
            baseline.dataset_digest,
            gold.digest(&fixtures_root().unwrap()).unwrap(),
            "graph-gold/baseline.json was recorded against other labels or fixtures; \
             re-record it with `{}`",
            report::REGENERATE_BASELINE
        );
        assert!(baseline.commit_sha.is_some() && baseline.timestamp.is_some());
    }

    /// Every gold symbol's line must name the symbol: a shifted fixture or a
    /// typo in the gold would otherwise turn into a silent false negative.
    #[test]
    fn every_gold_symbol_line_names_the_symbol() {
        let fixtures = fixtures_root().unwrap();
        for language in GoldSet::embedded().unwrap().languages {
            let dir = fixtures.join(&language.fixture);
            for symbol in &language.symbols {
                let source = std::fs::read_to_string(dir.join(symbol.file()))
                    .unwrap_or_else(|e| panic!("{}: {e}", symbol.id));
                let line = source
                    .lines()
                    .nth(symbol.line as usize - 1)
                    .unwrap_or_default();
                assert!(
                    line.contains(symbol.name()),
                    "{}: line {} is `{line}`",
                    symbol.id,
                    symbol.line
                );
            }
            // The call line must name the callee, unless the gold marks the
            // call as made without its name (`cls()`, an operator, ...).
            let symbols = language.symbol_map();
            for call in &language.calls {
                let caller = symbols[call.from.as_str()];
                let source = std::fs::read_to_string(dir.join(caller.file())).unwrap();
                let line = source
                    .lines()
                    .nth(call.line as usize - 1)
                    .unwrap_or_default();
                let named = call.targets().any(|t| line.contains(symbols[t].name()));
                assert!(
                    named || call.tags.iter().any(|t| t == "indirect"),
                    "{} -> {}: line {} is `{line}`",
                    call.from,
                    call.to,
                    call.line
                );
            }
        }
    }

    /// The evaluator runs end to end on the real fixtures and is
    /// deterministic on one machine: two runs give the same report.
    #[tokio::test]
    async fn two_runs_agree() {
        let gold = GoldSet::embedded().unwrap();
        let fixtures = fixtures_root().unwrap();
        let first = run(&gold, &fixtures).await.unwrap();
        let second = run(&gold, &fixtures).await.unwrap();
        assert_eq!(first, second);
        for language in &gold.languages {
            let summary = &first.setups[ISOLATED].summary[&language.language];
            assert_eq!(summary.calls.gold, language.calls.len());
            assert_eq!(summary.symbols.gold, language.symbols.len());
        }
    }
}
