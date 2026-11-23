//! Scoring an extraction against the gold set.
//!
//! # Symbols
//!
//! A gold symbol is found when the extraction has a symbol in the same file,
//! with the same short name, whose line span contains the gold line (the line
//! of the name in the definition). The span rule tolerates decorators,
//! attributes and doc comments that an extractor may or may not include. When
//! several extracted symbols qualify, the tightest span wins; each extracted
//! symbol is used at most once. Kinds are not part of the match, so a struct
//! reported as a class still counts; disagreements are listed separately.
//!
//! An unmatched extracted symbol is a false positive only when its kind is a
//! definition the gold set covers ([`DEFINITION_KINDS`]). Variables,
//! constants, fields, modules, imports and comments are out of the gold's
//! scope and are neither rewarded nor penalised.
//!
//! # Calls
//!
//! A CALLS edge is a true positive when both endpoints map to gold symbols and
//! the pair is a gold edge (its primary target or one of its `also_accept`
//! targets). Each gold edge is credited once; a second extracted edge that
//! satisfies an already-credited gold edge (say, to both a class and its
//! constructor) is *redundant* and a call the gold lists as runtime-only is
//! *dynamic*: both are excluded from precision. Every other extracted edge is
//! a false positive — including one whose endpoint is not a gold symbol at
//! all, since an edge to the wrong node is wrong.
//!
//! Per resolution label (`exact`, `import`, `name-match`, `ambiguous`):
//! precision is the share of that label's scored edges that are correct, and
//! `recall_share` is the share of all gold edges found *with* that label — the
//! shares add up to the overall recall. A label has no recall of its own: the
//! gold does not say how an edge ought to be resolved.

use myceliums_core::dispatch::DispatchIndex;
use myceliums_core::CallResolution;
use myceliums_storage::{CodeSymbol, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::extract::Extraction;
use super::gold::LanguageGold;

/// Extracted kinds that count as definitions: unmatched ones are false
/// positives.
pub const DEFINITION_KINDS: [SymbolKind; 6] = [
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Class,
    SymbolKind::Interface,
    SymbolKind::Enum,
    SymbolKind::TypeAlias,
];

/// Extracted kinds that are not code definitions at all and never match.
const NON_CODE_KINDS: [SymbolKind; 8] = [
    SymbolKind::Import,
    SymbolKind::Rationale,
    SymbolKind::Section,
    SymbolKind::Document,
    SymbolKind::Email,
    SymbolKind::Conversation,
    SymbolKind::Person,
    SymbolKind::Attachment,
];

/// Label given to an edge whose metadata carries no resolution.
pub const UNLABELLED: &str = "unlabelled";

/// Precision, recall and F1 from raw counts. `None` where undefined.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub gold: usize,
    /// Scored predictions: true plus false positives.
    pub predicted: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
}

impl Counts {
    pub fn precision(&self) -> Option<f64> {
        ratio(self.true_positives, self.predicted)
    }

    pub fn recall(&self) -> Option<f64> {
        ratio(self.true_positives, self.gold)
    }

    pub fn f1(&self) -> Option<f64> {
        let (p, r) = (self.precision()?, self.recall()?);
        Some(if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        })
    }

    pub fn add(&mut self, other: &Counts) {
        self.gold += other.gold;
        self.predicted += other.predicted;
        self.true_positives += other.true_positives;
        self.false_positives += other.false_positives;
        self.false_negatives += other.false_negatives;
    }
}

/// `numerator / denominator`, or `None` for an empty denominator.
pub fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

/// Symbol extraction against the gold symbols.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SymbolScore {
    pub counts: Counts,
    /// Extracted definitions with no gold counterpart: `file::qualified (Kind, line)`.
    pub false_positives: Vec<String>,
    /// Gold symbol ids nothing was extracted for.
    pub false_negatives: Vec<String>,
    /// Matched symbols whose extracted kind disagrees with the gold kind.
    pub kind_mismatches: Vec<String>,
}

/// Edges of one resolution label.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelCounts {
    /// Scored edges with this label (true plus false positives).
    pub predicted: usize,
    pub true_positives: usize,
    pub false_positives: usize,
}

impl LabelCounts {
    fn add(&mut self, other: &LabelCounts) {
        self.predicted += other.predicted;
        self.true_positives += other.true_positives;
        self.false_positives += other.false_positives;
    }
}

/// Gold edges carrying one tag, and how many were found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TagCounts {
    pub gold: usize,
    pub found: usize,
}

/// One extracted edge that is not in the gold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrongEdge {
    pub from: String,
    pub to: String,
    pub label: String,
    pub line: Option<u32>,
    /// Why it is wrong, when the gold says (a documented trap), or what is
    /// wrong with it (an endpoint that is no gold symbol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One gold edge that was not extracted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissedEdge {
    pub from: String,
    pub to: String,
    pub line: u32,
    pub tags: Vec<String>,
    pub why: String,
}

/// CALLS edges against the gold edges.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallScore {
    pub counts: Counts,
    /// Correct edges duplicating an already-credited gold edge; not scored.
    pub redundant: usize,
    /// Edges matching a runtime-only call; not scored.
    pub dynamic: usize,
    /// False positives whose call site lies outside the caller's body: the
    /// call was attributed to another symbol of the same name. A lower bound:
    /// edge metadata has the call line but not its file, so a misattributed
    /// call whose line happens to fall inside the wrong caller's span is not
    /// counted.
    pub wrong_caller: usize,
    pub by_label: BTreeMap<String, LabelCounts>,
    pub by_tag: BTreeMap<String, TagCounts>,
    pub false_positives: Vec<WrongEdge>,
    pub false_negatives: Vec<MissedEdge>,
}

/// Targets dynamic dispatch adds to the CALLS edges (#190): for each CALLS
/// edge a dispatch may follow, the implementations of its target that the
/// caller has no CALLS edge to. Correct when the gold lists the caller and
/// the implementation as an edge (an overriding implementation is among the
/// `also_accept` targets of a dispatch call). Not part of the CALLS scores.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DispatchScore {
    pub targets: usize,
    pub correct: usize,
    /// Calls the gold lists as runtime-only; not scored.
    #[serde(default)]
    pub dynamic: usize,
    pub wrong: Vec<String>,
}

/// Everything measured for one language.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LanguageScore {
    pub symbols: SymbolScore,
    pub calls: CallScore,
    #[serde(default)]
    pub dispatch: DispatchScore,
}

/// Sums over languages: counts only, no lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub symbols: Counts,
    pub calls: Counts,
    pub wrong_caller: usize,
    pub by_label: BTreeMap<String, LabelCounts>,
    pub by_tag: BTreeMap<String, TagCounts>,
}

impl Totals {
    pub fn of<'a>(scores: impl IntoIterator<Item = &'a LanguageScore>) -> Self {
        let mut totals = Totals::default();
        for score in scores {
            totals.symbols.add(&score.symbols.counts);
            totals.calls.add(&score.calls.counts);
            totals.wrong_caller += score.calls.wrong_caller;
            for (label, counts) in &score.calls.by_label {
                totals
                    .by_label
                    .entry(label.clone())
                    .or_default()
                    .add(counts);
            }
            for (tag, counts) in &score.calls.by_tag {
                let entry = totals.by_tag.entry(tag.clone()).or_default();
                entry.gold += counts.gold;
                entry.found += counts.found;
            }
        }
        totals
    }
}

/// Score `extraction` against `gold`.
///
/// `prefix` is the path of the language's fixture inside the analyzed tree:
/// empty when the fixture itself was analyzed, `python/` when the whole
/// gold fixture directory was analyzed as one repository. Symbols outside the
/// prefix belong to other languages; an edge from this language into them is
/// a false positive.
pub fn score_language(gold: &LanguageGold, extraction: &Extraction, prefix: &str) -> LanguageScore {
    let by_uid: HashMap<&str, &CodeSymbol> = extraction
        .symbols
        .iter()
        .map(|s| (s.uid.as_str(), s))
        .collect();
    let mine = |symbol: &CodeSymbol| symbol.file_path.starts_with(prefix);
    let local_path = |symbol: &CodeSymbol| symbol.file_path[prefix.len()..].to_string();

    // --- symbols -------------------------------------------------------------
    let candidates: Vec<&CodeSymbol> = extraction
        .symbols
        .iter()
        .filter(|s| mine(s) && !NON_CODE_KINDS.contains(&s.kind))
        .collect();
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    let mut gold_of_uid: HashMap<&str, &str> = HashMap::new();
    let mut symbols = SymbolScore::default();
    for wanted in &gold.symbols {
        let best = candidates
            .iter()
            .filter(|c| {
                !taken.contains(c.uid.as_str())
                    && local_path(c) == wanted.file()
                    && c.name == wanted.name()
                    && c.start_line <= wanted.line
                    && wanted.line <= c.end_line
            })
            .min_by_key(|c| {
                (
                    c.end_line - c.start_line,
                    !DEFINITION_KINDS.contains(&c.kind),
                    c.uid.clone(),
                )
            });
        match best {
            Some(found) => {
                taken.insert(found.uid.as_str());
                gold_of_uid.insert(found.uid.as_str(), wanted.id.as_str());
                if !kind_agrees(&wanted.kind, &found.kind) {
                    symbols.kind_mismatches.push(format!(
                        "{}: gold {}, extracted {}",
                        wanted.id, wanted.kind, found.kind
                    ));
                }
            }
            None => symbols.false_negatives.push(wanted.id.clone()),
        }
    }
    let mut unmatched: Vec<&&CodeSymbol> = candidates
        .iter()
        .filter(|c| !taken.contains(c.uid.as_str()) && DEFINITION_KINDS.contains(&c.kind))
        .collect();
    unmatched.sort_by_key(|c| (local_path(c), c.start_line, c.qualified_name.clone()));
    symbols.false_positives = unmatched
        .iter()
        .map(|c| {
            format!(
                "{}::{} ({}, line {})",
                local_path(c),
                c.qualified_name,
                c.kind,
                c.start_line
            )
        })
        .collect();
    symbols.counts = Counts {
        gold: gold.symbols.len(),
        predicted: taken.len() + unmatched.len(),
        true_positives: taken.len(),
        false_positives: unmatched.len(),
        false_negatives: symbols.false_negatives.len(),
    };

    // --- calls ---------------------------------------------------------------
    let display = |uid: &str| -> String {
        if let Some(id) = gold_of_uid.get(uid) {
            return (*id).to_string();
        }
        match by_uid.get(uid) {
            Some(s) if mine(s) => format!("{}::{} [not gold]", local_path(s), s.qualified_name),
            Some(s) => format!("{}::{} [other fixture]", s.file_path, s.qualified_name),
            None => format!("<unknown symbol {uid}>"),
        }
    };

    struct Edge<'a> {
        from: Option<&'a str>,
        to: Option<&'a str>,
        from_display: String,
        to_display: String,
        to_name: Option<&'a str>,
        caller_span: Option<(u32, u32)>,
        label: String,
        line: Option<u32>,
    }
    let mut seen = BTreeSet::new();
    let mut edges: Vec<Edge> = extraction
        .calls
        .iter()
        .filter(|r| match by_uid.get(r.source_uid.as_str()) {
            Some(source) => mine(source),
            None => prefix.is_empty(),
        })
        .filter(|r| seen.insert((r.source_uid.as_str(), r.target_uid.as_str())))
        .map(|r| Edge {
            from: gold_of_uid.get(r.source_uid.as_str()).copied(),
            to: gold_of_uid.get(r.target_uid.as_str()).copied(),
            from_display: display(&r.source_uid),
            to_display: display(&r.target_uid),
            to_name: by_uid.get(r.target_uid.as_str()).map(|s| s.name.as_str()),
            caller_span: by_uid
                .get(r.source_uid.as_str())
                .map(|s| (s.start_line, s.end_line)),
            label: CallResolution::from_metadata(&r.metadata)
                .map(|res| res.label().to_string())
                .unwrap_or_else(|| UNLABELLED.to_string()),
            line: call_line(&r.metadata),
        })
        .collect();
    edges.sort_by(|a, b| {
        (&a.from_display, &a.to_display, a.line).cmp(&(&b.from_display, &b.to_display, b.line))
    });

    let mut primary: HashMap<(&str, &str), usize> = HashMap::new();
    let mut alternate: HashMap<(&str, &str), usize> = HashMap::new();
    for (index, call) in gold.calls.iter().enumerate() {
        primary.insert((call.from.as_str(), call.to.as_str()), index);
        for target in &call.also_accept {
            alternate.insert((call.from.as_str(), target.as_str()), index);
        }
    }
    let dynamic: BTreeSet<(&str, &str)> = gold
        .dynamic
        .iter()
        .map(|d| (d.from.as_str(), d.to.as_str()))
        .collect();

    let mut credited = vec![false; gold.calls.len()];
    let mut calls = CallScore::default();
    let mut verdicts: Vec<Option<bool>> = vec![None; edges.len()]; // Some(true)=TP, Some(false)=FP
                                                                   // Pass 1: primary targets. Pass 2: alternates, dynamic calls, the rest.
    for (i, edge) in edges.iter().enumerate() {
        if let (Some(from), Some(to)) = (edge.from, edge.to) {
            if let Some(&g) = primary.get(&(from, to)) {
                if !credited[g] {
                    credited[g] = true;
                    verdicts[i] = Some(true);
                }
            }
        }
    }
    for (i, edge) in edges.iter().enumerate() {
        if verdicts[i].is_some() {
            continue;
        }
        let pair = edge.from.zip(edge.to);
        let gold_edge = pair.and_then(|p| primary.get(&p).or_else(|| alternate.get(&p)));
        match gold_edge {
            Some(&g) if !credited[g] => {
                credited[g] = true;
                verdicts[i] = Some(true);
            }
            Some(_) => calls.redundant += 1,
            None if pair.is_some_and(|p| dynamic.contains(&p)) => calls.dynamic += 1,
            None => verdicts[i] = Some(false),
        }
    }

    for (edge, verdict) in edges.iter().zip(&verdicts) {
        let Some(correct) = *verdict else { continue };
        let label = calls.by_label.entry(edge.label.clone()).or_default();
        label.predicted += 1;
        if correct {
            label.true_positives += 1;
        } else {
            label.false_positives += 1;
            let misattributed = match (edge.line, edge.caller_span) {
                (Some(line), Some((start, end))) => line < start || line > end,
                _ => false,
            };
            calls.wrong_caller += usize::from(misattributed);
            let note = if misattributed {
                Some(format!(
                    "wrong caller: the call site is outside {}'s lines {}-{}",
                    edge.from_display,
                    edge.caller_span.map_or(0, |s| s.0),
                    edge.caller_span.map_or(0, |s| s.1),
                ))
            } else {
                trap_note(gold, edge.from, edge.to_name).or_else(|| {
                    (edge.from.is_none() || edge.to.is_none())
                        .then(|| "endpoint is not a gold symbol".to_string())
                })
            };
            calls.false_positives.push(WrongEdge {
                from: edge.from_display.clone(),
                to: edge.to_display.clone(),
                label: edge.label.clone(),
                line: edge.line,
                note,
            });
        }
    }

    for (call, found) in gold.calls.iter().zip(&credited) {
        for tag in &call.tags {
            let entry = calls.by_tag.entry(tag.clone()).or_default();
            entry.gold += 1;
            entry.found += usize::from(*found);
        }
        if !found {
            calls.false_negatives.push(MissedEdge {
                from: call.from.clone(),
                to: call.to.clone(),
                line: call.line,
                tags: call.tags.clone(),
                why: call.why.clone(),
            });
        }
    }

    let true_positives = credited.iter().filter(|c| **c).count();
    calls.counts = Counts {
        gold: gold.calls.len(),
        predicted: true_positives + calls.false_positives.len(),
        true_positives,
        false_positives: calls.false_positives.len(),
        false_negatives: calls.false_negatives.len(),
    };

    // --- dispatch (#190) --------------------------------------------------------
    let index = DispatchIndex::new(&extraction.overrides);
    let direct: BTreeSet<(&str, &str)> = extraction
        .calls
        .iter()
        .map(|r| (r.source_uid.as_str(), r.target_uid.as_str()))
        .collect();
    let mut dispatched: BTreeSet<(&str, &str)> = BTreeSet::new();
    for call in extraction.calls.iter().filter(|r| {
        by_uid
            .get(r.source_uid.as_str())
            .is_some_and(|source| mine(source))
            && index.dispatches(r)
    }) {
        for ov in index.implementations(&call.target_uid) {
            if !direct.contains(&(call.source_uid.as_str(), ov.other)) {
                dispatched.insert((call.source_uid.as_str(), ov.other));
            }
        }
    }
    let mut dispatch = DispatchScore::default();
    for (source, target) in dispatched {
        let pair = gold_of_uid
            .get(source)
            .copied()
            .zip(gold_of_uid.get(target).copied());
        if pair.is_some_and(|p| dynamic.contains(&p)) {
            dispatch.dynamic += 1;
            continue;
        }
        dispatch.targets += 1;
        if pair.is_some_and(|p| primary.contains_key(&p) || alternate.contains_key(&p)) {
            dispatch.correct += 1;
        } else {
            dispatch
                .wrong
                .push(format!("{} → {}", display(source), display(target)));
        }
    }

    LanguageScore {
        symbols,
        calls,
        dispatch,
    }
}

/// The documented trap a wrong edge falls into, if any.
fn trap_note(gold: &LanguageGold, from: Option<&str>, to_name: Option<&str>) -> Option<String> {
    let (from, to_name) = (from?, to_name?);
    gold.traps
        .iter()
        .find(|t| t.within == from && t.name == to_name)
        .map(|t| format!("trap (line {}): {}", t.line, t.why))
}

/// The call-site line recorded in edge metadata (`line:<n>;...`).
fn call_line(metadata: &str) -> Option<u32> {
    metadata
        .split(';')
        .find_map(|part| part.strip_prefix("line:"))
        .and_then(|n| n.parse().ok())
}

/// Whether an extracted kind is what the labeller's kind means.
fn kind_agrees(gold_kind: &str, extracted: &SymbolKind) -> bool {
    match gold_kind {
        "function" => *extracted == SymbolKind::Function,
        "method" | "constructor" => *extracted == SymbolKind::Method,
        "class" | "struct" => *extracted == SymbolKind::Class,
        "interface" | "trait" => *extracted == SymbolKind::Interface,
        "enum" => *extracted == SymbolKind::Enum,
        "type" => *extracted == SymbolKind::TypeAlias,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_eval::gold::LanguageGold;
    use myceliums_storage::{Relationship, RelationshipKind};

    const GOLD: &str = r#"{
        "language": "python", "fixture": "graph-accuracy/python",
        "labelled_by": "agent", "reviewed_by": null, "verified_with": "reading",
        "symbols": [
            {"id": "a.py::main", "kind": "function", "line": 1},
            {"id": "a.py::parse", "kind": "function", "line": 5},
            {"id": "b.py::parse", "kind": "function", "line": 1},
            {"id": "b.py::Box", "kind": "class", "line": 3},
            {"id": "b.py::Box.__init__", "kind": "method", "line": 4},
            {"id": "b.py::run", "kind": "function", "line": 9}
        ],
        "calls": [
            {"from": "a.py::main", "to": "a.py::parse", "line": 2, "tags": ["same-name"], "why": "local"},
            {"from": "a.py::main", "to": "b.py::Box", "also_accept": ["b.py::Box.__init__"], "line": 3, "tags": ["constructor"], "why": "new"}
        ],
        "dynamic": [{"from": "b.py::run", "to": "a.py::main", "why": "callback"}],
        "traps": [{"in": "b.py::run", "name": "parse", "line": 10, "why": "str method"}]
    }"#;

    fn symbol(
        uid: &str,
        file: &str,
        name: &str,
        kind: SymbolKind,
        lines: (u32, u32),
    ) -> CodeSymbol {
        CodeSymbol {
            uid: uid.into(),
            name: name.into(),
            qualified_name: name.into(),
            kind,
            file_path: file.into(),
            start_line: lines.0,
            end_line: lines.1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    fn call(from: &str, to: &str, label: &str) -> Relationship {
        Relationship {
            uid: format!("{from}->{to}"),
            source_uid: from.into(),
            target_uid: to.into(),
            kind: RelationshipKind::Calls,
            repo_id: "r".into(),
            // A line inside the caller's body (see `extraction`).
            metadata: format!(
                "line:{};resolved:{label}",
                if from == "run" { 10 } else { 2 }
            ),
        }
    }

    fn extraction(prefix: &str, calls: Vec<Relationship>) -> Extraction {
        let p = |f: &str| format!("{prefix}{f}");
        Extraction {
            symbols: vec![
                symbol("main", &p("a.py"), "main", SymbolKind::Function, (1, 3)),
                symbol("a-parse", &p("a.py"), "parse", SymbolKind::Function, (5, 6)),
                symbol("b-parse", &p("b.py"), "parse", SymbolKind::Function, (1, 2)),
                symbol("box", &p("b.py"), "Box", SymbolKind::Class, (3, 7)),
                symbol("init", &p("b.py"), "__init__", SymbolKind::Method, (4, 5)),
                symbol("run", &p("b.py"), "run", SymbolKind::Function, (9, 11)),
                // Not in the gold: a definition (FP) and a variable (ignored).
                symbol(
                    "extra",
                    &p("b.py"),
                    "helper",
                    SymbolKind::Function,
                    (12, 13),
                ),
                symbol("var", &p("b.py"), "LIMIT", SymbolKind::Constant, (14, 14)),
            ],
            calls,
            ..Extraction::default()
        }
    }

    fn gold() -> LanguageGold {
        LanguageGold::parse(GOLD).unwrap()
    }

    #[test]
    fn symbols_match_by_file_name_and_line_span() {
        let score = score_language(&gold(), &extraction("", vec![]), "");
        let counts = score.symbols.counts;
        assert_eq!(counts.true_positives, 6);
        assert_eq!(
            counts.false_positives, 1,
            "{:?}",
            score.symbols.false_positives
        );
        assert_eq!(counts.false_negatives, 0);
        assert!(score.symbols.false_positives[0].contains("helper"));
    }

    #[test]
    fn a_same_name_symbol_in_another_file_does_not_satisfy_the_gold() {
        let mut extraction = extraction("", vec![]);
        extraction.symbols.retain(|s| s.uid != "a-parse");
        let score = score_language(&gold(), &extraction, "");
        assert_eq!(
            score.symbols.false_negatives,
            vec!["a.py::parse".to_string()]
        );
    }

    #[test]
    fn calls_are_scored_per_label_with_traps_and_neutral_edges() {
        let calls = vec![
            call("main", "a-parse", "ambiguous"), // TP
            call("main", "box", "name-match"),    // TP (primary)
            call("main", "init", "exact"),        // redundant (alternate of credited)
            call("run", "b-parse", "ambiguous"),  // FP, trap
            call("run", "main", "name-match"),    // dynamic
            call("main", "extra", "name-match"),  // FP, not gold
        ];
        let score = score_language(&gold(), &extraction("", calls), "");
        let c = &score.calls;
        assert_eq!(c.counts.true_positives, 2);
        assert_eq!(c.counts.false_positives, 2);
        assert_eq!(c.counts.false_negatives, 0);
        assert_eq!((c.redundant, c.dynamic), (1, 1));
        assert_eq!(c.counts.precision(), Some(0.5));
        assert_eq!(c.counts.recall(), Some(1.0));
        let ambiguous = c.by_label["ambiguous"];
        assert_eq!(
            (ambiguous.true_positives, ambiguous.false_positives),
            (1, 1)
        );
        assert!(
            !c.by_label.contains_key("exact"),
            "redundant edges are not scored"
        );
        let trap = c
            .false_positives
            .iter()
            .find(|e| e.from == "b.py::run")
            .unwrap();
        assert!(
            trap.note.as_deref().unwrap().starts_with("trap"),
            "{trap:?}"
        );
        let stray = c
            .false_positives
            .iter()
            .find(|e| e.from == "a.py::main")
            .unwrap();
        assert!(stray.to.contains("[not gold]"), "{stray:?}");
        assert_eq!(c.by_tag["constructor"], TagCounts { gold: 1, found: 1 });
    }

    #[test]
    fn an_alternate_target_alone_credits_the_edge() {
        let calls = vec![call("main", "init", "exact")];
        let score = score_language(&gold(), &extraction("", calls), "");
        assert_eq!(score.calls.counts.true_positives, 1);
        assert_eq!(score.calls.redundant, 0);
        assert_eq!(score.calls.false_negatives.len(), 1);
        assert_eq!(score.calls.false_negatives[0].to, "a.py::parse");
    }

    #[test]
    fn in_a_shared_tree_only_this_languages_symbols_and_edges_count() {
        let mut shared = extraction("python/", vec![call("main", "go-parse", "ambiguous")]);
        shared.symbols.push(symbol(
            "go-parse",
            "go/parse.go",
            "parse",
            SymbolKind::Function,
            (1, 3),
        ));
        let score = score_language(&gold(), &shared, "python/");
        assert_eq!(score.symbols.counts.true_positives, 6);
        assert_eq!(
            score.symbols.counts.false_positives, 1,
            "go symbol is not ours"
        );
        let wrong = &score.calls.false_positives[0];
        assert!(wrong.to.contains("[other fixture]"), "{wrong:?}");
    }

    #[test]
    fn a_call_site_outside_the_callers_body_is_a_wrong_caller() {
        // `run` spans lines 9-11; a call recorded at line 2 was made elsewhere
        // and attributed to `run` by name.
        let mut edge = call("run", "a-parse", "name-match");
        edge.metadata = "line:2;resolved:name-match".into();
        let score = score_language(&gold(), &extraction("", vec![edge]), "");
        assert_eq!(score.calls.wrong_caller, 1);
        let note = score.calls.false_positives[0].note.as_deref().unwrap();
        assert!(note.starts_with("wrong caller"), "{note}");
    }

    #[test]
    fn unlabelled_edges_are_reported_as_such() {
        let mut edge = call("main", "a-parse", "x");
        edge.metadata = "line:2".into();
        let score = score_language(&gold(), &extraction("", vec![edge]), "");
        assert_eq!(score.calls.by_label[UNLABELLED].true_positives, 1);
    }

    #[test]
    fn f1_is_undefined_without_predictions() {
        let counts = Counts {
            gold: 3,
            ..Counts::default()
        };
        assert_eq!(counts.precision(), None);
        assert_eq!(counts.f1(), None);
        assert_eq!(counts.recall(), Some(0.0));
    }
}
