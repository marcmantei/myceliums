//! Loading the benchmark corpus: fixture sources parsed into searchable symbols.
//!
//! The corpus is the existing `tests/fixtures/sample-*-project/` tree. Parsing
//! it directly (rather than indexing into a store) keeps the evaluation
//! offline, deterministic and free of embedding-model downloads.

use anyhow::{Context, Result};
use myceliums_core::parser::{to_code_symbols, SourceLanguage, SourceParser};
use myceliums_storage::CodeSymbol;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::golden_set::SymbolRef;

/// The fixture projects that make up the benchmark corpus.
///
/// Restricted to the multi-file `sample-*-project` fixtures: they contain
/// realistic cross-file structure (services, utils, entry points), which is
/// what retrieval quality should be measured on. Single-file grammar fixtures
/// would inflate scores with trivially unique names.
const CORPUS_PROJECTS: &[&str] = &[
    "sample-py-project",
    "sample-ts-project",
    "sample-js-project",
    "sample-go-project",
    "sample-csharp-project",
];

/// The parsed corpus: every symbol the engine can retrieve, in a stable order.
pub struct Corpus {
    symbols: Vec<CodeSymbol>,
}

impl Corpus {
    /// Parse the fixture corpus rooted at `fixtures_root` (`tests/fixtures`).
    ///
    /// Files are visited in sorted order and symbols keep their parse order, so
    /// two runs over an unchanged tree produce byte-identical rankings — the
    /// determinism the acceptance criteria require.
    pub fn load(fixtures_root: &Path) -> Result<Self> {
        let mut symbols = Vec::new();

        for project in CORPUS_PROJECTS {
            let project_dir = fixtures_root.join(project);
            if !project_dir.is_dir() {
                anyhow::bail!("corpus project missing: {}", project_dir.display());
            }

            let mut files: Vec<PathBuf> = WalkDir::new(&project_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| entry.into_path())
                .collect();
            files.sort();

            for file in files {
                let Some(language) = file
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(SourceLanguage::from_extension)
                else {
                    continue;
                };
                if language.is_content() {
                    continue;
                }

                let source = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading corpus file {}", file.display()))?;
                let relative = file
                    .strip_prefix(fixtures_root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");

                let mut parser = SourceParser::new(language)
                    .with_context(|| format!("creating parser for {}", relative))?;
                let parsed = parser
                    .parse(&source)
                    .with_context(|| format!("parsing {}", relative))?;

                symbols.extend(to_code_symbols(
                    &parsed.symbols,
                    &relative,
                    "benchmark-corpus",
                ));
            }
        }

        Ok(Self { symbols })
    }

    /// The corpus symbols, as handed to the search engine.
    pub fn symbols(&self) -> &[CodeSymbol] {
        &self.symbols
    }

    /// Number of retrievable symbols.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// True when nothing was parsed.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// True when the corpus contains the symbol a golden label points at.
    ///
    /// Used to fail loudly when a fixture is renamed out from under the labels,
    /// instead of quietly reporting a recall regression.
    pub fn contains(&self, reference: &SymbolRef) -> bool {
        self.symbols
            .iter()
            .any(|symbol| symbol_ref(symbol) == *reference)
    }
}

/// Project a corpus symbol onto its stable golden-set reference.
pub fn symbol_ref(symbol: &CodeSymbol) -> SymbolRef {
    SymbolRef::new(symbol.file_path.clone(), symbol.qualified_name.clone())
}

/// Locate `tests/fixtures` by walking up from this crate's manifest directory.
///
/// Keeps `cargo run -p myceliums-benchmarks` working from any directory.
pub fn fixtures_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .context("locating repository root from crate manifest")?;
    let fixtures = root.join("tests").join("fixtures");
    if !fixtures.is_dir() {
        anyhow::bail!("fixtures directory not found at {}", fixtures.display());
    }
    Ok(fixtures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_parses_all_sample_projects() {
        let corpus = Corpus::load(&fixtures_root().unwrap()).expect("corpus must load");
        assert!(
            corpus.len() > 40,
            "expected a non-trivial corpus, got {} symbols",
            corpus.len()
        );
    }

    #[test]
    fn corpus_loading_is_deterministic() {
        let root = fixtures_root().unwrap();
        let first = Corpus::load(&root).unwrap();
        let second = Corpus::load(&root).unwrap();
        let names = |c: &Corpus| -> Vec<String> {
            c.symbols()
                .iter()
                .map(|s| format!("{}::{}", s.file_path, s.qualified_name))
                .collect()
        };
        assert_eq!(names(&first), names(&second));
    }

    #[test]
    fn every_golden_label_exists_in_the_corpus() {
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let set = crate::eval::golden_set::GoldenSet::embedded().unwrap();
        for query in &set.queries {
            for reference in &query.relevant {
                assert!(
                    corpus.contains(reference),
                    "query '{}' labels '{}', which is not in the corpus",
                    query.id,
                    reference
                );
            }
        }
    }
}
