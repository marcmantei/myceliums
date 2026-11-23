//! The doc-mention sets (#180): mentions drawn from the spring-framework
//! reference docs and labelled with the declaration each denotes, by
//! reading the docs and the code (`graph-gold/held-out/doc-mentions-*.json`).
//!
//! - `doc-mentions-spring-heldout.json` (held out): 50 mentions, drawn by
//!   `sample_doc_mentions.py --heldout` (seed 4180) — 30 code spans, 10
//!   `xref:` macros, 10 Javadoc links. The numbers to quote.
//! - `doc-mentions-spring-dev.json` (dev): 25 mentions (seed 180), read
//!   while the matching rules were tuned.
//!
//! Reported next to the fixture gate, never gated: the sets are small and an
//! agent labelled them.
//!
//! # Scoring
//!
//! Per item. Its *predictions* are the targets of the doc MENTIONS edges
//! whose source is in the item's file and that record a match on the item's
//! line for the item's text: a code span with the same text; for an
//! `api-link` item, the API link to the same class page; for an `xref`
//! item, a code span inside its label. A prediction is correct when every
//! predicted symbol is one of the item's targets (same file, the qualified
//! name the label gives or its last segments).
//!
//! - precision: items with a correct prediction / items with a prediction;
//! - recall: items with a target and a correct prediction / items with a
//!   target.

use anyhow::{Context, Result};
use myceliums_core::doc_mentions::METHOD;
use myceliums_core::mentions::MentionMetadata;
use myceliums_storage::{CodeSymbol, SymbolKind};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{Extraction, ExtractionCache};
use super::held_out::pinned_checkout;
use super::report::wilson;

const HELD_OUT: &str = include_str!("../../graph-gold/held-out/doc-mentions-spring-heldout.json");
const DEV: &str = include_str!("../../graph-gold/held-out/doc-mentions-spring-dev.json");

#[derive(Debug, Clone, Deserialize)]
pub struct DocMentionSet {
    #[serde(skip)]
    pub name: String,
    pub description: String,
    pub role: String,
    pub labelled_by: String,
    pub reviewed_by: Option<String>,
    pub repos: BTreeMap<String, String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub id: String,
    pub stratum: String,
    pub file: String,
    pub line: u32,
    pub text: String,
    pub targets: Vec<Target>,
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Target {
    pub file: String,
    pub symbol: String,
}

impl DocMentionSet {
    /// Both sets, held out first.
    pub fn all() -> Result<Vec<Self>> {
        Ok(vec![
            Self::parse(HELD_OUT, "doc-mentions-spring-heldout.json")?,
            Self::parse(DEV, "doc-mentions-spring-dev.json")?,
        ])
    }

    fn parse(json: &str, name: &str) -> Result<Self> {
        let mut set: Self =
            serde_json::from_str(json).with_context(|| format!("parsing {name}"))?;
        set.name = name.to_string();
        Ok(set)
    }

    fn is_dev(&self) -> bool {
        self.role == "dev"
    }
}

/// One item's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scored {
    pub id: String,
    pub stratum: String,
    pub text: String,
    /// The item has a target.
    pub labelled: bool,
    /// Qualified names myceliums linked, sorted.
    pub predicted: Vec<String>,
    /// Every predicted symbol is a target.
    pub correct: bool,
}

/// Counts over a group of items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub items: usize,
    pub labelled: usize,
    pub predicted: usize,
    pub correct: usize,
}

impl Counts {
    fn add(&mut self, s: &Scored) {
        self.items += 1;
        self.labelled += usize::from(s.labelled);
        self.predicted += usize::from(!s.predicted.is_empty());
        self.correct += usize::from(s.correct);
    }
}

/// What a run of one set found.
#[derive(Debug, Default)]
pub struct DocMentionReport {
    pub scored: Vec<Scored>,
    pub skipped: Vec<String>,
    /// Repository-wide figures: edges by source, matches by syntax.
    pub totals: Vec<String>,
}

/// Score `set` against the pinned checkouts under `repos_dir`.
pub async fn run(
    set: &DocMentionSet,
    repos_dir: &Path,
    cache: &mut ExtractionCache,
) -> Result<DocMentionReport> {
    let mut report = DocMentionReport::default();
    for (repo, pin) in &set.repos {
        let dir = match pinned_checkout(repos_dir, repo, pin) {
            Ok(dir) => dir,
            Err(reason) => {
                report.skipped.push(reason);
                continue;
            }
        };
        let extraction = cache.get(&dir).await?;
        report.scored.extend(score(&set.items, extraction));
        report.totals.push(totals(repo, extraction));
    }
    Ok(report)
}

/// A recorded match: how it was written, as what, and the symbol it links.
struct Match<'a> {
    syntax: String,
    text: String,
    target: &'a CodeSymbol,
}

/// Score every item against the doc MENTIONS edges of `extraction`.
pub fn score(items: &[Item], extraction: &Extraction) -> Vec<Scored> {
    let by_uid: HashMap<&str, &CodeSymbol> = extraction
        .symbols
        .iter()
        .map(|s| (s.uid.as_str(), s))
        .collect();
    let mut at: HashMap<(&str, u32), Vec<Match<'_>>> = HashMap::new();
    for rel in &extraction.mentions {
        let Ok(meta) = serde_json::from_str::<MentionMetadata>(&rel.metadata) else {
            continue;
        };
        if meta.extraction_method != METHOD {
            continue;
        }
        let (Some(source), Some(target)) = (
            by_uid.get(rel.source_uid.as_str()),
            by_uid.get(rel.target_uid.as_str()),
        ) else {
            continue;
        };
        for m in meta.matches {
            at.entry((source.file_path.as_str(), m.line))
                .or_default()
                .push(Match {
                    syntax: m.syntax.unwrap_or_default(),
                    text: m.text.unwrap_or_default(),
                    target,
                });
        }
    }
    items
        .iter()
        .map(|item| {
            let predicted: Vec<&CodeSymbol> = at
                .get(&(item.file.as_str(), item.line))
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|m| belongs_to(item, m))
                .map(|m| m.target)
                .collect();
            let correct = !predicted.is_empty()
                && predicted
                    .iter()
                    .all(|p| item.targets.iter().any(|t| is_target(p, t)));
            let names: BTreeSet<String> = predicted
                .iter()
                .map(|p| format!("{} ({})", p.qualified_name, p.file_path))
                .collect();
            Scored {
                id: item.id.clone(),
                stratum: item.stratum.clone(),
                text: item.text.clone(),
                labelled: !item.targets.is_empty(),
                predicted: names.into_iter().collect(),
                correct,
            }
        })
        .collect()
}

/// Whether a match on the item's line is the item's mention.
fn belongs_to(item: &Item, m: &Match<'_>) -> bool {
    match item.stratum.as_str() {
        "code-span" => m.syntax == "code-span" && m.text == item.text.trim(),
        "api-link" => {
            let class = m.text.split('#').next().unwrap_or("");
            m.syntax == "api-link"
                && (item
                    .text
                    .contains(&format!("{}.html", class.replace('.', "/")))
                    || item.text.contains(&m.text))
        }
        "xref" => m.syntax == "code-span" && item.text.contains(&format!("`{}`", m.text)),
        _ => false,
    }
}

/// Whether `symbol` is the labelled `target`: same file, and the label's
/// name ends with the symbol's qualified name (myceliums qualifies by the
/// innermost enclosing type, a label by all of them).
fn is_target(symbol: &CodeSymbol, target: &Target) -> bool {
    symbol.file_path == target.file
        && (target.symbol == symbol.qualified_name
            || target
                .symbol
                .ends_with(&format!(".{}", symbol.qualified_name)))
}

/// Repository-wide figures of the doc mentions.
fn totals(repo: &str, extraction: &Extraction) -> String {
    let kinds: HashMap<&str, &SymbolKind> = extraction
        .symbols
        .iter()
        .map(|s| (s.uid.as_str(), &s.kind))
        .collect();
    let (mut from_docs, mut from_comments) = (0usize, 0usize);
    let mut by_syntax: BTreeMap<String, usize> = BTreeMap::new();
    for rel in &extraction.mentions {
        let Ok(meta) = serde_json::from_str::<MentionMetadata>(&rel.metadata) else {
            continue;
        };
        if meta.extraction_method != METHOD {
            continue;
        }
        match kinds.get(rel.source_uid.as_str()) {
            Some(SymbolKind::Section | SymbolKind::Document) => from_docs += 1,
            _ => from_comments += 1,
        }
        for m in meta.matches {
            *by_syntax.entry(m.syntax.unwrap_or_default()).or_default() += 1;
        }
    }
    let syntax = by_syntax
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let stats = extraction
        .doc_stats
        .as_ref()
        .map(|s| {
            format!(
                "; mentions linked {}, ambiguous {} ({} names), unresolved {}",
                s.linked,
                s.ambiguous,
                s.ambiguous_by_name.len(),
                s.unresolved
            )
        })
        .unwrap_or_default();
    format!(
        "{repo}: {} doc MENTIONS edges ({from_docs} from doc sections, {from_comments} from doc comments); \
         matches by syntax: {syntax}{stats}; analysis {:.1} s",
        from_docs + from_comments,
        extraction.analysis_secs
    )
}

fn rate(part: usize, whole: usize) -> String {
    match wilson(part, whole) {
        Some((low, high)) => format!(
            "{part}/{whole} = {:.2} ({low:.2}–{high:.2})",
            part as f64 / whole as f64
        ),
        None => "n/a".to_string(),
    }
}

/// The report as Markdown.
pub fn markdown(set: &DocMentionSet, report: &DocMentionReport) -> String {
    let mut out = String::new();
    let role = if set.is_dev() {
        "dev set, not held out: the matching rules were tuned on it"
    } else {
        "held out"
    };
    let _ = writeln!(
        out,
        "\n### Doc mentions: `{}` ({role}; report only, not gated)\n",
        set.name
    );
    let _ = writeln!(
        out,
        "{} mentions drawn from the reference docs and labelled by reading the docs and the code \
         (labelled by `{}`, reviewed by `{}`).\n",
        set.items.len(),
        set.labelled_by,
        set.reviewed_by.as_deref().unwrap_or("nobody yet")
    );
    for reason in &report.skipped {
        let _ = writeln!(out, "- skipped: {reason}");
    }
    if report.scored.is_empty() {
        return out;
    }
    let mut groups: BTreeMap<String, Counts> = BTreeMap::new();
    let mut total = Counts::default();
    for s in &report.scored {
        groups.entry(s.stratum.clone()).or_default().add(s);
        total.add(s);
    }
    let _ = writeln!(
        out,
        "| stratum | items | with a target | linked | precision | recall |\n|---|---:|---:|---:|---|---|"
    );
    for (name, c) in groups
        .iter()
        .chain(std::iter::once((&"all".to_string(), &total)))
    {
        let recall = report
            .scored
            .iter()
            .filter(|s| (name == "all" || &s.stratum == name) && s.labelled && s.correct)
            .count();
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} | {} | {} |",
            c.items,
            c.labelled,
            c.predicted,
            rate(c.correct, c.predicted),
            rate(recall, c.labelled)
        );
    }
    let wrong: Vec<&Scored> = report
        .scored
        .iter()
        .filter(|s| !s.predicted.is_empty() && !s.correct)
        .collect();
    let missed: Vec<&Scored> = report
        .scored
        .iter()
        .filter(|s| s.labelled && s.predicted.is_empty())
        .collect();
    let _ = writeln!(out, "\nWrong links ({}):", wrong.len());
    for s in wrong {
        let _ = writeln!(out, "- {} `{}` → {}", s.id, s.text, s.predicted.join(", "));
    }
    let _ = writeln!(out, "\nNot linked ({}):", missed.len());
    for s in missed {
        let _ = writeln!(out, "- {} `{}`", s.id, s.text);
    }
    for line in &report.totals {
        let _ = writeln!(out, "\n{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_core::mentions::MentionMatch;
    use myceliums_storage::{Relationship, RelationshipKind};

    #[test]
    fn the_sets_parse_and_are_disjoint() {
        let sets = DocMentionSet::all().unwrap();
        assert_eq!(sets[0].items.len(), 50);
        assert_eq!(sets[1].items.len(), 25);
        for set in &sets {
            assert_eq!(set.labelled_by, "agent");
        }
        let held: BTreeSet<(&str, u32, &str)> = sets[0]
            .items
            .iter()
            .map(|i| (i.file.as_str(), i.line, i.text.as_str()))
            .collect();
        assert!(sets[1].items.iter().all(|i| !held.contains(&(
            i.file.as_str(),
            i.line,
            i.text.as_str()
        ))));
    }

    fn symbol(uid: &str, file: &str, qualified: &str, kind: SymbolKind) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: qualified.rsplit('.').next().unwrap().to_string(),
            qualified_name: qualified.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 100,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    fn mention(source: &str, target: &str, line: u32, syntax: &str, text: &str) -> Relationship {
        let meta = MentionMetadata {
            matches: vec![MentionMatch {
                line,
                context: String::new(),
                method: METHOD.to_string(),
                text: Some(text.to_string()),
                syntax: Some(syntax.to_string()),
            }],
            count: 1,
            extraction_method: METHOD.to_string(),
        };
        Relationship {
            uid: format!("{source}-{target}"),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Mentions,
            repo_id: "r".to_string(),
            metadata: serde_json::to_string(&meta).unwrap(),
        }
    }

    fn item(id: &str, stratum: &str, line: u32, text: &str, targets: &[(&str, &str)]) -> Item {
        Item {
            id: id.to_string(),
            stratum: stratum.to_string(),
            file: "docs/a.adoc".to_string(),
            line,
            text: text.to_string(),
            targets: targets
                .iter()
                .map(|(file, symbol)| Target {
                    file: file.to_string(),
                    symbol: symbol.to_string(),
                })
                .collect(),
            why: String::new(),
        }
    }

    #[test]
    fn items_are_scored_by_line_text_and_target() {
        let extraction = Extraction {
            symbols: vec![
                symbol("sec", "docs/a.adoc", "Intro", SymbolKind::Section),
                symbol(
                    "b",
                    "src/WebClient.java",
                    "WebClient.Builder",
                    SymbolKind::Interface,
                ),
                symbol("x", "src/Other.java", "Builder", SymbolKind::Interface),
                symbol("bean", "src/Bean.java", "Bean", SymbolKind::Interface),
            ],
            mentions: vec![
                mention("sec", "b", 3, "code-span", "WebClient.Builder"),
                mention("sec", "x", 4, "code-span", "Builder"),
                mention("sec", "bean", 5, "api-link", "context.annotation.Bean"),
                mention("sec", "bean", 6, "code-span", "@Bean"),
            ],
            ..Default::default()
        };
        let items = vec![
            item(
                "1",
                "code-span",
                3,
                "WebClient.Builder",
                &[("src/WebClient.java", "WebClient.Builder")],
            ),
            item("2", "code-span", 4, "Builder", &[]),
            item(
                "3",
                "api-link",
                5,
                "{spring-framework-api}/context/annotation/Bean.html[`@Bean`]",
                &[("src/Bean.java", "Bean")],
            ),
            item(
                "4",
                "xref",
                6,
                "xref:core/beans/java/bean-annotation.adoc[Using the `@Bean` Annotation]",
                &[("src/Bean.java", "Bean")],
            ),
            item(
                "5",
                "code-span",
                7,
                "Session",
                &[("src/Session.java", "Session")],
            ),
        ];
        let scored = score(&items, &extraction);
        let outcome: Vec<(bool, bool)> = scored
            .iter()
            .map(|s| (!s.predicted.is_empty(), s.correct))
            .collect();
        assert_eq!(
            outcome,
            vec![
                (true, true),
                (true, false),
                (true, true),
                (true, true),
                (false, false)
            ]
        );
    }
}
