//! Measure how much recall the IVF-PQ vector index costs against exact search.
//!
//! ```text
//! cargo run -p myceliums-benchmarks --release --bin ann-recall
//! ```
//!
//! Builds a synthetic table of `ANN_INDEX_THRESHOLD` (5 000) unit vectors in a
//! scratch LanceDB store — the size at which `myc analyze` switches a
//! repository from an exact flat scan to IVF-PQ — and reports recall@10 of the
//! indexed search against brute-force cosine, for a clustered and a uniform
//! distribution. See `myceliums_benchmarks::ann` for the procedure and
//! `benchmarks/METHODOLOGY.md` for the recorded result.

use anyhow::Result;
use myceliums_benchmarks::ann::{measure, AnnConfig, Distribution};

#[tokio::main]
async fn main() -> Result<()> {
    println!(
        "{:<10} {:>6} {:>8} {:>11} {:>11} {:>20} {:>11} {:>10}",
        "data",
        "rows",
        "queries",
        "flat@10",
        "ANN@10",
        "ANN@10 95% CI",
        "top-1 agree",
        "partitions"
    );
    for distribution in [Distribution::Clustered, Distribution::Uniform] {
        let result = measure(&AnnConfig::at_threshold(distribution)).await?;
        println!(
            "{:<10} {:>6} {:>8} {:>11.4} {:>11.4} {:>20} {:>11.4} {:>10}",
            distribution.label(),
            result.rows,
            result.queries,
            result.flat_recall,
            result.ann_recall,
            format!(
                "[{:.4}, {:.4}]",
                result.ann_recall_ci.0, result.ann_recall_ci.1
            ),
            result.ann_top1_agreement,
            result.partitions,
        );
    }
    Ok(())
}
