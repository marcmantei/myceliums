//! Manual latency check for the cached read path (#93). Ignored by default.
//!
//! Indexes a project (embeddings off) into a throwaway data directory, then
//! times one cold and several warm calls of a few graph read tools:
//!
//! ```text
//! MYC_BENCH_PROJECT=/path/to/project \
//!   cargo test --release -p myceliums-mcp --test graph_cache_bench -- --ignored --nocapture
//! ```
//!
//! Without `MYC_BENCH_PROJECT` it uses the small `sample-ts-project` fixture.

use myceliums_mcp::{AnalyzeParams, GetCallersParams, GetStatsParams, MyceliumsMcp, SearchParams};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const WARM_CALLS: u32 = 20;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[tokio::test]
#[ignore = "manual benchmark; run with --ignored --nocapture"]
async fn cold_vs_warm_read_latency() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    let project = std::env::var_os("MYC_BENCH_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-ts-project")
        })
        .canonicalize()
        .unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let server = MyceliumsMcp::new();
    let report = server
        .analyze_for_test(AnalyzeParams {
            path: project.to_string_lossy().into_owned(),
            force: Some(true),
            skip_embeddings: Some(true),
            ..Default::default()
        })
        .await
        .expect("analyze");
    println!("{report}");

    // Each tool gets a fresh server, so its first call finds an empty cache.
    macro_rules! bench {
        ($label:expr, $server:ident, $call:expr) => {{
            let $server = MyceliumsMcp::new();
            let start = Instant::now();
            let _ = $call;
            let cold = start.elapsed();
            let mut warm = Vec::new();
            for _ in 0..WARM_CALLS {
                let start = Instant::now();
                let _ = $call;
                warm.push(start.elapsed());
            }
            println!(
                "{:<16} cold {:>9.2?}  warm median {:>9.2?}  (table loads: {})",
                $label,
                cold,
                median(warm),
                $server.graph_table_loads_for_test()
            );
        }};
    }

    bench!(
        "context_search",
        s,
        s.context_search_for_test(SearchParams {
            query: "parse".to_string(),
            repo_id: Some(repo_id.clone()),
            ..Default::default()
        })
        .await
    );
    bench!(
        "get_callers",
        s,
        s.get_callers_for_test(GetCallersParams {
            repo_id: repo_id.clone(),
            symbol_name: "main".to_string(),
            ..Default::default()
        })
        .await
    );
    bench!(
        "get_stats",
        s,
        s.get_stats_for_test(GetStatsParams {
            repo_id: repo_id.clone(),
        })
        .await
    );
}
