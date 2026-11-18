//! Loading the benchmark corpus: sources parsed into searchable symbols.
//!
//! Two corpora share one loader shape:
//!
//! - the **fixture corpus** ([`Corpus::load`]) — the in-repo
//!   `tests/fixtures/sample-*-project/` tree the golden set is labelled
//!   against;
//! - a **real repository** ([`Corpus::load_repository`]) — one checkout of the
//!   held-out real-repo set (#95), walked with the file-discovery rules of `myc
//!   analyze`.
//!
//! The lexical mode ranks these symbols in memory; the model-backed modes index
//! the very same symbols into a scratch store (see [`super::models`]), so every
//! mode searches an identical corpus.

use anyhow::{Context, Result};
use myceliums_core::config::AnalysisSection;
use myceliums_core::parser::{to_code_symbols, SourceLanguage, SourceParser};
use myceliums_core::should_skip_file;
use myceliums_storage::CodeSymbol;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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

/// Repository id the corpus symbols carry. The store-backed modes scope their
/// vector search to it, exactly as a real index scopes to its repository.
pub const CORPUS_REPO_ID: &str = "benchmark-corpus";

/// Directory names `myc analyze` never descends into (see the analyzer's
/// `discover_files`). Hidden directories (`.git`, `.github`, ...) are skipped
/// as well.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "__pycache__",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
];

/// The parsed corpus: every symbol the engine can retrieve, in a stable order.
#[derive(Debug)]
pub struct Corpus {
    /// Repository id every symbol carries; the store scopes vector search to it.
    repo_id: String,
    symbols: Vec<CodeSymbol>,
    /// Source files that were discovered but not parsed, with the reason.
    skipped: Vec<(String, String)>,
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

                symbols.extend(
                    to_code_symbols(&parsed.symbols, &relative, CORPUS_REPO_ID)
                        .into_iter()
                        .map(with_stable_uid),
                );
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

        let mut seen = BTreeSet::new();
        for symbol in &symbols {
            if !seen.insert(symbol.uid.as_str()) {
                anyhow::bail!(
                    "two corpus symbols share the identity {}::{} at line {} — the \
                     store keys symbols by uid and would silently keep only one",
                    symbol.file_path,
                    symbol.qualified_name,
                    symbol.start_line
                );
            }
        }

        Ok(Self {
            repo_id: CORPUS_REPO_ID.to_string(),
            symbols,
            skipped: Vec::new(),
        })
    }

    /// Parse one real-repository checkout the way `myc analyze` reads it.
    ///
    /// Files are discovered with the analyzer's rules — hidden directories and
    /// `node_modules`, `__pycache__`, `target`, `dist`, `build`, `.venv`,
    /// `venv` are not entered, symlinks are not followed, the language comes
    /// from the extension — and filtered with the production
    /// [`should_skip_file`] under the default `[analysis]` settings (512 KB,
    /// 5 120-byte lines, minified bundles). Like the fixture corpus, content
    /// files (Markdown, plain text) and notebooks are not corpus: the labels
    /// are code symbols, and prose sections are a different retrieval problem.
    ///
    /// A file the parser rejects is skipped and recorded, as `myc analyze`
    /// skips it with a warning; [`skipped`](Self::skipped) lists them so the
    /// report can say what the corpus does not contain. Paths are relative to
    /// `root`, forward-slashed — the form the real-repo labels use.
    pub fn load_repository(root: &Path, repo_id: &str) -> Result<Self> {
        if !root.is_dir() {
            anyhow::bail!("repository checkout missing: {}", root.display());
        }
        let analysis = AnalysisSection::default();
        let mut files: Vec<(PathBuf, SourceLanguage)> = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                entry.depth() == 0
                    || !(name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name.as_ref()))
            })
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && !entry.path_is_symlink())
            .filter_map(|entry| {
                let language = entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(SourceLanguage::from_extension)?;
                Some((entry.into_path(), language))
            })
            .filter(|(_, language)| !language.is_content() && *language != SourceLanguage::Jupyter)
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut symbols = Vec::new();
        let mut skipped = Vec::new();
        for (file, language) in files {
            let relative = fixture_relative_path(root, &file)?;
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading repository file {}", file.display()))?;
            if bytes.is_empty() {
                skipped.push((relative, "empty file".to_string()));
                continue;
            }
            if let Some(reason) = should_skip_file(&file, &bytes, &analysis) {
                skipped.push((relative, reason.to_string()));
                continue;
            }
            let Ok(source) = String::from_utf8(bytes) else {
                skipped.push((relative, "not UTF-8".to_string()));
                continue;
            };
            let parsed = SourceParser::new(language).and_then(|mut parser| parser.parse(&source));
            match parsed {
                Ok(parsed) => symbols.extend(
                    to_code_symbols(&parsed.symbols, &relative, repo_id)
                        .into_iter()
                        .map(with_stable_uid),
                ),
                Err(error) => skipped.push((relative, format!("parse error: {error}"))),
            }
        }
        if symbols.is_empty() {
            anyhow::bail!(
                "repository {} contributed no symbols — an empty checkout would \
                 score every query zero",
                root.display()
            );
        }
        disambiguate_uids(&mut symbols);
        Ok(Self {
            repo_id: repo_id.to_string(),
            symbols,
            skipped,
        })
    }

    /// Repository id the symbols carry.
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    /// Discovered source files that were not parsed, with the reason —
    /// always empty for the fixture corpus, which fails on any such file.
    pub fn skipped(&self) -> &[(String, String)] {
        &self.skipped
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

/// Replace the parser's random uid with one derived from the symbol's identity.
///
/// The parser mints a fresh UUID per symbol per parse. Retrieval never reads
/// the value, but it does key on it: the store upserts by uid, and fusion
/// matches the lexical and vector legs by uid. A uid that changes every run
/// would let anything that iterates or orders by it vary between two runs over
/// the same tree, so the corpus uses a digest of file, qualified name, kind and
/// start line instead — opaque like a UUID, but the same on every run.
fn with_stable_uid(mut symbol: CodeSymbol) -> CodeSymbol {
    let identity = format!(
        "{}\0{}\0{}\0{}",
        symbol.file_path, symbol.qualified_name, symbol.kind, symbol.start_line
    );
    let digest = Sha256::digest(identity.as_bytes());
    symbol.uid = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    symbol
}

/// Make every uid unique while keeping it stable.
///
/// Real code occasionally yields two symbols with the same file, qualified
/// name, kind and start line (two assignments on one line, an overload the
/// grammar reports twice). The store upserts by uid and would keep only one,
/// so a repeated identity gets an ordinal mixed into its digest — the n-th
/// repeat in parse order, which is as stable as the parse itself.
fn disambiguate_uids(symbols: &mut [CodeSymbol]) {
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for symbol in symbols.iter_mut() {
        let repeats = seen.entry(symbol.uid.clone()).or_insert(0);
        if *repeats > 0 {
            let digest = Sha256::digest(format!("{}\0{}", symbol.uid, repeats).as_bytes());
            symbol.uid = digest[..16]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        }
        *repeats += 1;
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
    fn a_repository_is_read_with_the_analyzer_discovery_rules() {
        let root = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("pkg/core.py", "def wanted():\n    return 1\n");
        write("node_modules/dep/index.js", "function vendored() {}\n");
        write(".hidden/secret.py", "def hidden():\n    pass\n");
        write("build/out.py", "def generated():\n    pass\n");
        write("docs/guide.md", "# Guide\n\nprose\n");
        write("static/app.min.js", "function minified() {}\n");
        write("pkg/empty.py", "");

        let corpus = Corpus::load_repository(root.path(), "demo").unwrap();
        let names: Vec<String> = corpus
            .symbols()
            .iter()
            .map(|s| symbol_ref(s).to_string())
            .collect();
        assert_eq!(names, ["pkg/core.py::wanted"]);
        assert!(corpus.symbols().iter().all(|s| s.repo_id == "demo"));
        assert_eq!(corpus.repo_id(), "demo");
        let skipped: Vec<&str> = corpus.skipped().iter().map(|(f, _)| f.as_str()).collect();
        assert!(skipped.contains(&"static/app.min.js"), "{skipped:?}");
        assert!(skipped.contains(&"pkg/empty.py"), "{skipped:?}");
    }

    #[test]
    fn repeated_identities_get_distinct_stable_uids() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.py"), "def f():\n    pass\n").unwrap();
        let corpus = Corpus::load_repository(root.path(), "demo").unwrap();
        let mut symbols = vec![corpus.symbols()[0].clone(), corpus.symbols()[0].clone()];
        disambiguate_uids(&mut symbols);
        assert_ne!(symbols[0].uid, symbols[1].uid);
        assert_eq!(
            symbols[0].uid,
            corpus.symbols()[0].uid,
            "the first keeps its uid"
        );
        let mut again = vec![corpus.symbols()[0].clone(), corpus.symbols()[0].clone()];
        disambiguate_uids(&mut again);
        assert_eq!(symbols[1].uid, again[1].uid);
    }

    #[test]
    fn an_empty_repository_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(Corpus::load_repository(root.path(), "demo").is_err());
        assert!(Corpus::load_repository(&root.path().join("missing"), "demo").is_err());
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
    fn corpus_uids_are_stable_and_unique() {
        let root = fixtures_root().unwrap();
        let first = Corpus::load(&root).unwrap();
        let second = Corpus::load(&root).unwrap();
        let uids =
            |c: &Corpus| -> Vec<String> { c.symbols().iter().map(|s| s.uid.clone()).collect() };
        assert_eq!(uids(&first), uids(&second));
        let distinct: BTreeSet<String> = uids(&first).into_iter().collect();
        assert_eq!(distinct.len(), first.len());
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
