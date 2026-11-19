//! Measure how much recall an ANN vector index costs against exact search.
//!
//! ```text
//! # Production search (exact, no index) on the synthetic tables at --sizes
//! # (default 5000), next to the pre-#117 IVF-PQ index on the same table:
//! cargo run -p myceliums-benchmarks --release --bin ann-recall
//!
//! # The issue #117 sweep — every setting in `ann::sweep_settings`, synthetic
//! # clustered data, at several sizes:
//! cargo run -p myceliums-benchmarks --release --bin ann-recall -- --sweep --sizes 5000,20000,50000
//!
//! # The same sweep on real embeddings of a source tree (production parser and
//! # embedder, shipped default model; vectors cached in --cache):
//! cargo run -p myceliums-benchmarks --release --bin ann-recall -- --sweep \
//!     --repo /path/to/checkout --cache /path/to/vectors.bin --sizes 5000,20000,50000
//! ```
//!
//! `--json <file>` additionally writes every result as JSON lines;
//! `--only <setting>[,<setting>...]` restricts the sweep to settings by their
//! printed name (`--only none` measures the flat scan alone);
//! `--distribution uniform` sweeps the structureless synthetic table instead
//! of the clustered one. See
//! `myceliums_benchmarks::ann` for the procedure and `benchmarks/METHODOLOGY.md`
//! for the recorded results.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::ann::{
    describe, production_check, sweep_settings, AnnRecall, Bench, Distribution, Synthetic,
};
use myceliums_benchmarks::ann_real::load_or_embed;
use myceliums_benchmarks::eval::models::ModelSetup;
use std::io::Write;
use std::path::PathBuf;

/// Queries per table in the sweep.
const SWEEP_QUERIES: usize = 200;
/// Seed of the held-out query sample and table order for real data.
const REAL_SEED: u64 = 117;

struct Args {
    sweep: bool,
    sizes: Vec<usize>,
    repo: Option<PathBuf>,
    cache: Option<PathBuf>,
    json: Option<PathBuf>,
    only: Option<String>,
    distribution: Distribution,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        sweep: false,
        sizes: vec![5_000],
        repo: None,
        cache: None,
        json: None,
        only: None,
        distribution: Distribution::Clustered,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--sweep" => args.sweep = true,
            "--sizes" => {
                args.sizes = value()?
                    .split(',')
                    .map(|n| {
                        n.trim()
                            .parse()
                            .context("--sizes takes comma-separated row counts")
                    })
                    .collect::<Result<_>>()?
            }
            "--repo" => args.repo = Some(value()?.into()),
            "--cache" => args.cache = Some(value()?.into()),
            "--json" => args.json = Some(value()?.into()),
            "--only" => args.only = Some(value()?),
            "--distribution" => {
                args.distribution = match value()?.as_str() {
                    "clustered" => Distribution::Clustered,
                    "uniform" => Distribution::Uniform,
                    other => bail!("--distribution is clustered or uniform, not {other}"),
                }
            }
            other => bail!("unknown argument {other}"),
        }
    }
    if args.repo.is_some() != args.cache.is_some() {
        bail!("--repo and --cache go together");
    }
    Ok(args)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let mut json = match &args.json {
        Some(path) => Some(std::fs::File::create(path)?),
        None => None,
    };
    print_header();
    if !args.sweep {
        for &rows in &args.sizes {
            for distribution in [Distribution::Clustered, Distribution::Uniform] {
                let check = production_check(&Synthetic::standard(distribution, rows)).await?;
                report(&check.stale_index, json.as_mut())?;
                report(&check.production, json.as_mut())?;
            }
        }
        return Ok(());
    }

    let real = match (&args.repo, &args.cache) {
        (Some(repo), Some(cache)) => {
            let setup = ModelSetup::shipped_default()?;
            let pool_size = args.sizes.iter().copied().max().unwrap_or(0);
            Some(
                load_or_embed(
                    repo,
                    cache,
                    SWEEP_QUERIES,
                    pool_size,
                    REAL_SEED,
                    &setup.embedding,
                )
                .await?,
            )
        }
        _ => None,
    };
    for &rows in &args.sizes {
        let data = match &real {
            Some(real) => real.table(rows)?,
            None => Synthetic {
                queries: SWEEP_QUERIES,
                ..Synthetic::standard(args.distribution, rows)
            }
            .generate(),
        };
        let bench = Bench::load(data, 10).await?;
        let selected = sweep_settings().into_iter().filter(|settings| {
            args.only
                .as_ref()
                .is_none_or(|only| only.split(',').any(|o| describe(settings) == o.trim()))
        });
        for settings in selected {
            match bench.measure(settings).await {
                Ok(result) => report(&result, json.as_mut())?,
                Err(e) => eprintln!("{rows} rows, {}: {e:#}", describe(&settings)),
            }
        }
        eprintln!(
            "{rows} rows: exact (production) search p50/p95 {:.2}/{:.2} ms",
            bench.flat_latency_ms().0,
            bench.flat_latency_ms().1,
        );
    }
    Ok(())
}

fn print_header() {
    println!(
        "{:<16} {:>6} {:<34} {:>5} {:>7} {:>17} {:>6} {:>9} {:>15} {:>15}",
        "data",
        "rows",
        "setting",
        "parts",
        "ANN@10",
        "95% CI",
        "top-1",
        "build ms",
        "ANN p50/p95 ms",
        "flat p50/p95 ms"
    );
}

fn report(result: &AnnRecall, json: Option<&mut std::fs::File>) -> Result<()> {
    println!(
        "{:<16} {:>6} {:<34} {:>5} {:>7.4} {:>17} {:>6.3} {:>9.0} {:>15} {:>15}",
        result.data,
        result.rows,
        result.setting,
        result.partitions,
        result.ann_recall,
        format!(
            "[{:.4}, {:.4}]",
            result.ann_recall_ci.0, result.ann_recall_ci.1
        ),
        result.ann_top1_agreement,
        result.build_ms,
        format!(
            "{:.2}/{:.2}",
            result.ann_latency_ms.0, result.ann_latency_ms.1
        ),
        format!(
            "{:.2}/{:.2}",
            result.flat_latency_ms.0, result.flat_latency_ms.1
        ),
    );
    if result.flat_recall < 1.0 {
        bail!("flat recall {} — harness error", result.flat_recall);
    }
    if let Some(file) = json {
        writeln!(file, "{}", serde_json::to_string(result)?)?;
    }
    Ok(())
}
