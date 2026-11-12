//! The CI gate: does the current run regress against the recorded baseline?
//!
//! # Decision rule
//!
//! For every mode measured in the baseline and every metric, the gate
//! computes the paired bootstrap difference `current − baseline` over the
//! golden queries ([`super::bootstrap`]). A difference is a **regression**
//! only when both hold:
//!
//! 1. it is *significant*: the upper end of the 95% interval is below zero —
//!    a per-comparison interval, not corrected for the 36 mode × metric
//!    comparisons the gate makes — and
//! 2. it is *material*: the point estimate is at least [`MIN_EFFECT`] below zero.
//!
//! The build fails when any [`GATED_METRICS`] entry regresses in any mode.
//! Every other difference — significant or not, gated metric or not — is
//! reported and never fails the build.
//!
//! Both conditions are needed. Significance alone would fail on a consistent
//! but negligible shift (nDCG moving by 0.005 across many queries); a size
//! threshold alone would fail on one query flipping, which on 51 queries is
//! indistinguishable from noise.
//!
//! The gate also fails — as a broken comparison rather than a regression —
//! when it cannot compare honestly: the baseline belongs to a different golden
//! set, or a mode the baseline measured did not run now. Silently skipping a
//! mode would turn a missing model cache into a green build.

use anyhow::Result;
use std::fmt::Write as _;

use super::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use super::evaluator::{Metric, SearchMode};
use super::report::{format_delta, rounded, RunReport, REGENERATE_BASELINE};

/// Smallest drop that can fail the build, in absolute metric units.
///
/// 0.02 is about one query of 51 going from a perfect score to nothing
/// (1/51 ≈ 0.0196). A change whose (uncorrected) interval excludes zero but
/// that is smaller than this is a real but
/// immaterial shift; it is reported, not blocked.
pub const MIN_EFFECT: f64 = 0.02;

/// The metrics a regression in which fails the build.
///
/// MRR@10 is "how high is the first answer" — what an agent with a small
/// context window lives on. nDCG@10 sees every answer's position in the first
/// page. recall@10 is coverage of all labelled answers. The remaining metrics
/// (recall@1/5, hit@k, R-precision) are close relatives of these three and are
/// reported only, so one ranking change is not counted against the build
/// several times over.
pub const GATED_METRICS: [Metric; 3] = [Metric::Mrr, Metric::NdcgAt10, Metric::RecallAt10];

/// How one difference is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Significant and at least [`MIN_EFFECT`] worse.
    Regression,
    /// Significant and at least [`MIN_EFFECT`] better.
    Improvement,
    /// Inside the noise, or too small to matter.
    NoMaterialChange,
}

impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Verdict::Regression => "REGRESSION",
            Verdict::Improvement => "improvement",
            Verdict::NoMaterialChange => "no material change",
        }
    }
}

/// Apply the decision rule to one difference.
pub fn classify(estimate: &DeltaEstimate) -> Verdict {
    if estimate.significantly_negative() && estimate.delta <= -MIN_EFFECT {
        Verdict::Regression
    } else if estimate.significantly_positive() && estimate.delta >= MIN_EFFECT {
        Verdict::Improvement
    } else {
        Verdict::NoMaterialChange
    }
}

/// One mode × metric comparison against the baseline.
#[derive(Debug, Clone)]
pub struct Finding {
    pub mode: SearchMode,
    pub metric: Metric,
    pub baseline: f64,
    pub current: f64,
    pub estimate: DeltaEstimate,
    pub verdict: Verdict,
}

impl Finding {
    /// True when this finding fails the build.
    pub fn fails_build(&self) -> bool {
        self.verdict == Verdict::Regression && GATED_METRICS.contains(&self.metric)
    }
}

/// The result of comparing a run against the baseline.
#[derive(Debug, Clone, Default)]
pub struct GateOutcome {
    /// Every mode × metric comparison that could be made.
    pub findings: Vec<Finding>,
    /// Reasons the comparison itself is broken; any entry fails the gate.
    pub errors: Vec<String>,
    /// Things the reader should know that do not fail the gate.
    pub notes: Vec<String>,
}

impl GateOutcome {
    /// True when the build must fail.
    pub fn failed(&self) -> bool {
        !self.errors.is_empty() || self.findings.iter().any(Finding::fails_build)
    }

    /// The findings that fail the build.
    pub fn regressions(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fails_build())
    }
}

/// Compare `current` against `baseline` under the decision rule.
pub fn compare(current: &RunReport, baseline: &RunReport) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::default();

    if current.dataset_digest != baseline.dataset_digest {
        outcome.errors.push(format!(
            "the baseline was recorded against a different golden set (digest {} vs {}); \
             scores on different labels are not comparable. Re-record: {REGENERATE_BASELINE}",
            short(&baseline.dataset_digest),
            short(&current.dataset_digest)
        ));
        return Ok(outcome);
    }
    if current.models != baseline.models && current.models.is_some() {
        outcome.notes.push(format!(
            "model configuration differs from the baseline — baseline {}, current {}. \
             The comparison measures that change.",
            describe_models(baseline),
            describe_models(current)
        ));
    }
    if current.platform != baseline.platform {
        outcome.notes.push(format!(
            "baseline recorded on {}, this run on {}. Model inference is reproducible \
             per platform, not guaranteed bit-identical across them, so small model-mode \
             deltas can be platform noise; requiring both a 95% interval below zero \
             (uncorrected) and a drop of at least the minimal effect is what keeps them \
             from failing.",
            baseline.platform, current.platform
        ));
    }

    let query_ids = baseline.query_ids();
    for mode in SearchMode::all() {
        let Some(was) = baseline.measured(mode.id()) else {
            if current.measured(mode.id()).is_some() {
                outcome.notes.push(format!(
                    "`{}` is measured now but not in the baseline — nothing to compare; \
                     re-record the baseline to start gating it",
                    mode.id()
                ));
            }
            continue;
        };
        let Some(now) = current.measured(mode.id()) else {
            let reason = current
                .results
                .get(mode.id())
                .and_then(|m| m.reason.clone())
                .unwrap_or_else(|| "not run".to_string());
            outcome.errors.push(format!(
                "`{}` is measured in the baseline but did not run now ({reason}); the gate \
                 cannot vouch for a mode it did not score",
                mode.id()
            ));
            continue;
        };
        for metric in Metric::ALL {
            let (current_values, baseline_values) = match (
                now.aligned(metric, &query_ids),
                was.aligned(metric, &query_ids),
            ) {
                (Ok(c), Ok(b)) => (c, b),
                (Err(error), _) | (_, Err(error)) => {
                    outcome.errors.push(format!(
                        "`{}` {}: per-query scores do not line up with the baseline \
                             ({error}). Re-record: {REGENERATE_BASELINE}",
                        mode.id(),
                        metric.id()
                    ));
                    break;
                }
            };
            let estimate = rounded(paired_bootstrap_default(&current_values, &baseline_values)?);
            outcome.findings.push(Finding {
                mode,
                metric,
                baseline: was.metrics.map(|m| m.get(metric)).unwrap_or_default(),
                current: now.metrics.map(|m| m.get(metric)).unwrap_or_default(),
                verdict: classify(&estimate),
                estimate,
            });
        }
    }
    Ok(outcome)
}

/// Markdown summary of a gate outcome, for the CI job summary and stdout.
pub fn markdown(outcome: &GateOutcome) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### Current vs baseline\n");
    let _ = writeln!(
        out,
        "Δ = current − baseline, paired bootstrap 95% CI over the golden queries. \
         The build fails only on a **gated** metric whose interval lies wholly below \
         zero *and* whose drop is at least {MIN_EFFECT} — see \
         `crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`. Intervals are \
         per comparison, uncorrected for the {} mode × metric comparisons below.\n",
        outcome.findings.len()
    );
    if !outcome.findings.is_empty() {
        let _ = writeln!(
            out,
            "| Mode | Metric | Baseline | Current | Δ [95% CI] | Verdict |"
        );
        let _ = writeln!(out, "| --- | --- | ---: | ---: | ---: | --- |");
        for finding in &outcome.findings {
            let gated = if GATED_METRICS.contains(&finding.metric) {
                " (gated)"
            } else {
                ""
            };
            let verdict = if finding.fails_build() {
                format!("**{}**", finding.verdict.label())
            } else {
                finding.verdict.label().to_string()
            };
            let _ = writeln!(
                out,
                "| `{}` | {}{gated} | {:.4} | {:.4} | {} | {verdict} |",
                finding.mode.id(),
                finding.metric.label(),
                finding.baseline,
                finding.current,
                format_delta(&finding.estimate),
            );
        }
    }
    for note in &outcome.notes {
        let _ = writeln!(out, "\n> note: {note}");
    }
    for error in &outcome.errors {
        let _ = writeln!(out, "\n> **error:** {error}");
    }
    let verdict = if outcome.failed() {
        let regressions: Vec<String> = outcome
            .regressions()
            .map(|f| format!("`{}` {}", f.mode.id(), f.metric.label()))
            .collect();
        if regressions.is_empty() {
            "**FAILED** — the comparison could not be made (see errors above).".to_string()
        } else {
            format!(
                "**FAILED** — regression of at least {MIN_EFFECT} with its 95% CI below zero in {}. If the drop is \
                 intended, re-record the baseline in this PR and say why: `{REGENERATE_BASELINE}`",
                regressions.join(", ")
            )
        }
    } else {
        "**PASSED** — no gated metric regressed beyond the noise.".to_string()
    };
    let _ = writeln!(out, "\n{verdict}");
    out
}

fn short(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

fn describe_models(report: &RunReport) -> String {
    match &report.models {
        Some(models) => format!(
            "{} + {} (text v{})",
            models.embedding_model, models.reranker, models.embedding_text_version
        ),
        None => "none".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluator::{IntentScore, MetricValues, ModeScore, QueryScore};
    use crate::eval::golden_set::{GoldenSet, QueryIntent};
    use crate::eval::report::ModeReport;
    use std::collections::BTreeMap;

    fn estimate(delta: f64, ci_low: f64, ci_high: f64) -> DeltaEstimate {
        DeltaEstimate {
            delta,
            ci_low,
            ci_high,
        }
    }

    #[test]
    fn a_regression_must_be_both_significant_and_material() {
        // Significant and material.
        assert_eq!(
            classify(&estimate(-0.05, -0.09, -0.01)),
            Verdict::Regression
        );
        // Material but the interval reaches zero: noise on 51 queries.
        assert_eq!(
            classify(&estimate(-0.05, -0.12, 0.0)),
            Verdict::NoMaterialChange
        );
        // Significant but smaller than one query's worth of change.
        assert_eq!(
            classify(&estimate(-0.01, -0.015, -0.005)),
            Verdict::NoMaterialChange
        );
        // The threshold itself counts as material.
        assert_eq!(
            classify(&estimate(-MIN_EFFECT, -0.03, -0.001)),
            Verdict::Regression
        );
    }

    #[test]
    fn improvements_are_classified_symmetrically_and_never_fail() {
        assert_eq!(classify(&estimate(0.05, 0.01, 0.09)), Verdict::Improvement);
        assert_eq!(
            classify(&estimate(0.05, -0.01, 0.09)),
            Verdict::NoMaterialChange
        );
        assert_eq!(
            classify(&estimate(0.0, 0.0, 0.0)),
            Verdict::NoMaterialChange
        );
    }

    /// Per-query scores for every golden query, all equal to `base` except the
    /// first `changed` queries, which get `changed_to`.
    fn mode_score(mode: SearchMode, base: f64, changed: usize, changed_to: f64) -> ModeScore {
        let golden = GoldenSet::embedded().unwrap();
        let queries: Vec<QueryScore> = golden
            .queries
            .iter()
            .enumerate()
            .map(|(i, q)| QueryScore {
                id: q.id.clone(),
                intent: q.intent,
                values: MetricValues::from_fn(|_| if i < changed { changed_to } else { base }),
            })
            .collect();
        let mean = MetricValues::mean_of(queries.iter().map(|q| &q.values));
        ModeScore {
            mode,
            mean,
            by_intent: [(
                QueryIntent::ExactName,
                IntentScore {
                    queries: queries.len(),
                    mean,
                },
            )]
            .into_iter()
            .collect(),
            queries,
        }
    }

    fn run(modes: &[(SearchMode, Option<ModeScore>)]) -> RunReport {
        let golden = GoldenSet::embedded().unwrap();
        let results: BTreeMap<String, ModeReport> = modes
            .iter()
            .map(|(mode, score)| {
                let report = match score {
                    Some(score) => ModeReport::measured(score),
                    None => ModeReport::unavailable("weights not cached"),
                };
                (mode.id().to_string(), report)
            })
            .collect();
        RunReport::new(&golden, 96, None, results).unwrap()
    }

    #[test]
    fn an_unchanged_run_passes() {
        let baseline = run(&[(
            SearchMode::Lexical,
            Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
        )]);
        let outcome = compare(&baseline, &baseline).unwrap();
        assert!(!outcome.failed(), "{}", markdown(&outcome));
        assert_eq!(outcome.findings.len(), Metric::ALL.len());
    }

    #[test]
    fn a_broad_regression_fails_the_build() {
        let baseline = run(&[(
            SearchMode::Hybrid,
            Some(mode_score(SearchMode::Hybrid, 1.0, 0, 0.0)),
        )]);
        // Twelve of 51 queries lose their answer entirely.
        let current = run(&[(
            SearchMode::Hybrid,
            Some(mode_score(SearchMode::Hybrid, 1.0, 12, 0.0)),
        )]);
        let outcome = compare(&current, &baseline).unwrap();
        assert!(outcome.failed());
        let mut failing: Vec<Metric> = outcome.regressions().map(|f| f.metric).collect();
        failing.sort();
        let mut gated = GATED_METRICS.to_vec();
        gated.sort();
        assert_eq!(failing, gated, "every gated metric, and only those, fails");
        assert!(markdown(&outcome).contains("FAILED"));
    }

    #[test]
    fn a_single_query_flip_does_not_fail_the_build() {
        let baseline = run(&[(
            SearchMode::Lexical,
            Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
        )]);
        let current = run(&[(
            SearchMode::Lexical,
            Some(mode_score(SearchMode::Lexical, 1.0, 1, 0.0)),
        )]);
        let outcome = compare(&current, &baseline).unwrap();
        assert!(!outcome.failed(), "{}", markdown(&outcome));
    }

    #[test]
    fn a_mode_that_stopped_running_fails_loudly() {
        let baseline = run(&[
            (
                SearchMode::Lexical,
                Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
            ),
            (
                SearchMode::Semantic,
                Some(mode_score(SearchMode::Semantic, 1.0, 0, 0.0)),
            ),
        ]);
        let current = run(&[
            (
                SearchMode::Lexical,
                Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
            ),
            (SearchMode::Semantic, None),
        ]);
        let outcome = compare(&current, &baseline).unwrap();
        assert!(outcome.failed());
        assert!(
            outcome.errors[0].contains("semantic"),
            "{:?}",
            outcome.errors
        );
        assert!(outcome.errors[0].contains("weights not cached"));
    }

    #[test]
    fn a_different_golden_set_is_not_compared() {
        let baseline = run(&[(
            SearchMode::Lexical,
            Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
        )]);
        let mut current = baseline.clone();
        current.dataset_digest = "0".repeat(64);
        let outcome = compare(&current, &baseline).unwrap();
        assert!(outcome.failed());
        assert!(outcome.findings.is_empty());
    }

    #[test]
    fn a_newly_measured_mode_is_reported_not_gated() {
        let baseline = run(&[
            (
                SearchMode::Lexical,
                Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
            ),
            (SearchMode::Hybrid, None),
        ]);
        let current = run(&[
            (
                SearchMode::Lexical,
                Some(mode_score(SearchMode::Lexical, 1.0, 0, 0.0)),
            ),
            (
                SearchMode::Hybrid,
                Some(mode_score(SearchMode::Hybrid, 0.0, 0, 0.0)),
            ),
        ]);
        let outcome = compare(&current, &baseline).unwrap();
        assert!(!outcome.failed());
        assert!(outcome.notes.iter().any(|n| n.contains("hybrid")));
    }
}
