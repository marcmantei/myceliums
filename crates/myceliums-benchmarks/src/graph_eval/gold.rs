//! The hand-labelled gold set: symbols and CALLS edges per fixture language.
//!
//! One JSON file per language under `graph-gold/`, embedded at compile time so
//! the scores and the digest always describe the same labels. Each file names
//! the fixture directory (under `tests/fixtures/`) it describes.
//!
//! Identifiers are `<fixture-relative file>::<Container.name>`, e.g.
//! `shop/service.py::UserService.register`. They are the labeller's own
//! names, written from reading the code: the evaluator never matches them
//! against myceliums' qualified names, only against file, short name and line
//! (see [`super::score`]).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// The gold files, in report order: `(language, json)`.
const EMBEDDED: &[(&str, &str)] = &[
    ("python", include_str!("../../graph-gold/python.json")),
    (
        "typescript",
        include_str!("../../graph-gold/typescript.json"),
    ),
    ("go", include_str!("../../graph-gold/go.json")),
    ("rust", include_str!("../../graph-gold/rust.json")),
    ("java", include_str!("../../graph-gold/java.json")),
];

/// Directory under `tests/fixtures/` that holds every gold fixture.
pub const FIXTURE_DIR: &str = "graph-accuracy";

/// A defined symbol the extractor should find.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldSymbol {
    /// `<file>::<Container.name>`.
    pub id: String,
    /// The labeller's kind: function, method, class, struct, interface,
    /// trait, enum.
    pub kind: String,
    /// 1-based line of the symbol's name in its definition.
    pub line: u32,
}

impl GoldSymbol {
    /// The fixture-relative file.
    pub fn file(&self) -> &str {
        split_id(&self.id).0
    }

    /// The short name: the last segment of the qualified part, without the
    /// parameter list that tells overloads apart (`Repository.add(T, int)`).
    pub fn name(&self) -> &str {
        let qualified = split_id(&self.id).1;
        let qualified = qualified
            .split_once('(')
            .map_or(qualified, |(head, _)| head);
        qualified.rsplit('.').next().unwrap_or(qualified)
    }
}

/// A call edge between two gold symbols.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldCall {
    pub from: String,
    pub to: String,
    /// Other targets that count as this same edge: a class's explicit
    /// constructor for an instantiation, an override for a dispatched call.
    #[serde(default)]
    pub also_accept: Vec<String>,
    /// First call site, for the reader.
    pub line: u32,
    /// What makes the edge easy or hard (`alias`, `same-name`, ...).
    #[serde(default)]
    pub tags: Vec<String>,
    /// One-line rationale.
    pub why: String,
}

impl GoldCall {
    /// Every target that satisfies this edge, primary first.
    pub fn targets(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.to.as_str()).chain(self.also_accept.iter().map(String::as_str))
    }
}

/// A runtime-only call (through a function value): not scored either way.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicCall {
    pub from: String,
    pub to: String,
    pub why: String,
}

/// A call site that must NOT produce an edge to a fixture symbol, with why.
/// Documentation for the reader and the report; scoring needs no list of
/// negatives, because any edge outside the gold is a false positive.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trap {
    /// The symbol containing the call site.
    #[serde(rename = "in")]
    pub within: String,
    /// The name called at the site.
    pub name: String,
    pub line: u32,
    pub why: String,
}

/// One language's gold file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageGold {
    pub language: String,
    /// Fixture directory relative to `tests/fixtures/`.
    pub fixture: String,
    pub labelled_by: String,
    pub reviewed_by: Option<String>,
    /// How the labels were cross-checked beyond reading the code.
    pub verified_with: String,
    pub symbols: Vec<GoldSymbol>,
    pub calls: Vec<GoldCall>,
    #[serde(default)]
    pub dynamic: Vec<DynamicCall>,
    #[serde(default)]
    pub traps: Vec<Trap>,
}

impl LanguageGold {
    /// Parse and check internal consistency: unique symbol ids, every edge
    /// endpoint a declared symbol, no duplicate edges.
    pub fn parse(json: &str) -> Result<Self> {
        let gold: LanguageGold = serde_json::from_str(json).context("parsing gold file")?;
        gold.validate()?;
        Ok(gold)
    }

    fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for symbol in &self.symbols {
            if !symbol.id.contains("::") {
                bail!(
                    "{}: symbol id '{}' lacks `file::`",
                    self.language,
                    symbol.id
                );
            }
            if !ids.insert(symbol.id.as_str()) {
                bail!("{}: duplicate symbol id '{}'", self.language, symbol.id);
            }
        }
        let known = |id: &str, role: &str| -> Result<()> {
            if ids.contains(id) {
                Ok(())
            } else {
                bail!("{}: {role} '{id}' is not a gold symbol", self.language)
            }
        };
        let mut edges = BTreeSet::new();
        for call in &self.calls {
            known(&call.from, "caller")?;
            for target in call.targets() {
                known(target, "callee")?;
                if !edges.insert((call.from.as_str(), target)) {
                    bail!(
                        "{}: edge {} -> {target} is listed twice",
                        self.language,
                        call.from
                    );
                }
            }
            if call.why.trim().is_empty() {
                bail!(
                    "{}: edge {} -> {} has no rationale",
                    self.language,
                    call.from,
                    call.to
                );
            }
        }
        for dynamic in &self.dynamic {
            known(&dynamic.from, "dynamic caller")?;
            known(&dynamic.to, "dynamic callee")?;
        }
        for trap in &self.traps {
            known(&trap.within, "trap site")?;
        }
        Ok(())
    }

    /// Symbols keyed by id.
    pub fn symbol_map(&self) -> BTreeMap<&str, &GoldSymbol> {
        self.symbols.iter().map(|s| (s.id.as_str(), s)).collect()
    }
}

/// The whole gold set.
#[derive(Debug, Clone)]
pub struct GoldSet {
    pub languages: Vec<LanguageGold>,
}

impl GoldSet {
    /// The gold set compiled into this binary.
    pub fn embedded() -> Result<Self> {
        let languages = EMBEDDED
            .iter()
            .map(|(language, json)| {
                let gold = LanguageGold::parse(json)
                    .with_context(|| format!("graph-gold/{language}.json"))?;
                if gold.language != *language {
                    bail!(
                        "graph-gold/{language}.json declares language '{}'",
                        gold.language
                    );
                }
                Ok(gold)
            })
            .collect::<Result<_>>()?;
        Ok(Self { languages })
    }

    /// Content digest of the labels and of the fixture sources they describe.
    ///
    /// A baseline is only comparable with a run over the same labels *and* the
    /// same code: editing a fixture without relabelling would silently move
    /// every number, so the fixture bytes are part of the address.
    pub fn digest(&self, fixtures_root: &Path) -> Result<String> {
        let mut hasher = Sha256::new();
        for (language, json) in EMBEDDED {
            feed(&mut hasher, language.as_bytes());
            feed(&mut hasher, json.as_bytes());
        }
        for (relative, path) in fixture_files(&fixtures_root.join(FIXTURE_DIR))? {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            feed(&mut hasher, relative.as_bytes());
            feed(&mut hasher, &bytes);
        }
        Ok(format!("{:x}", hasher.finalize()))
    }
}

/// Length-prefixed, so `ab`+`c` and `a`+`bc` hash differently.
fn feed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Every file under `dir`, as `(forward-slashed relative path, path)`, sorted.
pub fn fixture_files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    if !dir.is_dir() {
        bail!("gold fixture directory missing: {}", dir.display());
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(dir)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, entry.into_path()));
        }
    }
    Ok(files)
}

/// `(file, qualified)` halves of a gold id.
fn split_id(id: &str) -> (&str, &str) {
    id.split_once("::").unwrap_or(("", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gold(json: &str) -> Result<LanguageGold> {
        LanguageGold::parse(json)
    }

    const MINIMAL: &str = r#"{
        "language": "python", "fixture": "graph-accuracy/python",
        "labelled_by": "agent", "reviewed_by": null, "verified_with": "reading",
        "symbols": [
            {"id": "a.py::f", "kind": "function", "line": 1},
            {"id": "a.py::C.g", "kind": "method", "line": 3}
        ],
        "calls": [{"from": "a.py::f", "to": "a.py::C.g", "line": 2, "why": "direct"}]
    }"#;

    #[test]
    fn ids_split_into_file_and_short_name() {
        let gold = gold(MINIMAL).unwrap();
        assert_eq!(gold.symbols[1].file(), "a.py");
        assert_eq!(gold.symbols[1].name(), "g");
        assert_eq!(gold.symbols[0].name(), "f");
        let overload = GoldSymbol {
            id: "R.java::Repository.add(T, int)".into(),
            kind: "method".into(),
            line: 1,
        };
        assert_eq!(overload.name(), "add");
    }

    #[test]
    fn an_edge_to_an_undeclared_symbol_is_refused() {
        let broken = MINIMAL.replace(r#""to": "a.py::C.g""#, r#""to": "a.py::nope""#);
        let error = gold(&broken).unwrap_err().to_string();
        assert!(error.contains("a.py::nope"), "{error}");
    }

    #[test]
    fn an_edge_without_a_rationale_is_refused() {
        let broken = MINIMAL.replace(r#""why": "direct""#, r#""why": " ""#);
        assert!(gold(&broken).is_err());
    }

    #[test]
    fn the_embedded_gold_set_is_consistent() {
        let set = GoldSet::embedded().unwrap();
        assert_eq!(set.languages.len(), EMBEDDED.len());
        for language in &set.languages {
            assert_eq!(language.labelled_by, "agent", "{}", language.language);
            assert!(
                language.fixture.starts_with(FIXTURE_DIR),
                "{}",
                language.fixture
            );
            assert!(
                (40..=80).contains(&language.calls.len()),
                "{}: {} gold edges, the set aims for 40-80",
                language.language,
                language.calls.len()
            );
        }
    }
}
