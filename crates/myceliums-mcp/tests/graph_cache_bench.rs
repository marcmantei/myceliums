//! Manual latency check for the cached read path (#93, #129). Ignored by
//! default.
//!
//! Indexes a project (embeddings off) into a throwaway data directory, then
//! times one cold and several warm calls of a few graph read tools, and the
//! service-level cost of reading all five tables per call — one snapshot per
//! table (the pre-#129 access pattern) versus one snapshot per call:
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

const WARM_CALLS: u32 = 200;

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
            let all = Instant::now();
            for _ in 0..WARM_CALLS {
                let start = Instant::now();
                let _ = $call;
                warm.push(start.elapsed());
            }
            let per_sec = f64::from(WARM_CALLS) / all.elapsed().as_secs_f64();
            println!(
                "{:<16} cold {:>9.2?}  warm median {:>9.2?}  warm {:>8.0} calls/s  (table loads: {})",
                $label,
                cold,
                median(warm),
                per_sec,
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

    // Service level: five warm table reads per call.
    let service = myceliums_core::GraphService::new(data.path());
    let id = repo_id.as_str();
    macro_rules! service_bench {
        ($label:expr, $body:expr) => {{
            $body; // warm the cache
            let (reads, loads) = (service.fingerprint_reads(), service.table_loads());
            let start = Instant::now();
            for _ in 0..SERVICE_CALLS {
                $body;
            }
            let elapsed = start.elapsed();
            println!(
                "{:<26} {:>8.0} calls/s  {:>7.2?}/call  fingerprint reads/call {}  table loads {}",
                $label,
                f64::from(SERVICE_CALLS) / elapsed.as_secs_f64(),
                elapsed / SERVICE_CALLS,
                (service.fingerprint_reads() - reads) / SERVICE_CALLS as usize,
                service.table_loads() - loads,
            );
        }};
    }
    service_bench!("5 tables, snapshot each", {
        service.graph(id).await.unwrap().symbols().await.unwrap();
        service.graph(id).await.unwrap().files().await.unwrap();
        service
            .graph(id)
            .await
            .unwrap()
            .relationships()
            .await
            .unwrap();
        service
            .graph(id)
            .await
            .unwrap()
            .communities()
            .await
            .unwrap();
        service.graph(id).await.unwrap().processes().await.unwrap();
    });
    service_bench!("5 tables, one snapshot", {
        let graph = service.graph(id).await.unwrap();
        graph.symbols().await.unwrap();
        graph.files().await.unwrap();
        graph.relationships().await.unwrap();
        graph.communities().await.unwrap();
        graph.processes().await.unwrap();
    });
}

const SERVICE_CALLS: u32 = 2000;
