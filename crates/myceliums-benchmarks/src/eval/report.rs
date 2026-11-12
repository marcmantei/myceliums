//! The report and baseline file format, and the mode-vs-mode comparison.
//!
//! One shape serves both files: `golden/report.json` (every run) and
//! `golden/baseline.json` (the committed reference, which adds a provenance
//! header). Per-query values are kept in both, because a paired comparison
//! needs the scores of each query, not just the means.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use super::bootstrap::{
    paired_bootstrap_default, DeltaEstimate, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, CONFIDENCE_LEVEL,
};
use super::evaluator::{Metric, MetricValues, ModeScore, SearchMode};
use super::golden_set::GoldenSet;
use super::models::ModelProvenance;

/// Layout version of the report and baseline files. `2.x` adds per-query
/// values, per-intent means, the extra metrics and model provenance.
pub const REPORT_SCHEMA_VERSION: &str = "2.0";

/// The command that regenerates the baseline, recorded inside it.
pub const REGENERATE_BASELINE: &str = "MYC_EVAL_MODELS=1 cargo run -p myceliums-benchmarks \
     --bin retrieval-eval -- --update-baseline";

/// Decimal places kept in reported scores.
///
/// Four is enough to see a real ranking change and few enough that float noise
/// in the last bits does not surface as a spurious diff between two runs of the
/// same code.
const REPORTED_DECIMALS: i32 = 4;

/// Round to [`REPORTED_DECIMALS`] so the report is diffable and free of float noise.
///
/// A tiny negative difference rounds to `-0.0`, which serialises as `-0.0` and
/// prints as `-0.0000` — a sign with no magnitude behind it. It is folded to
/// `0.0`.
pub fn round(value: f64) -> f64 {
    let scale = 10_f64.powi(REPORTED_DECIMALS);
    let rounded = (value * scale).round() / scale;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// The `status` values a mode report carries.
pub mod status {
    /// The mode ran and the numbers are real.
    pub const MEASURED: &str = "MEASURED";
    /// The mode did not run; there are no numbers, not zeroes.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
}

/// Mode pairs compared within one run: each model-backed mode against the
/// lexical baseline, fusion against its own semantic leg (does adding BM25
/// help?), and reranking against the hybrid ranking it reorders.
pub const MODE_COMPARISONS: [(SearchMode, SearchMode); 5] = [
    (SearchMode::Semantic, SearchMode::Lexical),
    (SearchMode::Hybrid, SearchMode::Lexical),
    (SearchMode::HybridRerank, SearchMode::Lexical),
    (SearchMode::Hybrid, SearchMode::Semantic),
    (SearchMode::HybridRerank, SearchMode::Hybrid),
];

/// How the confidence intervals in a report were computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub method: String,
    pub resamples: usize,
    pub seed: u64,
    pub confidence: f64,
}

impl BootstrapConfig {
    fn current() -> Self {
        Self {
            method: "paired bootstrap over queries, percentile interval".to_string(),
            resamples: BOOTSTRAP_RESAMPLES,
            seed: BOOTSTRAP_SEED,
            confidence: CONFIDENCE_LEVEL,
        }
    }
}

/// Means over the queries of one intent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentReport {
    pub queries: usize,
    pub metrics: MetricValues,
}

/// One mode's scores.
///
/// A measured mode carries `metrics`, `by_intent` and `per_query`; a mode that
/// did not run carries `status: "UNAVAILABLE"`, a `reason`, and `null` for all
/// three — an absent measurement must never be mistaken for a zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeReport {
    /// [`status::MEASURED`] or [`status::UNAVAILABLE`].
    pub status: String,
    /// Why the mode did not run, and what would let it — `null` when measured.
    pub reason: Option<String>,
    /// Mean of each metric over all queries.
    pub metrics: Option<MetricValues>,
    /// Means per intent, keyed by intent label.
    #[serde(default)]
    pub by_intent: Option<BTreeMap<String, IntentReport>>,
    /// Every metric for every query, keyed by query id.
    #[serde(default)]
    pub per_query: Option<BTreeMap<String, MetricValues>>,
}

impl ModeReport {
    /// A mode that ran.
    pub fn measured(score: &ModeScore) -> Self {
        Self {
            status: status::MEASURED.to_string(),
            reason: None,
            metrics: Some(score.mean.map(round)),
            by_intent: Some(
                score
                    .by_intent
                    .iter()
                    .map(|(intent, intent_score)| {
                        (
                            intent.label().to_string(),
                            IntentReport {
                                queries: intent_score.queries,
                                metrics: intent_score.mean.map(round),
                            },
                        )
                    })
                    .collect(),
            ),
            per_query: Some(
                score
                    .queries
                    .iter()
                    .map(|query| (query.id.clone(), query.values.map(round)))
                    .collect(),
            ),
        }
    }

    /// A mode that did not run, and why.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: status::UNAVAILABLE.to_string(),
            reason: Some(reason.into()),
            metrics: None,
            by_intent: None,
            per_query: None,
        }
    }

    /// True when the mode ran.
    pub fn is_measured(&self) -> bool {
        self.status == status::MEASURED && self.metrics.is_some()
    }

    /// Per-query values of `metric` in the order of `query_ids`, or an error
    /// naming the first query this report has no score for.
    pub fn aligned(&self, metric: Metric, query_ids: &[String]) -> Result<Vec<f64>> {
        let rows = self
            .per_query
            .as_ref()
            .context("the mode has no per-query scores")?;
        query_ids
            .iter()
            .map(|id| {
                rows.get(id)
                    .map(|values| values.get(metric))
                    .with_context(|| format!("no score for query '{id}'"))
            })
            .collect()
    }
}

/// A full evaluation run, as written to `report.json` and `baseline.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub schema_version: String,
    /// How to regenerate this file — baseline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regenerate_with: Option<String>,
    /// When the baseline was recorded — baseline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Commit the baseline was measured against — baseline only, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Content digest of the golden set the scores belong to.
    pub dataset_digest: String,
    /// Hand-maintained golden-set version.
    pub dataset_version: String,
    pub corpus_symbols: usize,
    pub queries: usize,
    /// Model configuration of the model-backed modes; `null` when none ran.
    #[serde(default)]
    pub models: Option<ModelProvenance>,
    /// Platform the numbers were produced on. Model inference is reproducible
    /// on one platform, not guaranteed bit-identical across them.
    pub platform: String,
    pub bootstrap: BootstrapConfig,
    /// One entry per mode, keyed by mode id.
    pub results: BTreeMap<String, ModeReport>,
    /// Mode-vs-mode differences with confidence intervals, keyed
    /// `"<treatment> vs <control>"`, then by metric id.
    #[serde(default)]
    pub comparisons: BTreeMap<String, BTreeMap<String, DeltaEstimate>>,
}

impl RunReport {
    /// Assemble the report of one run. `results` must hold every mode.
    pub fn new(
        golden: &GoldenSet,
        corpus_symbols: usize,
        models: Option<ModelProvenance>,
        results: BTreeMap<String, ModeReport>,
    ) -> Result<Self> {
        let mut report = Self {
            schema_version: REPORT_SCHEMA_VERSION.to_string(),
            regenerate_with: None,
            timestamp: None,
            commit_sha: None,
            dataset_digest: GoldenSet::embedded_digest(),
            dataset_version: golden.dataset_version.clone(),
            corpus_symbols,
            queries: golden.len(),
            models,
            platform: platform(),
            bootstrap: BootstrapConfig::current(),
            results,
            comparisons: BTreeMap::new(),
        };
        let query_ids = report.query_ids();
        for (treatment, control) in MODE_COMPARISONS {
            let (Some(t), Some(c)) = (
                report.measured(treatment.id()),
                report.measured(control.id()),
            ) else {
                continue;
            };
            let mut by_metric = BTreeMap::new();
            for metric in Metric::ALL {
                let estimate = paired_bootstrap_default(
                    &t.aligned(metric, &query_ids)?,
                    &c.aligned(metric, &query_ids)?,
                )?;
                by_metric.insert(metric.id().to_string(), rounded(estimate));
            }
            report
                .comparisons
                .insert(comparison_key(treatment.id(), control.id()), by_metric);
        }
        Ok(report)
    }

    /// The measured report of one mode, if it ran.
    pub fn measured(&self, mode_id: &str) -> Option<&ModeReport> {
        self.results.get(mode_id).filter(|mode| mode.is_measured())
    }

    /// Query ids this report has per-query scores for, in sorted order.
    ///
    /// This is the one resampling order: the mode-vs-mode intervals here and
    /// the current-vs-baseline intervals in the gate both pair queries in it,
    /// so the same per-query data always yields the same interval.
    pub fn query_ids(&self) -> Vec<String> {
        self.results
            .values()
            .find_map(|mode| mode.per_query.as_ref())
            .map(|rows| rows.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Read a report or baseline file.
    pub fn read(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let report: Self = serde_json::from_str(&json).with_context(|| {
            format!(
                "{} does not match the report schema {REPORT_SCHEMA_VERSION} — it is \
                 generated output; re-record with: {REGENERATE_BASELINE}",
                path.display()
            )
        })?;
        if !report.schema_version.starts_with("2.") {
            bail!(
                "{} has schema {} but this harness reads {REPORT_SCHEMA_VERSION}; re-record \
                 with: {REGENERATE_BASELINE}",
                path.display(),
                report.schema_version
            );
        }
        Ok(report)
    }

    /// Write pretty-printed JSON with a trailing newline, so the file is a
    /// clean diff.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }
}

/// Key of a mode-vs-mode comparison.
pub fn comparison_key(treatment: &str, control: &str) -> String {
    format!("{treatment} vs {control}")
}

/// An estimate rounded like every other reported number.
pub fn rounded(estimate: DeltaEstimate) -> DeltaEstimate {
    DeltaEstimate {
        delta: round(estimate.delta),
        ci_low: round(estimate.ci_low),
        ci_high: round(estimate.ci_high),
    }
}

/// `os-arch` of this build, e.g. `macos-aarch64`.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Format a delta and its interval as `+0.0123 [-0.0100, +0.0400]`.
pub fn format_delta(estimate: &DeltaEstimate) -> String {
    format!(
        "{:+.4} [{:+.4}, {:+.4}]",
        estimate.delta, estimate.ci_low, estimate.ci_high
    )
}

/// Markdown table of mean metrics per mode, unavailable modes included.
pub fn markdown_results(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str("| Mode |");
    for metric in Metric::ALL {
        out.push_str(&format!(" {} |", metric.label()));
    }
    out.push_str("\n| --- |");
    for _ in Metric::ALL {
        out.push_str(" ---: |");
    }
    out.push('\n');
    for mode in SearchMode::all() {
        match report.measured(mode.id()).and_then(|m| m.metrics.as_ref()) {
            Some(metrics) => {
                out.push_str(&format!("| `{}` |", mode.id()));
                for metric in Metric::ALL {
                    out.push_str(&format!(" {:.4} |", metrics.get(metric)));
                }
                out.push('\n');
            }
            None => {
                let reason = report
                    .results
                    .get(mode.id())
                    .and_then(|m| m.reason.as_deref())
                    .unwrap_or("not run");
                out.push_str(&format!("| `{}` | UNAVAILABLE — {reason} |", mode.id()));
                for _ in 1..Metric::ALL.len() {
                    out.push_str(" |");
                }
                out.push('\n');
            }
        }
    }
    out
}

/// Markdown table of the mode-vs-mode comparisons for a few headline metrics.
pub fn markdown_comparisons(report: &RunReport, metrics: &[Metric]) -> String {
    if report.comparisons.is_empty() {
        return String::new();
    }
    let mut out = String::from("| Comparison |");
    for metric in metrics {
        out.push_str(&format!(" Δ {} [95% CI] |", metric.label()));
    }
    out.push_str("\n| --- |");
    for _ in metrics {
        out.push_str(" ---: |");
    }
    out.push('\n');
    for (treatment, control) in MODE_COMPARISONS {
        let key = comparison_key(treatment.id(), control.id());
        let Some(by_metric) = report.comparisons.get(&key) else {
            continue;
        };
        out.push_str(&format!("| `{key}` |"));
        for metric in metrics {
            match by_metric.get(metric.id()) {
                Some(estimate) => out.push_str(&format!(" {} |", format_delta(estimate))),
                None => out.push_str(" — |"),
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluator::{IntentScore, QueryScore};
    use crate::eval::golden_set::QueryIntent;

    fn score(mode: SearchMode, golden: &GoldenSet, value: f64) -> ModeScore {
        let values = MetricValues::from_fn(|_| value);
        ModeScore {
            mode,
            mean: values,
            by_intent: [(
                QueryIntent::ExactName,
                IntentScore {
                    queries: golden.len(),
                    mean: values,
                },
            )]
            .into_iter()
            .collect(),
            queries: golden
                .queries
                .iter()
                .map(|q| QueryScore {
                    id: q.id.clone(),
                    intent: q.intent,
                    values,
                })
                .collect(),
        }
    }

    fn report_with(lexical: f64, hybrid: Option<f64>) -> RunReport {
        let golden = GoldenSet::embedded().unwrap();
        let mut results = BTreeMap::new();
        results.insert(
            "lexical".to_string(),
            ModeReport::measured(&score(SearchMode::Lexical, &golden, lexical)),
        );
        results.insert(
            "hybrid".to_string(),
            match hybrid {
                Some(value) => ModeReport::measured(&score(SearchMode::Hybrid, &golden, value)),
                None => ModeReport::unavailable("no weights"),
            },
        );
        RunReport::new(&golden, 96, None, results).unwrap()
    }

    #[test]
    fn measured_modes_are_compared_and_unmeasured_ones_are_not() {
        let report = report_with(0.5, Some(0.75));
        let comparison = &report.comparisons[&comparison_key("hybrid", "lexical")];
        assert_eq!(comparison["mrr"].delta, 0.25);
        assert_eq!(comparison.len(), Metric::ALL.len());

        let report = report_with(0.5, None);
        assert!(report.comparisons.is_empty());
        assert!(report.measured("hybrid").is_none());
    }

    #[test]
    fn an_unavailable_mode_carries_no_numbers() {
        let mode = ModeReport::unavailable("why");
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["status"], status::UNAVAILABLE);
        assert!(json["metrics"].is_null());
        assert!(json["per_query"].is_null());
        assert!(!mode.is_measured());
    }

    #[test]
    fn a_report_round_trips_through_json() {
        let report = report_with(0.5, Some(0.75));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        report.write(&path).unwrap();
        assert_eq!(RunReport::read(&path).unwrap(), report);
    }

    #[test]
    fn a_report_of_an_older_schema_is_refused_with_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baseline.json");
        std::fs::write(&path, r#"{"schema_version":"1.0","results":{}}"#).unwrap();
        let error = RunReport::read(&path).unwrap_err();
        assert!(
            format!("{error:#}").contains("--update-baseline"),
            "{error:#}"
        );
    }

    #[test]
    fn per_query_values_align_by_id_not_position() {
        let report = report_with(0.5, Some(0.75));
        let mut ids = report.query_ids();
        ids.reverse();
        let aligned = report.results["hybrid"].aligned(Metric::Mrr, &ids).unwrap();
        assert!(aligned.iter().all(|v| *v == 0.75));
        assert!(report.results["hybrid"]
            .aligned(Metric::Mrr, &["no-such-query".to_string()])
            .is_err());
    }

    #[test]
    fn rounding_keeps_four_decimals() {
        assert_eq!(round(1.0 / 3.0), 0.3333);
        assert_eq!(round(0.66666), 0.6667);
    }

    #[test]
    fn rounding_never_produces_negative_zero() {
        for value in [-0.0, -0.00001, -0.0000499] {
            let r = round(value);
            assert!(r == 0.0 && r.is_sign_positive(), "{value} rounded to {r:?}");
            assert_eq!(serde_json::to_string(&r).unwrap(), "0.0");
        }
        assert_eq!(round(-0.00005), -0.0001);
    }

    #[test]
    fn report_and_gate_resample_in_the_same_order() {
        // The same per-query data must give the same interval whether the
        // report computes it (mode vs mode) or the gate does (current vs
        // baseline): both pair queries in `query_ids()` order.
        let report = report_with(0.5, Some(0.75));
        let ids = report.query_ids();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        let lexical = report.results["lexical"]
            .aligned(Metric::Mrr, &ids)
            .unwrap();
        let hybrid = report.results["hybrid"].aligned(Metric::Mrr, &ids).unwrap();
        let direct = rounded(paired_bootstrap_default(&hybrid, &lexical).unwrap());
        assert_eq!(
            report.comparisons[&comparison_key("hybrid", "lexical")]["mrr"],
            direct
        );
    }
}
