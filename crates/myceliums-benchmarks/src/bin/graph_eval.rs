//! Measure symbol and CALLS-edge accuracy against the hand-labelled gold set.
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin graph-eval
//! cargo run -p myceliums-benchmarks --bin graph-eval -- --check-baseline --summary "$GITHUB_STEP_SUMMARY"
//! cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline
//! ```
//!
//! Prints per-language and per-resolution-label tables (with 95% Wilson
//! intervals), the comparison with `graph-gold/baseline.json`, and every
//! false positive and negative; writes the full detail to
//! `graph-gold/report.json`.
//!
//! Without `--check-baseline` the run is a report: the exit status is
//! non-zero only when the evaluation itself cannot run. With it, the run
//! also fails when CALLS precision or recall regresses against the baseline
//! (see `myceliums_benchmarks::graph_eval::gate` for the rule) or a CALLS edge
//! connects two languages.
//!
//! `graph-gold/baseline.json` is generated output. Re-record it with
//! `--update-baseline` in the same change as an intended label, fixture or
//! extractor change.

use anyhow::{Context, Result};
use myceliums_benchmarks::eval::corpus::fixtures_root;
use myceliums_benchmarks::graph_eval::gate;
use myceliums_benchmarks::graph_eval::gold::GoldSet;
use myceliums_benchmarks::graph_eval::report::{
    markdown, markdown_comparison, RunReport, REGENERATE_BASELINE,
};
use myceliums_benchmarks::graph_eval::run;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const DEFAULT_REPORT: &str = "graph-gold/report.json";
const BASELINE: &str = "graph-gold/baseline.json";

struct Options {
    update_baseline: bool,
    check_baseline: bool,
    report_path: PathBuf,
    summary_path: Option<PathBuf>,
}

impl Options {
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            update_baseline: false,
            check_baseline: false,
            report_path: crate_dir.join(DEFAULT_REPORT),
            summary_path: None,
        };
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--update-baseline" => options.update_baseline = true,
                "--check-baseline" => options.check_baseline = true,
                "--out" => {
                    options.report_path =
                        PathBuf::from(args.next().context("--out requires a path")?)
                }
                "--summary" => {
                    options.summary_path = Some(PathBuf::from(
                        args.next().context("--summary requires a path")?,
                    ))
                }
                "--help" | "-h" => {
                    println!(
                        "graph-eval [options]\n\n  \
                         --check-baseline    fail on a regression against {BASELINE}\n  \
                         --update-baseline   rewrite {BASELINE}\n  \
                         --out <path>        write the JSON report here (default {DEFAULT_REPORT})\n  \
                         --summary <path>    append the Markdown report (e.g. $GITHUB_STEP_SUMMARY)"
                    );
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument '{other}' (see --help)"),
            }
        }
        Ok(options)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // The analyzer logs every file it parses; keep stdout for the report.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .with_writer(std::io::stderr)
        .init();

    let options = Options::from_args(std::env::args())?;
    let gold = GoldSet::embedded()?;
    let report = run(&gold, &fixtures_root()?).await?;

    let mut text = markdown(&report);
    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE);
    let mut failed = false;
    match RunReport::read(&baseline_path) {
        Ok(baseline) => {
            text.push_str(&markdown_comparison(&report, &baseline));
            if options.check_baseline {
                let outcome = gate::compare(&report, &baseline, &gold)?;
                text.push('\n');
                text.push_str(&gate::markdown(&outcome));
                failed = outcome.failed();
            }
        }
        Err(error) => {
            text.push_str(&format!("\nNo baseline to compare with: {error:#}\n"));
            failed = options.check_baseline;
        }
    }
    println!("{text}");

    report.write(&options.report_path)?;
    eprintln!("report written to {}", options.report_path.display());

    if let Some(path) = &options.summary_path {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(text.as_bytes())?;
    }

    if options.update_baseline {
        let mut baseline = report.clone();
        baseline.regenerate_with = Some(REGENERATE_BASELINE.to_string());
        baseline.timestamp =
            Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        baseline.commit_sha = Some(commit_sha());
        baseline.write(&baseline_path)?;
        eprintln!("baseline written to {}", baseline_path.display());
    }
    if failed {
        anyhow::bail!("graph accuracy regressed against {BASELINE} (see the gate table above)");
    }
    Ok(())
}

/// HEAD at recording time, or `"unknown"` outside a git checkout. Provenance
/// only: the content digest, not this sha, is what the tests hold the
/// baseline to.
fn commit_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
