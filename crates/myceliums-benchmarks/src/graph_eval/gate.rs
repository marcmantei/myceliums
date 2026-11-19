//! The CI gate for graph accuracy: does the current run lose CALLS-edge
//! precision or recall against the recorded baseline?
//!
//! # Decision rule
//!
//! The same rule as the retrieval gate ([`crate::eval::gate`], see
//! `benchmarks/METHODOLOGY.md`, "The CI gate"). For each setup the baseline
//! measured (`isolated`, `polyglot`) and each gated metric — CALLS precision
//! and CALLS recall over all languages — take the paired bootstrap difference
//! `current − baseline` over the gold set's edges. It is a **regression** only
//! when
//!
//! 1. the whole 95% interval lies below zero, and
//! 2. the drop is at least [`MIN_EFFECT`] (0.02: about five of the 249 gold
//!    edges).
//!
//! Recall pairs the gold edges: each is found or not in either run.
//! Precision pairs the union of the edges either run extracted that the
//! scoring counts: each gold edge found, and each false edge, with its
//! contribution to either run's numerator and denominator
//! ([`paired_ratio_bootstrap`]). The evaluation is deterministic, so a run
//! that does not change the analyzer cannot move these at all; the interval
//! asks whether a change moved more edges than a handful of flips explains.
//!
//! Per-language and per-label numbers are reported, not gated: 43–59 edges
//! per language are too few to gate on.
//!
//! # Invariant
//!
//! Independently of any baseline, the polyglot run must have no CALLS edge
//! between two languages: the resolver never links across language families
//! (#139), so one such edge is a bug, not noise.
//!
//! The gate also fails, as a broken comparison, when the baseline was
//! recorded against other labels or fixtures (digest mismatch), or lacks a
//! setup the current run measured.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::Result;

use super::gold::GoldSet;
use super::report::{RunReport, SetupReport, POLYGLOT, REGENERATE_BASELINE};
use crate::eval::bootstrap::{paired_ratio_bootstrap, DeltaEstimate};
use crate::eval::gate::{classify, Verdict, MIN_EFFECT};

/// A gated metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatedMetric {
    CallPrecision,
    CallRecall,
}

impl GatedMetric {
    pub fn label(self) -> &'static str {
        match self {
            GatedMetric::CallPrecision => "call precision",
            GatedMetric::CallRecall => "call recall",
        }
    }
}

/// One setup × metric comparison against the baseline.
#[derive(Debug, Clone)]
pub struct Finding {
    pub setup: String,
    pub metric: GatedMetric,
    pub baseline: f64,
    pub current: f64,
    pub estimate: DeltaEstimate,
    pub verdict: Verdict,
}

/// The result of gating a run.
#[derive(Debug, Clone, Default)]
pub struct GateOutcome {
    pub findings: Vec<Finding>,
    /// Broken invariants or comparisons; any entry fails the gate.
    pub errors: Vec<String>,
    /// Worth knowing, never failing.
    pub notes: Vec<String>,
}

impl GateOutcome {
    /// True when the build must fail.
    pub fn failed(&self) -> bool {
        !self.errors.is_empty()
            || self
                .findings
                .iter()
                .any(|f| f.verdict == Verdict::Regression)
    }
}

/// A gold edge or an extracted false edge, by language and endpoints.
type EdgeKey = (String, String, String);

/// What one run says about the edges of one setup.
struct RunEdges {
    /// Every gold edge, and whether this run found it.
    gold: BTreeMap<EdgeKey, bool>,
    /// The false edges this run extracted.
    false_edges: BTreeSet<EdgeKey>,
}

impl RunEdges {
    fn of(setup: &SetupReport, gold: &GoldSet) -> Self {
        let mut edges = RunEdges {
            gold: BTreeMap::new(),
            false_edges: BTreeSet::new(),
        };
        for language in &gold.languages {
            let detail = setup.detail.get(&language.language);
            let missed: BTreeSet<(&str, &str)> = detail
                .map(|d| {
                    d.calls
                        .false_negatives
                        .iter()
                        .map(|e| (e.from.as_str(), e.to.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            for call in &language.calls {
                let found =
                    detail.is_some() && !missed.contains(&(call.from.as_str(), call.to.as_str()));
                edges.gold.insert(
                    (
                        language.language.clone(),
                        call.from.clone(),
                        call.to.clone(),
                    ),
                    found,
                );
            }
            for wrong in detail.iter().flat_map(|d| &d.calls.false_positives) {
                edges.false_edges.insert((
                    language.language.clone(),
                    wrong.from.clone(),
                    wrong.to.clone(),
                ));
            }
        }
        edges
    }

    /// `(numerator, denominator)` of this run's precision for one item.
    fn precision_item(&self, key: &EdgeKey) -> (f64, f64) {
        match self.gold.get(key) {
            Some(true) => (1.0, 1.0),
            Some(false) => (0.0, 0.0),
            None if self.false_edges.contains(key) => (0.0, 1.0),
            None => (0.0, 0.0),
        }
    }
}

/// Paired estimates of `current − baseline` for both gated metrics.
fn estimates(current: &RunEdges, baseline: &RunEdges) -> Result<[(GatedMetric, DeltaEstimate); 2]> {
    let found = |edges: &RunEdges| -> Vec<(f64, f64)> {
        edges
            .gold
            .values()
            .map(|&found| (f64::from(u8::from(found)), 1.0))
            .collect()
    };
    let recall = paired_ratio_bootstrap(&found(current), &found(baseline))?;

    let items: BTreeSet<&EdgeKey> = [current, baseline]
        .into_iter()
        .flat_map(|edges| {
            edges
                .gold
                .iter()
                .filter(|(_, &found)| found)
                .map(|(key, _)| key)
                .chain(&edges.false_edges)
        })
        .collect();
    let contributions = |edges: &RunEdges| -> Vec<(f64, f64)> {
        items.iter().map(|key| edges.precision_item(key)).collect()
    };
    let precision = paired_ratio_bootstrap(&contributions(current), &contributions(baseline))?;
    Ok([
        (GatedMetric::CallPrecision, precision),
        (GatedMetric::CallRecall, recall),
    ])
}

/// False edges in the polyglot setup that connect two fixtures' languages.
pub fn cross_language_edges(report: &RunReport) -> usize {
    report.setups.get(POLYGLOT).map_or(0, |setup| {
        setup
            .detail
            .values()
            .flat_map(|d| &d.calls.false_positives)
            .filter(|e| e.to.ends_with("[other fixture]"))
            .count()
    })
}

/// Gate `current` against `baseline`.
pub fn compare(current: &RunReport, baseline: &RunReport, gold: &GoldSet) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::default();

    let crossing = cross_language_edges(current);
    if crossing > 0 {
        outcome.errors.push(format!(
            "{crossing} CALLS edge(s) in the {POLYGLOT} run connect two languages; \
             a call is never resolved across language families"
        ));
    }
    if current.dataset_digest != baseline.dataset_digest {
        outcome.errors.push(format!(
            "the baseline was recorded against other labels or fixtures (digest {} vs {}); \
             re-record it: {REGENERATE_BASELINE}",
            short(&baseline.dataset_digest),
            short(&current.dataset_digest)
        ));
        return Ok(outcome);
    }
    if current.platform != baseline.platform {
        outcome.notes.push(format!(
            "baseline platform `{}`, this run `{}`",
            baseline.platform, current.platform
        ));
    }
    for (setup, now) in &current.setups {
        let Some(then) = baseline.setups.get(setup) else {
            outcome.errors.push(format!(
                "the baseline has no `{setup}` setup to compare with"
            ));
            continue;
        };
        let current_edges = RunEdges::of(now, gold);
        let baseline_edges = RunEdges::of(then, gold);
        for (metric, estimate) in estimates(&current_edges, &baseline_edges)? {
            let (current_rate, baseline_rate) = match metric {
                GatedMetric::CallPrecision => (
                    now.overall.calls.precision(),
                    then.overall.calls.precision(),
                ),
                GatedMetric::CallRecall => {
                    (now.overall.calls.recall(), then.overall.calls.recall())
                }
            };
            outcome.findings.push(Finding {
                setup: setup.clone(),
                metric,
                baseline: baseline_rate.unwrap_or(0.0),
                current: current_rate.unwrap_or(0.0),
                estimate,
                verdict: classify(&estimate),
            });
        }
    }
    Ok(outcome)
}

fn short(digest: &str) -> &str {
    &digest[..12.min(digest.len())]
}

/// The gate's verdict as Markdown.
pub fn markdown(outcome: &GateOutcome) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### Gate against the recorded baseline\n");
    let _ = writeln!(
        out,
        "A regression is a drop of at least {MIN_EFFECT} whose paired bootstrap 95% \
         interval lies below zero; the {POLYGLOT} run must have no cross-language edge.\n"
    );
    let _ = writeln!(
        out,
        "| setup | metric | baseline | current | Δ [95% CI] | verdict |"
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---|");
    for f in &outcome.findings {
        let verdict = match f.verdict {
            Verdict::Regression => "REGRESSION",
            Verdict::Improvement => "improvement",
            Verdict::NoMaterialChange => "no material change",
        };
        let _ = writeln!(
            out,
            "| {} | {} | {:.4} | {:.4} | {:+.4} [{:+.4}, {:+.4}] | {verdict} |",
            f.setup,
            f.metric.label(),
            f.baseline,
            f.current,
            f.estimate.delta,
            f.estimate.ci_low,
            f.estimate.ci_high
        );
    }
    for note in &outcome.notes {
        let _ = writeln!(out, "\nNote: {note}");
    }
    for error in &outcome.errors {
        let _ = writeln!(out, "\n**FAILED**: {error}");
    }
    let _ = writeln!(
        out,
        "\n{}",
        if outcome.failed() {
            "**FAILED** — accept an intended drop by re-recording the baseline in the same PR."
        } else {
            "**PASSED** — no gated metric regressed beyond the noise."
        }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_eval::report::ISOLATED;
    use crate::graph_eval::score::{Counts, MissedEdge, WrongEdge};

    fn baseline() -> RunReport {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("graph-gold/baseline.json");
        RunReport::read(&path).unwrap()
    }

    /// A copy of `report` where the isolated run additionally misses up to
    /// `missed` gold edges of the first language and extracts `wrong` false
    /// edges.
    fn degrade(report: &RunReport, gold: &GoldSet, missed: usize, wrong: usize) -> RunReport {
        let mut report = report.clone();
        let setup = report.setups.get_mut(ISOLATED).unwrap();
        let language = &gold.languages[0];
        let detail = setup.detail.get_mut(&language.language).unwrap();
        let already: BTreeSet<(String, String)> = detail
            .calls
            .false_negatives
            .iter()
            .map(|e| (e.from.clone(), e.to.clone()))
            .collect();
        let newly_missed: Vec<MissedEdge> = language
            .calls
            .iter()
            .filter(|c| !already.contains(&(c.from.clone(), c.to.clone())))
            .take(missed)
            .map(|c| MissedEdge {
                from: c.from.clone(),
                to: c.to.clone(),
                line: c.line,
                tags: vec![],
                why: String::new(),
            })
            .collect();
        let added = newly_missed.len();
        detail.calls.false_negatives.extend(newly_missed);
        for i in 0..wrong {
            detail.calls.false_positives.push(WrongEdge {
                from: format!("wrong{i}"),
                to: "nowhere".into(),
                label: "ambiguous".into(),
                line: None,
                note: None,
            });
        }
        let c = &mut setup.overall.calls;
        *c = Counts {
            gold: c.gold,
            predicted: c.predicted - added + wrong,
            true_positives: c.true_positives - added,
            false_positives: c.false_positives + wrong,
            false_negatives: c.false_negatives + added,
        };
        report
    }

    #[test]
    fn the_baseline_passes_against_itself() {
        let gold = GoldSet::embedded().unwrap();
        let baseline = baseline();
        let outcome = compare(&baseline, &baseline, &gold).unwrap();
        assert!(!outcome.failed(), "{}", markdown(&outcome));
        assert_eq!(outcome.findings.len(), 2 * baseline.setups.len());
        for finding in &outcome.findings {
            assert_eq!(finding.estimate.delta, 0.0);
        }
    }

    #[test]
    fn a_few_flipped_edges_pass_and_many_fail() {
        let gold = GoldSet::embedded().unwrap();
        let baseline = baseline();
        let few = degrade(&baseline, &gold, 2, 2);
        assert!(!compare(&few, &baseline, &gold).unwrap().failed());

        let many = degrade(&baseline, &gold, 12, 0);
        let outcome = compare(&many, &baseline, &gold).unwrap();
        assert!(outcome.failed(), "{}", markdown(&outcome));
        let failed: Vec<_> = outcome
            .findings
            .iter()
            .filter(|f| f.verdict == Verdict::Regression)
            .map(|f| (f.setup.as_str(), f.metric))
            .collect();
        assert_eq!(failed, [(ISOLATED, GatedMetric::CallRecall)]);

        let noisy = degrade(&baseline, &gold, 0, 15);
        let outcome = compare(&noisy, &baseline, &gold).unwrap();
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.metric == GatedMetric::CallPrecision && f.verdict == Verdict::Regression));
    }

    #[test]
    fn a_cross_language_edge_fails_whatever_the_baseline() {
        let gold = GoldSet::embedded().unwrap();
        let baseline = baseline();
        let mut crossing = baseline.clone();
        let polyglot = crossing.setups.get_mut(POLYGLOT).unwrap();
        let detail = polyglot.detail.values_mut().next().unwrap();
        detail.calls.false_positives.push(WrongEdge {
            from: "x".into(),
            to: "go/main.go::run [other fixture]".into(),
            label: "name-match".into(),
            line: None,
            note: None,
        });
        let outcome = compare(&crossing, &baseline, &gold).unwrap();
        assert!(outcome.failed());
        assert!(outcome.errors[0].contains("connect two languages"));
    }

    #[test]
    fn a_baseline_for_other_labels_is_refused() {
        let gold = GoldSet::embedded().unwrap();
        let baseline = baseline();
        let mut other = baseline.clone();
        other.dataset_digest = "0".repeat(64);
        let outcome = compare(&baseline, &other, &gold).unwrap();
        assert!(outcome.failed());
    }
}
