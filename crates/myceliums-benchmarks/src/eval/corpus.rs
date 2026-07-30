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
#[derive(Debug)]
pub struct Corpus {
    symbols: Vec<CodeSymbol>,
}

impl Corpus {
    /// Parse the fixture corpus rooted at `fixtures_root` (`tests/fixtures`).
    ///
    /// Files are visited in sorted order and symbols keep their parse order, so
    /// two runs over an unchanged tree produce byte-identical rankings — the
    /// determinism the acceptance criteria require.
    ///
    /// Every project in [`CORPUS_PROJECTS`] must contribute at least one
    /// symbol. A renamed, emptied or unparseable fixture would otherwise shrink
    /// the corpus and move every reported metric with no visible cause, which
    /// is exactly the silent regression this benchmark exists to catch.
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

            let before = symbols.len();
            for file in files {
                let Some(language) = file
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(SourceLanguage::from_extension)
                else {
                    continue;
                };
                // Content files (Markdown, JSON, ...) hold prose, not symbols.
                // They are not retrievable answers, so they are not corpus.
                if language.is_content() {
                    continue;
                }

                let source = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading corpus file {}", file.display()))?;
                let relative = fixture_relative_path(fixtures_root, &file)?;

                let mut parser = SourceParser::new(language)
                    .with_context(|| format!("creating parser for {relative}"))?;
                let parsed = parser
                    .parse(&source)
                    .with_context(|| format!("parsing {relative}"))?;

                symbols.extend(to_code_symbols(
                    &parsed.symbols,
                    &relative,
                    "benchmark-corpus",
                ));
            }

            if symbols.len() == before {
                anyhow::bail!(
                    "corpus project '{}' contributed no symbols ({}). \
                     Every configured project must be retrievable — an empty one \
                     silently moves every reported metric.",
                    project,
                    project_dir.display()
                );
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

/// The fixture-relative, forward-slashed path a golden label refers to.
///
/// Labels are `(file, qualified_name)` pairs written against this exact form,
/// so a path that is not under `fixtures_root` is a loading bug, not a case to
/// paper over — every caller built the path by joining onto that root.
fn fixture_relative_path(fixtures_root: &Path, file: &Path) -> Result<String> {
    let relative = file.strip_prefix(fixtures_root).with_context(|| {
        format!(
            "corpus file {} is not under the fixtures root {}",
            file.display(),
            fixtures_root.display()
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
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

    /// Floor on corpus size. The five fixture projects yield 96 symbols today;
    /// 40 is low enough that ordinary fixture edits do not trip it, and high
    /// enough that a project silently dropping out of the corpus does.
    const MINIMUM_CORPUS_SYMBOLS: usize = 40;

    #[test]
    fn corpus_parses_all_sample_projects() {
        let corpus = Corpus::load(&fixtures_root().unwrap()).expect("corpus must load");
        assert!(
            corpus.len() > MINIMUM_CORPUS_SYMBOLS,
            "expected a non-trivial corpus, got {} symbols",
            corpus.len()
        );
    }

    #[test]
    fn a_project_contributing_nothing_is_an_error() {
        // A fixture root with the right directory names but no source files is
        // exactly the shape a renamed or emptied fixture takes. It must fail
        // loudly rather than report a smaller corpus and different metrics.
        let empty_root = tempfile::tempdir().unwrap();
        for project in CORPUS_PROJECTS {
            std::fs::create_dir_all(empty_root.path().join(project)).unwrap();
        }
        let error = Corpus::load(empty_root.path())
            .expect_err("an empty corpus project must not load quietly");
        assert!(error.to_string().contains("contributed no symbols"));
    }

    #[test]
    fn fixture_paths_are_relative_and_forward_slashed() {
        let root = Path::new("/tmp/fixtures");
        let path = fixture_relative_path(root, &root.join("sample-py-project/utils/helpers.py"))
            .expect("a path under the root resolves");
        assert_eq!(path, "sample-py-project/utils/helpers.py");
        assert!(
            fixture_relative_path(root, Path::new("/elsewhere/stray.py")).is_err(),
            "a path outside the fixtures root is a loading bug, not a fallback"
        );
    }

    #[test]
    fn backslash_separators_are_normalised() {
        // The golden set stores one canonical spelling of a path, so a
        // Windows-shaped separator must not yield a second SymbolRef for the
        // same file. On Unix a backslash is an ordinary filename character,
        // which is exactly why this needs an explicit case: building the input
        // with `Path::join` and forward slashes never reaches the replacement.
        let root = Path::new("/tmp/fixtures");
        let windows_style = Path::new(r"/tmp/fixtures/sample-py-project\utils\helpers.py");
        assert_eq!(
            fixture_relative_path(root, windows_style).unwrap(),
            "sample-py-project/utils/helpers.py"
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
