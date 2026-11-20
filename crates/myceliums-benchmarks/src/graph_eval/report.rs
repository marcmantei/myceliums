//! The report and baseline format for the graph-accuracy evaluation.
//!
//! One shape serves `graph-gold/report.json` (every run) and
//! `graph-gold/baseline.json` (committed, with a provenance header) — the
//! same arrangement as the retrieval evaluation. Counts are kept, not just
//! rates, so a reader can recompute every number and see its denominator.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::score::{ratio, Counts, LabelCounts, LanguageScore, TagCounts, Totals};
use crate::eval::report::round;

/// Layout version of report and baseline.
pub const SCHEMA_VERSION: &str = "1.0";

/// The command that regenerates the baseline, recorded inside it.
pub const REGENERATE_BASELINE: &str =
    "cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline";

/// Each fixture analyzed as its own repository.
pub const ISOLATED: &str = "isolated";
/// All fixtures analyzed as one repository, so names collide across languages.
pub const POLYGLOT: &str = "polyglot";

/// Rates derived from counts, rounded for diffing. `null` where undefined.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

impl Rates {
    pub fn of(counts: &Counts) -> Self {
        Self {
            precision: counts.precision().map(round),
            recall: counts.recall().map(round),
            f1: counts.f1().map(round),
        }
    }
}

/// Headline numbers of one language (or all of them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub symbols: Counts,
    pub symbol_rates: Rates,
    pub calls: Counts,
    pub call_rates: Rates,
    /// False positives made at a call site outside the caller's body.
    pub wrong_caller: usize,
}

impl Summary {
    fn of(symbols: &Counts, calls: &Counts, wrong_caller: usize) -> Self {
        Self {
            symbols: *symbols,
            symbol_rates: Rates::of(symbols),
            calls: *calls,
            call_rates: Rates::of(calls),
            wrong_caller,
        }
    }
}

/// One resolution label's edges, with derived rates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelSummary {
    #[serde(flatten)]
    pub counts: LabelCounts,
    /// Correct share of this label's scored edges.
    pub precision: Option<f64>,
    /// Share of all gold edges found with this label.
    pub recall_share: Option<f64>,
}

impl LabelSummary {
    fn of(counts: &LabelCounts, gold_edges: usize) -> Self {
        Self {
            counts: *counts,
            precision: ratio(counts.true_positives, counts.predicted).map(round),
            recall_share: ratio(counts.true_positives, gold_edges).map(round),
        }
    }
}

/// One setup (isolated or polyglot): summaries and the full detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupReport {
    pub overall: Summary,
    /// Per language, keyed by language.
    pub summary: BTreeMap<String, Summary>,
    /// All languages together, per resolution label.
    pub by_label: BTreeMap<String, LabelSummary>,
    /// Per language and resolution label.
    pub by_language_label: BTreeMap<String, BTreeMap<String, LabelSummary>>,
    /// Gold edges found, per tag, all languages together.
    pub by_tag: BTreeMap<String, TagCounts>,
    /// Lists of false positives and negatives, per language.
    pub detail: BTreeMap<String, LanguageScore>,
}

impl SetupReport {
    pub fn new(scores: &[(String, LanguageScore)]) -> Self {
        let totals = Totals::of(scores.iter().map(|(_, s)| s));
        let labels = |by_label: &BTreeMap<String, LabelCounts>, gold: usize| {
            by_label
                .iter()
                .map(|(label, counts)| (label.clone(), LabelSummary::of(counts, gold)))
                .collect()
        };
        Self {
            overall: Summary::of(&totals.symbols, &totals.calls, totals.wrong_caller),
            summary: scores
                .iter()
                .map(|(lang, s)| {
                    (
                        lang.clone(),
                        Summary::of(&s.symbols.counts, &s.calls.counts, s.calls.wrong_caller),
                    )
                })
                .collect(),
            by_label: labels(&totals.by_label, totals.calls.gold),
            by_language_label: scores
                .iter()
                .map(|(lang, s)| (lang.clone(), labels(&s.calls.by_label, s.calls.counts.gold)))
                .collect(),
            by_tag: totals.by_tag,
            detail: scores.iter().cloned().collect(),
        }
    }
}

/// A full run, as written to `report.json` and `baseline.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub schema_version: String,
    /// How to regenerate this file — baseline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regenerate_with: Option<String>,
    /// When the baseline was recorded — baseline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Commit the baseline was measured at — baseline only, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Digest of the gold labels and the fixture sources (see `GoldSet::digest`).
    pub dataset_digest: String,
    /// Who labelled the gold set and who reviewed it — `null` until a human has.
    pub labelled_by: String,
    pub reviewed_by: Option<String>,
    /// `os-arch` of the run, recorded in case a platform difference (say,
    /// in path handling) ever shows up in the numbers.
    pub platform: String,
    /// Keyed by setup: [`ISOLATED`], [`POLYGLOT`].
    pub setups: BTreeMap<String, SetupReport>,
}

impl RunReport {
    pub fn read(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let report: RunReport =
            serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))?;
        if report.schema_version != SCHEMA_VERSION {
            bail!(
                "{} has schema {}, this evaluator writes {SCHEMA_VERSION}; re-record it with `{REGENERATE_BASELINE}`",
                path.display(),
                report.schema_version
            );
        }
        Ok(report)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json + "\n").with_context(|| format!("writing {}", path.display()))
    }
}

/// `os-arch`, e.g. `macos-aarch64`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The Wilson score interval (95%) for `successes` of `n`, `None` for
/// `n == 0`. Unlike the normal approximation it stays inside [0, 1] and is
/// sensible at 0 or `n` successes, which these small, often perfect counts
/// need.
pub fn wilson(successes: usize, n: usize) -> Option<(f64, f64)> {
    if n == 0 {
        return None;
    }
    const Z: f64 = 1.959_963_984_540_054;
    let (n, p) = (n as f64, successes as f64 / n as f64);
    let denominator = 1.0 + Z * Z / n;
    let centre = (p + Z * Z / (2.0 * n)) / denominator;
    let half = Z * (p * (1.0 - p) / n + Z * Z / (4.0 * n * n)).sqrt() / denominator;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

/// `0.9512 (0.91–0.98)`: a rate with its Wilson interval.
fn rate_with_interval(successes: usize, n: usize) -> String {
    match wilson(successes, n) {
        Some((low, high)) => format!("{:.4} ({low:.2}–{high:.2})", successes as f64 / n as f64),
        None => "n/a".to_string(),
    }
}

/// `0.8571` style, or `n/a`.
fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |v| format!("{v:.4}"))
}

/// The Markdown report: headline tables, then the lists.
pub fn markdown(report: &RunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Graph accuracy (symbols and CALLS edges)\n");
    let _ = writeln!(
        out,
        "Gold set labelled by `{}`, reviewed by `{}`; digest `{}`; platform `{}`.\n",
        report.labelled_by,
        report.reviewed_by.as_deref().unwrap_or("nobody yet"),
        &report.dataset_digest[..12.min(report.dataset_digest.len())],
        report.platform
    );
    for (setup, result) in &report.setups {
        let _ = writeln!(out, "### {setup}\n");
        let _ = writeln!(
            out,
            "| language | symbol P | symbol R | gold edges | extracted | TP | FP | of which wrong caller | FN | call P (95% CI) | call R (95% CI) | call F1 |"
        );
        let _ = writeln!(
            out,
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
        );
        let rows = result
            .summary
            .iter()
            .map(|(lang, s)| (lang.as_str(), s))
            .chain(std::iter::once(("**all**", &result.overall)));
        for (lang, s) in rows {
            let _ = writeln!(
                out,
                "| {lang} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                pct(s.symbol_rates.precision),
                pct(s.symbol_rates.recall),
                s.calls.gold,
                s.calls.predicted,
                s.calls.true_positives,
                s.calls.false_positives,
                s.wrong_caller,
                s.calls.false_negatives,
                rate_with_interval(s.calls.true_positives, s.calls.predicted),
                rate_with_interval(s.calls.true_positives, s.calls.gold),
                pct(s.call_rates.f1),
            );
        }
        let _ = writeln!(
            out,
            "\nCALLS edges by resolution label (precision with 95% Wilson interval; TP/extracted):\n"
        );
        let labels: Vec<&String> = result.by_label.keys().collect();
        let _ = writeln!(
            out,
            "| language | {} |",
            labels
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let _ = writeln!(out, "|---|{}", "---:|".repeat(labels.len()));
        let cell = |l: Option<&LabelSummary>| match l {
            Some(l) => format!(
                "{} {}/{}",
                rate_with_interval(l.counts.true_positives, l.counts.predicted),
                l.counts.true_positives,
                l.counts.predicted
            ),
            None => "–".to_string(),
        };
        for (lang, by_label) in &result.by_language_label {
            let cells: Vec<String> = labels.iter().map(|l| cell(by_label.get(*l))).collect();
            let _ = writeln!(out, "| {lang} | {} |", cells.join(" | "));
        }
        let cells: Vec<String> = labels
            .iter()
            .map(|l| cell(result.by_label.get(*l)))
            .collect();
        let _ = writeln!(out, "| **all** | {} |", cells.join(" | "));
        let shares: Vec<String> = labels
            .iter()
            .map(|l| pct(result.by_label[*l].recall_share))
            .collect();
        let _ = writeln!(out, "| recall share | {} |", shares.join(" | "));

        let _ = writeln!(out, "\nGold edges found, by tag:\n");
        let _ = writeln!(out, "| tag | found / gold |");
        let _ = writeln!(out, "|---|---:|");
        for (tag, counts) in &result.by_tag {
            let _ = writeln!(out, "| {tag} | {} / {} |", counts.found, counts.gold);
        }
        let _ = writeln!(out);
    }
    if let Some(isolated) = report.setups.get(ISOLATED) {
        let _ = writeln!(out, "### False positives and negatives ({ISOLATED})\n");
        for (lang, detail) in &isolated.detail {
            let _ = writeln!(out, "<details><summary>{lang}</summary>\n");
            let _ = writeln!(out, "Wrong CALLS edges:\n");
            for e in &detail.calls.false_positives {
                let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
                let note = e
                    .note
                    .as_deref()
                    .map(|n| format!(" — {n}"))
                    .unwrap_or_default();
                let _ = writeln!(out, "- `{}` → `{}` [{}]{line}{note}", e.from, e.to, e.label);
            }
            let _ = writeln!(out, "\nMissed CALLS edges:\n");
            for e in &detail.calls.false_negatives {
                let _ = writeln!(
                    out,
                    "- `{}` → `{}` (line {}; {}) — {}",
                    e.from,
                    e.to,
                    e.line,
                    e.tags.join(", "),
                    e.why
                );
            }
            let symbols = &detail.symbols;
            if !symbols.false_negatives.is_empty() {
                let _ = writeln!(
                    out,
                    "\nMissed symbols: {}",
                    code_list(&symbols.false_negatives)
                );
            }
            if !symbols.false_positives.is_empty() {
                let _ = writeln!(
                    out,
                    "\nExtra symbols: {}",
                    code_list(&symbols.false_positives)
                );
            }
            if !symbols.kind_mismatches.is_empty() {
                let _ = writeln!(
                    out,
                    "\nKind disagreements: {}",
                    code_list(&symbols.kind_mismatches)
                );
            }
            let _ = writeln!(out, "\n</details>\n");
        }
    }
    out
}

fn code_list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Current vs baseline headline rates per language, as Markdown. Only the
/// overall rates are gated ([`super::gate`]); these are for reading.
pub fn markdown_comparison(current: &RunReport, baseline: &RunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### Against the recorded baseline, per language\n");
    if current.dataset_digest != baseline.dataset_digest {
        let _ = writeln!(
            out,
            "The baseline was recorded against a different gold set or fixture \
             (digest `{}` vs `{}`): the numbers below are not comparable. \
             Re-record with `{REGENERATE_BASELINE}`.\n",
            &baseline.dataset_digest[..12.min(baseline.dataset_digest.len())],
            &current.dataset_digest[..12.min(current.dataset_digest.len())],
        );
    }
    if current.platform != baseline.platform {
        let _ = writeln!(
            out,
            "Baseline platform `{}`, this run `{}`: ambiguous resolutions can \
             differ if anything platform-specific reaches the analyzer.\n",
            baseline.platform, current.platform
        );
    }
    let _ = writeln!(
        out,
        "| setup | language | call P | Δ | call R | Δ | symbol R | Δ |"
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|---:|");
    let delta = |now: Option<f64>, then: Option<f64>| match (now, then) {
        (Some(a), Some(b)) => format!("{:+.4}", round(a - b)),
        _ => "n/a".to_string(),
    };
    for (setup, now) in &current.setups {
        let Some(then) = baseline.setups.get(setup) else {
            continue;
        };
        let rows = now
            .summary
            .iter()
            .map(|(l, s)| (l.clone(), s, then.summary.get(l)))
            .chain(std::iter::once((
                "all".to_string(),
                &now.overall,
                Some(&then.overall),
            )));
        for (lang, a, b) in rows {
            let Some(b) = b else { continue };
            let _ = writeln!(
                out,
                "| {setup} | {lang} | {} | {} | {} | {} | {} | {} |",
                pct(a.call_rates.precision),
                delta(a.call_rates.precision, b.call_rates.precision),
                pct(a.call_rates.recall),
                delta(a.call_rates.recall, b.call_rates.recall),
                pct(a.symbol_rates.recall),
                delta(a.symbol_rates.recall, b.symbol_rates.recall),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> RunReport {
        let mut score = LanguageScore::default();
        score.calls.counts = Counts {
            gold: 4,
            predicted: 4,
            true_positives: 3,
            false_positives: 1,
            false_negatives: 1,
        };
        score.calls.by_label.insert(
            "exact".into(),
            LabelCounts {
                predicted: 4,
                true_positives: 3,
                false_positives: 1,
            },
        );
        let setup = SetupReport::new(&[("python".into(), score)]);
        RunReport {
            schema_version: SCHEMA_VERSION.into(),
            regenerate_with: None,
            timestamp: None,
            commit_sha: None,
            dataset_digest: "abc".into(),
            labelled_by: "agent".into(),
            reviewed_by: None,
            platform: platform(),
            setups: BTreeMap::from([(ISOLATED.to_string(), setup)]),
        }
    }

    #[test]
    fn wilson_intervals_match_known_values() {
        // 139 of 245: the #94 baseline's call precision, 0.50–0.63.
        let (low, high) = wilson(139, 245).unwrap();
        assert!(
            (low - 0.505).abs() < 0.002 && (high - 0.629).abs() < 0.002,
            "{low} {high}"
        );
        // A perfect small count is not certain: 2 of 2 is 0.34–1.00.
        let (low, high) = wilson(2, 2).unwrap();
        assert!((low - 0.342).abs() < 0.002 && high == 1.0, "{low} {high}");
        assert_eq!(wilson(0, 0), None);
    }

    #[test]
    fn rates_are_derived_from_counts() {
        let report = report();
        let setup = &report.setups[ISOLATED];
        assert_eq!(setup.overall.call_rates.precision, Some(0.75));
        assert_eq!(setup.overall.call_rates.recall, Some(0.75));
        assert_eq!(setup.by_label["exact"].recall_share, Some(0.75));
        assert_eq!(setup.overall.symbol_rates.precision, None);
    }

    #[test]
    fn a_report_round_trips_through_json() {
        let report = report();
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(serde_json::from_str::<RunReport>(&json).unwrap(), report);
    }

    #[test]
    fn markdown_names_every_language_and_label() {
        let text = markdown(&report());
        assert!(text.contains("| python |"), "{text}");
        assert!(text.contains("exact"), "{text}");
    }

    #[test]
    fn a_comparison_flags_a_different_dataset() {
        let current = report();
        let mut baseline = report();
        baseline.dataset_digest = "def".into();
        assert!(markdown_comparison(&current, &baseline).contains("not comparable"));
        assert!(!markdown_comparison(&current, &current).contains("not comparable"));
    }
}
