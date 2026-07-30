//! Measure retrieval quality against the golden set.
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin retrieval-eval
//! cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
//! ```
//!
//! Prints a per-mode table to stdout and writes the full per-query detail to
//! JSON. With `--update-baseline` it also rewrites `golden/baseline.json`, which
//! is the recorded starting point future changes are compared against.
//!
//! # Regenerating the baseline
//!
//! `golden/baseline.json` is *generated output*, never hand-edited. Its
//! `commit_sha` and `timestamp` are stamped by this binary at the moment of the
//! run, so the only supported way to change them is to re-run it:
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
//! ```
//!
//! Re-record whenever a scoring change or a label change is *intended* — that
//! is the moment the numbers legitimately move. Commit the regenerated file in
//! the same change as the code or labels that moved it, so a reviewer sees the
//! cause and the effect in one diff. A `-dirty` suffix on `commit_sha` means the
//! run measured uncommitted code; re-record from a clean tree before committing.
//!
//! Offline and deterministic by construction: the corpus is parsed from in-repo
//! fixtures, no model weights are downloaded, and the only non-reproducible
//! fields (timestamp, commit) are confined to the baseline's provenance header.

use anyhow::{Context, Result};
use myceliums_benchmarks::eval::corpus::{fixtures_root, Corpus};
use myceliums_benchmarks::eval::evaluator::{
    evaluate, ModeScore, OfflineBlocker, SearchMode, RECALL_CUTOFFS,
};
use myceliums_benchmarks::eval::golden_set::GoldenSet;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Where the report lands unless `--out` says otherwise.
const DEFAULT_REPORT: &str = "golden/report.json";

/// The recorded reference scores, rewritten only on `--update-baseline`.
const BASELINE: &str = "golden/baseline.json";

/// Column widths of the aggregate table, in print order. The separator rule is
/// derived from these so a column change cannot leave a ragged underline.
const TABLE_COLUMNS: [usize; 5] = [14, 10, 10, 11, 8];

/// One space between each pair of columns.
const COLUMN_GAP: usize = 1;

/// Width of the aggregate table's separator rule.
fn table_width() -> usize {
    TABLE_COLUMNS.iter().sum::<usize>() + COLUMN_GAP * (TABLE_COLUMNS.len() - 1)
}

/// How the binary was invoked.
struct Options {
    /// Rewrite `golden/baseline.json` from this run.
    update_baseline: bool,
    /// Where to write the full JSON report.
    report_path: PathBuf,
}

impl Options {
    /// Parse `--update-baseline` and `--out <path>`.
    ///
    /// `--help` prints usage and exits successfully; any other unrecognised
    /// argument is an error rather than a silent no-op, so a typo cannot look
    /// like a successful run that quietly ignored what you asked for.
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            update_baseline: false,
            report_path: crate_dir.join(DEFAULT_REPORT),
        };
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--update-baseline" => options.update_baseline = true,
                "--out" => {
                    let path = args.next().context("--out requires a path")?;
                    options.report_path = PathBuf::from(path);
                }
                "--help" | "-h" => {
                    println!(
                        "retrieval-eval [--update-baseline] [--out <path>]\n\n  \
                         --update-baseline  rewrite golden/baseline.json from this run\n  \
                         --out <path>       write the JSON report here (default {DEFAULT_REPORT})"
                    );
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument '{other}' (try --help)"),
            }
        }
        Ok(options)
    }
}

/// The `status` values a mode report carries. Named once here so the writer and
/// the table reader below cannot drift apart on a string literal.
mod status {
    /// The mode ran and the numbers are real.
    pub const MEASURED: &str = "MEASURED";
    /// The mode did not run; there are no numbers, not zeroes.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
}

/// One mode's scores, as they appear in the report and the baseline.
///
/// Both variants carry the same three keys — `status`, `reason`, `metrics` — so
/// a consumer reads one shape regardless of outcome. A mode that cannot run
/// offline reports `status: "UNAVAILABLE"` with `metrics: null`: an absent
/// measurement must never be mistaken for a zero.
#[derive(Debug, Serialize)]
struct ModeReport {
    /// [`status::MEASURED`] or [`status::UNAVAILABLE`].
    status: &'static str,
    /// Why the mode did not run, and what would let it — `null` when measured.
    reason: Option<String>,
    /// The scores, or `null` when the mode did not run.
    metrics: Option<Metrics>,
}

/// The four numbers a measured mode reports.
#[derive(Debug, Serialize)]
struct Metrics {
    recall_at_1: f64,
    recall_at_5: f64,
    recall_at_10: f64,
    mrr: f64,
}

impl ModeReport {
    fn measured(score: &ModeScore) -> Self {
        Self {
            status: status::MEASURED,
            reason: None,
            metrics: Some(Metrics {
                recall_at_1: round(score.recall_at(1)),
                recall_at_5: round(score.recall_at(5)),
                recall_at_10: round(score.recall_at(10)),
                mrr: round(score.mrr),
            }),
        }
    }

    fn unavailable(blocker: OfflineBlocker) -> Self {
        Self {
            status: status::UNAVAILABLE,
            reason: Some(format!("{} — {}", blocker.reason, blocker.lifted_by)),
            metrics: None,
        }
    }
}

/// Decimal places kept in reported scores.
///
/// Four is enough to see a real ranking change and few enough that float noise
/// in the last bits does not surface as a spurious diff between two runs of the
/// same code.
const REPORTED_DECIMALS: i32 = 4;

/// Round to [`REPORTED_DECIMALS`] so the report is diffable and free of float noise.
fn round(value: f64) -> f64 {
    let scale = 10_f64.powi(REPORTED_DECIMALS);
    (value * scale).round() / scale
}

fn main() -> Result<()> {
    let options = Options::from_args(std::env::args())?;

    let corpus = Corpus::load(&fixtures_root()?).context("loading the benchmark corpus")?;
    let golden = GoldenSet::embedded().context("loading the golden set")?;

    println!(
        "corpus: {} symbols   golden set: {} queries (dataset {})\n",
        corpus.len(),
        golden.len(),
        golden.dataset_version
    );

    // Evaluate every mode the issue names, measured or not, so the report says
    // plainly which are unmeasured rather than quietly omitting them.
    let mut reports = serde_json::Map::new();
    let mut measured: Vec<ModeScore> = Vec::new();

    for mode in SearchMode::all() {
        let report = match mode.offline_blocker() {
            Some(blocker) => ModeReport::unavailable(blocker),
            None => {
                let score = evaluate(mode, &corpus, &golden)?;
                let report = ModeReport::measured(&score);
                measured.push(score);
                report
            }
        };
        reports.insert(mode.id().to_string(), serde_json::to_value(&report)?);
    }

    print_table(&reports);
    print_per_query(&measured);

    let report = json!({
        "schema_version": "1.0",
        "dataset_version": golden.dataset_version,
        "corpus_symbols": corpus.len(),
        "queries": golden.len(),
        "results": reports,
        "per_query": per_query_json(&measured),
    });
    write_json(&options.report_path, &report)?;
    println!("\nreport written to {}", options.report_path.display());

    if options.update_baseline {
        let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE);
        let baseline = json!({
            "regenerate_with": "cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline",
            "timestamp": build_timestamp(),
            "commit_sha": commit_sha(),
            "dataset_version": golden.dataset_version,
            "corpus_symbols": corpus.len(),
            "queries": golden.len(),
            "results": reports,
        });
        write_json(&baseline_path, &baseline)?;
        println!("baseline updated at {}", baseline_path.display());
    }

    Ok(())
}

/// Print the aggregate table: one row per mode, unavailable modes included.
///
/// Unavailable modes print a marker instead of numbers and their reason is
/// listed below the table, so the reader never has to guess whether a blank
/// cell means "zero" or "not run".
fn print_table(reports: &serde_json::Map<String, serde_json::Value>) {
    let [mode_w, r1_w, r5_w, r10_w, mrr_w] = TABLE_COLUMNS;
    println!(
        "{:<mode_w$} {:>r1_w$} {:>r5_w$} {:>r10_w$} {:>mrr_w$}",
        "mode", "recall@1", "recall@5", "recall@10", "MRR"
    );
    println!("{}", "-".repeat(table_width()));

    for (mode, value) in reports {
        let measured = value.get("status").and_then(|s| s.as_str()) == Some(status::MEASURED);
        match value.get("metrics").filter(|_| measured) {
            Some(metrics) => {
                let get = |key: &str| metrics.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
                println!(
                    "{:<mode_w$} {:>r1_w$.4} {:>r5_w$.4} {:>r10_w$.4} {:>mrr_w$.4}",
                    mode,
                    get("recall_at_1"),
                    get("recall_at_5"),
                    get("recall_at_10"),
                    get("mrr")
                );
            }
            // The marker refers to the reasons listed under the table.
            None => println!(
                "{mode:<mode_w$} {:>width$}",
                format!("{} [*]", status::UNAVAILABLE),
                width = table_width() - mode_w - COLUMN_GAP
            ),
        }
    }

    let mut footnoted = false;
    for (mode, value) in reports {
        if let Some(reason) = value.get("reason").and_then(|r| r.as_str()) {
            if !footnoted {
                println!("\n[*] not measured — no numbers are reported for these modes:");
                footnoted = true;
            }
            println!("    {mode}: {reason}");
        }
    }
}

/// Print per-query reciprocal rank for each measured mode, worst first, so the
/// queries that need attention are the ones you read.
fn print_per_query(scores: &[ModeScore]) {
    for score in scores {
        println!("\nper-query detail — {} (worst first)", score.mode.id());
        // Widened to the longest query id so the columns stay aligned as the
        // golden set grows; a ragged table hides the numbers it exists to show.
        let id_width = score
            .queries
            .iter()
            .map(|q| q.id.len())
            .max()
            .unwrap_or(5)
            .max(5);
        println!(
            "{:<id_width$} {:<13} {:>9} {:>10}",
            "query", "intent", "RR", "recall@10"
        );
        println!("{}", "-".repeat(id_width + 35));
        let mut queries = score.queries.clone();
        // Ties broken by id so the ordering — and therefore the output — is
        // identical on every run.
        queries.sort_by(|a, b| {
            a.reciprocal_rank
                .partial_cmp(&b.reciprocal_rank)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        for query in &queries {
            println!(
                "{:<id_width$} {:<13} {:>9.4} {:>10.4}",
                query.id,
                query.intent.label(),
                query.reciprocal_rank,
                query.recall.get(&10).copied().unwrap_or(0.0)
            );
        }
    }
}

/// Per-query scores for every measured mode, keyed by mode id.
fn per_query_json(scores: &[ModeScore]) -> serde_json::Value {
    let mut modes = serde_json::Map::new();
    for score in scores {
        let queries: Vec<serde_json::Value> = score
            .queries
            .iter()
            .map(|query| {
                let recall: serde_json::Map<String, serde_json::Value> = RECALL_CUTOFFS
                    .iter()
                    .map(|k| {
                        (
                            format!("recall_at_{k}"),
                            json!(round(query.recall.get(k).copied().unwrap_or(0.0))),
                        )
                    })
                    .collect();
                json!({
                    "id": query.id,
                    "intent": query.intent.label(),
                    "reciprocal_rank": round(query.reciprocal_rank),
                    "recall": recall,
                })
            })
            .collect();
        modes.insert(score.mode.id().to_string(), json!(queries));
    }
    json!(modes)
}

/// Serialize pretty-printed with a trailing newline, so the file is a clean diff.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut json = serde_json::to_string_pretty(value)?;
    json.push('\n');
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

/// Commit the baseline was recorded at, or `"unknown"` outside a git checkout.
///
/// Provenance only — the scores themselves never depend on it.
///
/// A dirty tree gets a `-dirty` suffix. Without it the sha implies a
/// reproducibility the file does not have: the numbers were produced from
/// uncommitted code that nobody else can check out. Note that re-recording a
/// baseline necessarily predates the commit that carries it, so this sha names
/// the tree the measurement ran against, not the commit the file lands in.
fn commit_sha() -> String {
    let Some(sha) = git(&["rev-parse", "HEAD"]) else {
        return "unknown".to_string();
    };
    match git(&["status", "--porcelain"]) {
        Some(changes) if !changes.is_empty() => format!("{sha}-dirty"),
        _ => sha,
    }
}

/// Run a git command in the crate directory, or `None` if git is unavailable.
fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_string())
}

/// When the baseline was recorded, as an RFC 3339 UTC timestamp.
fn build_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
