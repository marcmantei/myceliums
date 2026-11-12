//! Measure retrieval quality against the golden set.
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin retrieval-eval
//! cargo run -p myceliums-benchmarks --bin retrieval-eval -- --allow-model-download
//! cargo run -p myceliums-benchmarks --bin retrieval-eval -- --fetch-models
//! cargo run -p myceliums-benchmarks --bin retrieval-eval -- --check-baseline
//! MYC_EVAL_MODELS=1 cargo run -p myceliums-benchmarks --bin retrieval-eval -- --update-baseline
//! ```
//!
//! Prints a per-mode table, the per-intent breakdown, mode-vs-mode differences
//! with bootstrap confidence intervals, and the comparison against the recorded
//! baseline. Writes the full per-query detail to JSON.
//!
//! # Model weights
//!
//! The lexical mode needs nothing. Semantic, hybrid and hybrid+rerank run when
//! their weights are already in the fastembed cache (`FASTEMBED_CACHE_DIR`,
//! else `HF_HOME`, else `.fastembed_cache` in the working directory), or when
//! the run opts in to downloading them with `--allow-model-download` or
//! `MYC_EVAL_MODELS=1`. Otherwise they are reported `UNAVAILABLE` with the
//! reason; they never report a number they did not measure.
//!
//! # The gate
//!
//! `--check-baseline` exits non-zero when a gated metric regresses
//! significantly and materially against `golden/baseline.json`, or when the
//! comparison cannot be made (see `eval::gate` for the rule). Without it the
//! comparison is printed and never fails.
//!
//! # Regenerating the baseline
//!
//! `golden/baseline.json` is *generated output*, never hand-edited. Re-record
//! whenever a scoring, model or label change is *intended*, and commit the
//! regenerated file in the same change as its cause. `--update-baseline`
//! refuses to write a baseline in which any mode is unmeasured, so a baseline
//! can never silently stop covering a mode.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::eval::corpus::{fixtures_root, Corpus};
use myceliums_benchmarks::eval::evaluator::{evaluate, Metric, ModeScore, Retrieval, SearchMode};
use myceliums_benchmarks::eval::gate;
use myceliums_benchmarks::eval::golden_set::{GoldenSet, QueryIntent};
use myceliums_benchmarks::eval::models::{
    ModelIndex, ModelPolicy, ModelSetup, ALLOW_DOWNLOAD_FLAG, MODELS_ENV_VAR,
};
use myceliums_benchmarks::eval::report::{
    format_delta, markdown_comparisons, markdown_results, ModeReport, RunReport, MODE_COMPARISONS,
    REGENERATE_BASELINE,
};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Where the report lands unless `--out` says otherwise.
const DEFAULT_REPORT: &str = "golden/report.json";

/// The recorded reference scores, rewritten only on `--update-baseline`.
const BASELINE: &str = "golden/baseline.json";

/// Metrics shown in the mode-vs-mode table on stdout and in the summary.
const HEADLINE_METRICS: [Metric; 4] = [
    Metric::Mrr,
    Metric::NdcgAt10,
    Metric::RecallAt10,
    Metric::HitAt10,
];

/// How the binary was invoked.
struct Options {
    update_baseline: bool,
    check_baseline: bool,
    allow_model_download: bool,
    print_cache_key: bool,
    fetch_models: bool,
    report_path: PathBuf,
    summary_path: Option<PathBuf>,
}

impl Options {
    /// Parse the command line. `--help` prints usage and exits; any other
    /// unrecognised argument is an error rather than a silent no-op, so a typo
    /// cannot look like a successful run that ignored what you asked for.
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            update_baseline: false,
            check_baseline: false,
            allow_model_download: false,
            print_cache_key: false,
            fetch_models: false,
            report_path: crate_dir.join(DEFAULT_REPORT),
            summary_path: None,
        };
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--update-baseline" => options.update_baseline = true,
                "--check-baseline" => options.check_baseline = true,
                ALLOW_DOWNLOAD_FLAG => options.allow_model_download = true,
                "--model-cache-key" => options.print_cache_key = true,
                "--fetch-models" => options.fetch_models = true,
                "--out" => {
                    let path = args.next().context("--out requires a path")?;
                    options.report_path = PathBuf::from(path);
                }
                "--summary" => {
                    let path = args.next().context("--summary requires a path")?;
                    options.summary_path = Some(PathBuf::from(path));
                }
                "--help" | "-h" => {
                    println!(
                        "retrieval-eval [options]\n\n  \
                         {ALLOW_DOWNLOAD_FLAG}  download missing model weights (or set {MODELS_ENV_VAR}=1)\n  \
                         --check-baseline        exit non-zero on a material regression whose 95% CI is below zero\n  \
                         --update-baseline       rewrite golden/baseline.json (every mode must run)\n  \
                         --out <path>            write the JSON report here (default {DEFAULT_REPORT})\n  \
                         --summary <path>        append a Markdown summary (e.g. $GITHUB_STEP_SUMMARY)\n  \
                         --model-cache-key       print the CI cache key for the model weights and exit\n  \
                         --fetch-models          download the model weights and exit (an explicit opt-in)"
                    );
                    std::process::exit(0);
                }
                other => bail!("unknown argument '{other}' (try --help)"),
            }
        }
        Ok(options)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args(std::env::args())?;
    let setup = ModelSetup::shipped_default().context("resolving the shipped model setup")?;
    if options.print_cache_key {
        println!("{}", setup.cache_key());
        return Ok(());
    }
    if options.fetch_models {
        let dir = setup.fetch().await?;
        println!(
            "fetched {} and {} into {}",
            setup.embedding.model,
            setup.reranker_id,
            dir.display()
        );
        return Ok(());
    }
    let policy = ModelPolicy::from_flag_and_env(options.allow_model_download);

    let corpus = Corpus::load(&fixtures_root()?).context("loading the benchmark corpus")?;
    let golden = GoldenSet::embedded().context("loading the golden set")?;
    println!(
        "corpus: {} symbols   golden set: {} queries (dataset {})",
        corpus.len(),
        golden.len(),
        golden.dataset_version
    );

    // Build the embedding index only if some model-backed mode may run.
    let model_modes_runnable = SearchMode::all()
        .into_iter()
        .any(|mode| mode.needs_embeddings() && setup.blocker(mode, policy).is_none());
    let model_index = if model_modes_runnable {
        println!(
            "models: {} + {} (cache {})",
            setup.embedding.model,
            setup.reranker_id,
            setup.cache_dir().display()
        );
        Some(
            ModelIndex::build(&corpus, &setup, policy)
                .await
                .context("building the embedding index for the model-backed modes")?,
        )
    } else {
        None
    };
    let retrieval = match &model_index {
        Some(index) => Retrieval::with_models(index),
        None => Retrieval::lexical_only(&corpus).await?,
    };
    // BM25 and RRF break ties in store order; say whether it is the parse order.
    let same_order = retrieval
        .symbols()
        .iter()
        .map(|s| &s.uid)
        .eq(corpus.symbols().iter().map(|s| &s.uid));
    println!(
        "symbol order: store.get_symbols() ({})",
        if same_order {
            "same as parse order"
        } else {
            "differs from parse order"
        }
    );

    let mut results = BTreeMap::new();
    let mut scores: Vec<ModeScore> = Vec::new();
    for mode in SearchMode::all() {
        let blocker = if mode.needs_embeddings() {
            setup.blocker(mode, policy)
        } else {
            None
        };
        let report = match blocker {
            Some(reason) => ModeReport::unavailable(reason),
            None => {
                let score = evaluate(mode, &retrieval, &golden)
                    .await
                    .with_context(|| format!("evaluating mode '{}'", mode.id()))?;
                let report = ModeReport::measured(&score);
                scores.push(score);
                report
            }
        };
        results.insert(mode.id().to_string(), report);
    }

    let models = model_index.as_ref().map(|_| setup.provenance());
    let report = RunReport::new(&golden, corpus.len(), models, results)?;

    println!();
    print_results(&report);
    print_by_intent(&scores);
    print_comparisons(&report);
    print_per_query(&scores, &golden);

    report.write(&options.report_path)?;
    println!("\nreport written to {}", options.report_path.display());

    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE);
    let mut summary = format!(
        "## Retrieval quality\n\nDataset `{}` · {} queries · {} symbols · {}\n\n{}\n",
        report.dataset_version,
        report.queries,
        report.corpus_symbols,
        report.platform,
        markdown_results(&report)
    );
    let comparisons = markdown_comparisons(&report, &HEADLINE_METRICS);
    if !comparisons.is_empty() {
        summary.push_str(&format!(
            "\n### Mode vs mode (paired bootstrap, 95% CI)\n\nEach interval is uncorrected \
             for multiple comparisons (about 45 are computed per run).\n\n{comparisons}"
        ));
    }

    let gate_failed = match RunReport::read(&baseline_path) {
        Ok(baseline) => {
            let outcome = gate::compare(&report, &baseline)?;
            let rendered = gate::markdown(&outcome);
            println!("\n{rendered}");
            summary.push('\n');
            summary.push_str(&rendered);
            outcome.failed()
        }
        Err(error) => {
            let message = format!("baseline unreadable: {error:#}");
            println!("\n{message}");
            summary.push_str(&format!("\n> **error:** {message}\n"));
            // Without a readable baseline the gate cannot vouch for anything.
            true
        }
    };

    if let Some(path) = &options.summary_path {
        append(path, &summary)?;
    }

    if options.update_baseline {
        let unmeasured: Vec<&str> = SearchMode::all()
            .into_iter()
            .filter(|mode| report.measured(mode.id()).is_none())
            .map(|mode| mode.id())
            .collect();
        if !unmeasured.is_empty() {
            bail!(
                "refusing to record a baseline without {} — a baseline that drops a mode \
                 stops the gate from covering it. Re-run with {ALLOW_DOWNLOAD_FLAG} or \
                 {MODELS_ENV_VAR}=1.",
                unmeasured.join(", ")
            );
        }
        let mut baseline = report.clone();
        baseline.regenerate_with = Some(REGENERATE_BASELINE.to_string());
        baseline.timestamp = Some(build_timestamp());
        baseline.commit_sha = Some(commit_sha());
        baseline.write(&baseline_path)?;
        println!("baseline updated at {}", baseline_path.display());
    } else if options.check_baseline && gate_failed {
        bail!("retrieval-quality gate failed (see the comparison above)");
    }

    Ok(())
}

/// Print the aggregate table: one row per mode, unavailable modes included.
fn print_results(report: &RunReport) {
    print!("{:<14}", "mode");
    for metric in Metric::ALL {
        print!(" {:>9}", metric.label());
    }
    println!();
    println!("{}", "-".repeat(14 + 10 * Metric::ALL.len()));
    let mut footnotes = Vec::new();
    for mode in SearchMode::all() {
        match report.measured(mode.id()).and_then(|m| m.metrics.as_ref()) {
            Some(metrics) => {
                print!("{:<14}", mode.id());
                for metric in Metric::ALL {
                    print!(" {:>9.4}", metrics.get(metric));
                }
                println!();
            }
            None => {
                println!("{:<14} UNAVAILABLE [*]", mode.id());
                let reason = report
                    .results
                    .get(mode.id())
                    .and_then(|m| m.reason.clone())
                    .unwrap_or_default();
                footnotes.push(format!("    {}: {reason}", mode.id()));
            }
        }
    }
    if !footnotes.is_empty() {
        println!("\n[*] not measured — no numbers are reported for these modes:");
        for note in footnotes {
            println!("{note}");
        }
    }
}

/// Print MRR and nDCG@10 per intent for every measured mode.
fn print_by_intent(scores: &[ModeScore]) {
    if scores.is_empty() {
        return;
    }
    println!("\nby intent — MRR@10 / nDCG@10");
    print!("{:<14}", "intent");
    for score in scores {
        print!(" {:>17}", score.mode.id());
    }
    println!();
    for intent in QueryIntent::ALL {
        print!("{:<14}", intent.label());
        for score in scores {
            match score.by_intent.get(&intent) {
                Some(s) => print!(" {:>8.4} / {:.4}", s.mean.mrr, s.mean.ndcg_at_10),
                None => print!(" {:>17}", "—"),
            }
        }
        println!();
    }
}

/// Print the mode-vs-mode differences for the headline metrics.
fn print_comparisons(report: &RunReport) {
    if report.comparisons.is_empty() {
        return;
    }
    println!("\nmode vs mode — Δ [95% paired bootstrap CI, uncorrected for multiple comparisons]");
    for (treatment, control) in MODE_COMPARISONS {
        let key = format!("{} vs {}", treatment.id(), control.id());
        let Some(by_metric) = report.comparisons.get(&key) else {
            continue;
        };
        println!("  {key}");
        for metric in HEADLINE_METRICS {
            if let Some(estimate) = by_metric.get(metric.id()) {
                println!("    {:<10} {}", metric.label(), format_delta(estimate));
            }
        }
    }
}

/// Print reciprocal rank per query for every measured mode side by side, so a
/// query one mode finds and another misses is visible at a glance.
fn print_per_query(scores: &[ModeScore], golden: &GoldenSet) {
    if scores.is_empty() {
        return;
    }
    let id_width = golden
        .queries
        .iter()
        .map(|q| q.id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    println!("\nper-query reciprocal rank (golden-set order)");
    print!("{:<id_width$}", "query");
    for score in scores {
        print!(" {:>13}", score.mode.id());
    }
    println!();
    for (index, query) in golden.queries.iter().enumerate() {
        print!("{:<id_width$}", query.id);
        for score in scores {
            print!(" {:>13.4}", score.queries[index].values.mrr);
        }
        println!();
    }
}

/// Append `text` to `path`, creating it if needed.
fn append(path: &Path, text: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// Commit the baseline was recorded at, or `"unknown"` outside a git checkout.
///
/// Provenance only — the scores themselves never depend on it, and the
/// content digest, not this sha, is what the tests hold the baseline to. A
/// dirty tree gets a `-dirty` suffix: the numbers were produced from
/// uncommitted code that nobody else can check out.
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
