//! Semantic search on one real repository through the exact path that ships
//! and through the pre-#117 IVF-PQ index, query by query (#141).
//!
//! ```text
//! crates/myceliums-benchmarks/real-repos/fetch.sh
//! cargo run -p myceliums-benchmarks --bin exact-vs-ann -- --repo hono --builds 5
//! ```
//!
//! The corpus is indexed once, as `real-repo-eval` indexes it. Every query is
//! embedded once, and the same query vector is then searched
//!
//! 1. through the production store (`Store::vector_search`, exact), checked
//!    against a brute-force cosine over every stored vector — the run fails if
//!    the results are not the top `k` in order or their scores are not the
//!    cosines — and against `ModelIndex::rank`, the path `real-repo-eval`
//!    scores;
//! 2. through the IVF-PQ index the product built before #117 (16 sub-vectors,
//!    √n partitions, LanceDB's default probes, no refine), built over the same
//!    table `--builds` times, since its k-means training need not be
//!    deterministic.
//!
//! A third ranking, brute-force cosine over the rows outside test files
//! ([`is_test_file`]), measures how much of the gap test code alone explains:
//! the labels never name it, so every test symbol in a top 10 is a slot no
//! label can fill.
//!
//! It reports both nDCG@10 per query, the paired bootstrap of the difference,
//! where each labelled symbol sits in the exact ordering, and the top 10 of
//! both paths for every query whose score differs. Report-only: numbers never
//! fail it.

use anyhow::{bail, ensure, Context, Result};
use myceliums_benchmarks::ann::pre_117_settings;
use myceliums_benchmarks::eval::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use myceliums_benchmarks::eval::corpus::{symbol_ref, Corpus};
use myceliums_benchmarks::eval::evaluator::{
    mask_repeats, score_ranking, Metric, MetricValues, SearchMode, RANKING_DEPTH,
};
use myceliums_benchmarks::eval::exact_vs_ann::{
    check_exact, is_test_file, label_ranks, BruteForce, Cosines, ExactCheck, LabelRank,
    EXACT_TOLERANCE,
};
use myceliums_benchmarks::eval::golden_set::{GoldenQuery, SymbolRef};
use myceliums_benchmarks::eval::metrics::mean;
use myceliums_benchmarks::eval::models::{
    embedding_batch, ModelIndex, ModelPolicy, ModelSetup, ALLOW_DOWNLOAD_FLAG,
};
use myceliums_benchmarks::eval::real_repos::{repos_dir, verify_checkout, RealRepoSet};
use myceliums_benchmarks::eval::report::{format_delta, platform, round, rounded};
use myceliums_core::{HybridParams, TestPolicy};
use myceliums_storage::{AnnIndexBuild, CodeSymbol};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One search's results, best first, with the score the store exposed.
type Results = Vec<(CodeSymbol, f32)>;

/// Results compared per query and path.
const TOP: usize = 10;

/// Metrics the report carries per query and path.
const METRICS: [Metric; 3] = [Metric::NdcgAt10, Metric::Mrr, Metric::RecallAt10];

struct Options {
    repo: String,
    builds: usize,
    allow_model_download: bool,
    repos_dir: Option<PathBuf>,
    out: Option<PathBuf>,
}

impl Options {
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Self {
            repo: "hono".to_string(),
            builds: 5,
            allow_model_download: false,
            repos_dir: None,
            out: None,
        };
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                ALLOW_DOWNLOAD_FLAG => options.allow_model_download = true,
                "--repo" => options.repo = args.next().context("--repo requires a name")?,
                "--builds" => {
                    options.builds = args
                        .next()
                        .context("--builds requires a count")?
                        .parse()
                        .context("--builds must be a positive integer")?
                }
                "--repos-dir" => {
                    options.repos_dir = Some(PathBuf::from(
                        args.next().context("--repos-dir requires a path")?,
                    ))
                }
                "--out" => {
                    options.out = Some(PathBuf::from(args.next().context("--out requires a path")?))
                }
                "--help" | "-h" => {
                    println!(
                        "exact-vs-ann [options]\n\n  \
                         --repo <name>          repository of real-repos/queries.json (default hono)\n  \
                         --builds <n>           IVF-PQ index builds to search (default 5)\n  \
                         --repos-dir <dir>      checkouts (default ~/.cache/myceliums-eval-repos)\n  \
                         --out <path>           also write the JSON report\n  \
                         {ALLOW_DOWNLOAD_FLAG}  download missing model weights"
                    );
                    std::process::exit(0);
                }
                other => bail!("unknown argument '{other}' (try --help)"),
            }
        }
        ensure!(options.builds > 0, "--builds must be at least 1");
        Ok(options)
    }
}

/// One returned row.
#[derive(Serialize)]
struct Hit {
    file: String,
    symbol: String,
    /// Exact cosine to the query.
    cosine: f64,
    /// 1-based exact rank in the whole table.
    exact_rank: usize,
    /// The score the search exposed (for the IVF-PQ path, from a quantised
    /// distance).
    score: f32,
    relevant: bool,
}

#[derive(Serialize)]
struct QueryReport {
    id: String,
    intent: String,
    split: String,
    query: String,
    labels: Vec<LabelRank>,
    exact: MetricValues,
    /// One per build.
    ann: Vec<MetricValues>,
    /// Per build: how many of the IVF-PQ top 10 are in the exact top 10
    /// (tie-tolerant), over 10.
    ann_recall_vs_exact: Vec<f64>,
    /// Brute-force cosine over the rows outside test files only.
    exact_without_tests: MetricValues,
    /// Share of the exact top 10 that lies in test files.
    exact_test_share: f64,
    /// Per build: share of the IVF-PQ top 10 that lies in test files.
    ann_test_share: Vec<f64>,
    exact_check: ExactCheck,
    exact_top: Vec<Hit>,
    /// The first build's top 10.
    ann_top: Vec<Hit>,
}

#[derive(Serialize)]
struct BuildSummary {
    build: usize,
    partitions: u32,
    seconds: f64,
    mean: MetricValues,
    mean_recall_vs_exact: f64,
    /// Mean share of the top 10 in test files.
    test_share: f64,
    /// IVF-PQ build − exact, paired over the queries.
    delta: BTreeMap<String, DeltaEstimate>,
    better: usize,
    worse: usize,
    same: usize,
}

#[derive(Serialize)]
struct Report {
    generated_at: String,
    platform: String,
    repo: String,
    commit: String,
    symbols: usize,
    queries: usize,
    embedding_batch: usize,
    exact_tolerance: f64,
    /// Worst exact-path error over every query.
    worst_exact_check: ExactCheck,
    exact_mean: MetricValues,
    /// Mean share of the exact top 10 in test files.
    exact_test_share: f64,
    /// Exact over the rows outside test files — what a search that never
    /// returned test code would score.
    exact_without_tests_mean: MetricValues,
    /// That ranking − exact, paired over the queries.
    exact_without_tests_delta: BTreeMap<String, DeltaEstimate>,
    builds: Vec<BuildSummary>,
    /// Per query, the IVF-PQ score averaged over the builds, − exact.
    mean_over_builds_delta: BTreeMap<String, DeltaEstimate>,
    per_query: Vec<QueryReport>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args(std::env::args())?;
    let started = Instant::now();
    let set = RealRepoSet::load()?;
    let assignment = set.split();
    let pin = set
        .repo(&options.repo)
        .with_context(|| format!("no repository '{}' in the real-repo set", options.repo))?
        .clone();
    let dir = match &options.repos_dir {
        Some(dir) => dir.clone(),
        None => repos_dir()?,
    };
    let root = verify_checkout(&dir, &pin)?;
    let corpus = Corpus::load_repository(&root, &pin.name)?;
    let queries: Vec<GoldenQuery> = set
        .queries_in(None, Some(&pin.name))
        .iter()
        .map(|q| q.golden())
        .collect();
    println!(
        "{}: {} symbols, {} queries",
        pin.name,
        corpus.len(),
        queries.len()
    );

    let setup = ModelSetup::shipped_default()?;
    let policy = ModelPolicy::from_flag_and_env(options.allow_model_download);
    let t = Instant::now();
    let index = ModelIndex::build(&corpus, &setup, policy).await?;
    println!("indexed in {:.0}s", t.elapsed().as_secs_f64());
    let table = BruteForce::new(index.store().get_symbols_with_vectors().await?)?;
    ensure!(
        table.len() == corpus.len(),
        "the store holds {} vectors for {} symbols",
        table.len(),
        corpus.len()
    );

    // One query vector per query, shared by every path.
    let mut vectors = Vec::with_capacity(queries.len());
    for query in &queries {
        vectors.push(index.embed_query(&query.query).await?);
    }
    let cosines: Vec<Cosines<'_>> = vectors.iter().map(|v| table.cosines(v)).collect();

    // Exact: the production store, checked against the oracle and against the
    // semantic ranking real-repo-eval scores with test code left in place
    // (`TestPolicy::include`; the shipped default reorders it since #146).
    let mut exact_runs = Vec::with_capacity(queries.len());
    for ((query, vector), cos) in queries.iter().zip(&vectors).zip(&cosines) {
        let results = index.store().vector_search(vector, RANKING_DEPTH).await?;
        let check = check_exact(&uid_scores(&results), cos)?;
        let evaluated: Vec<SymbolRef> = index
            .rank(
                SearchMode::Semantic,
                &query.query,
                RANKING_DEPTH,
                &HybridParams::with_tests(TestPolicy::include()),
            )
            .await?
            .iter()
            .map(symbol_ref)
            .collect();
        let refs: Vec<SymbolRef> = results.iter().map(|(s, _)| symbol_ref(s)).collect();
        ensure!(
            refs == evaluated,
            "'{}': the exact search here and real-repo-eval's semantic ranking differ",
            query.id
        );
        exact_runs.push((results, check));
    }
    let worst = exact_runs.iter().map(|(_, check)| *check).fold(
        ExactCheck {
            results: 0,
            max_score_error: 0.0,
            max_rank_error: 0.0,
        },
        |a, b| ExactCheck {
            results: a.results + b.results,
            max_score_error: a.max_score_error.max(b.max_score_error),
            max_rank_error: a.max_rank_error.max(b.max_rank_error),
        },
    );
    println!(
        "exact path vs brute-force cosine over {} results: max score error {:.2e}, \
         max rank error {:.2e} (tolerance {EXACT_TOLERANCE:.0e})",
        worst.results, worst.max_score_error, worst.max_rank_error
    );

    // IVF-PQ: the pre-#117 index over the same table, searched with its
    // settings; rebuilt per build.
    let settings = pre_117_settings();
    let threshold = settings
        .threshold
        .context("the pre-#117 setting has a threshold")?;
    let ann_store = index.reopen_with(settings).await?;
    let mut ann_runs: Vec<(u32, f64, Vec<Results>)> = Vec::new();
    for build in 0..options.builds {
        let t = Instant::now();
        let outcome = ann_store.create_ann_index(table.len(), threshold).await?;
        let AnnIndexBuild::Built { partitions } = outcome else {
            bail!("the IVF-PQ index was not built: {outcome:?}");
        };
        let seconds = t.elapsed().as_secs_f64();
        let mut runs = Vec::with_capacity(queries.len());
        for vector in &vectors {
            runs.push(ann_store.vector_search(vector, RANKING_DEPTH).await?);
        }
        println!(
            "IVF-PQ build {}: {partitions} partitions in {seconds:.1}s",
            build + 1
        );
        ann_runs.push((partitions, seconds, runs));
    }
    // The production handle must still be exact with the index present.
    for ((vector, cos), (exact, _)) in vectors.iter().zip(&cosines).zip(&exact_runs).take(5) {
        let again = index.store().vector_search(vector, RANKING_DEPTH).await?;
        ensure!(
            check_exact(&uid_scores(&again), cos)?.holds()
                && uid_scores(&again) == uid_scores(exact),
            "the production search changed once an IVF-PQ index existed"
        );
    }

    // Scores.
    let score = |query: &GoldenQuery, results: &[(CodeSymbol, f32)]| -> MetricValues {
        let ranking = mask_repeats(results.iter().map(|(s, _)| symbol_ref(s)).collect());
        score_ranking(query, &ranking).values
    };
    let mut per_query = Vec::with_capacity(queries.len());
    for (i, query) in queries.iter().enumerate() {
        let cos = &cosines[i];
        let (exact, check) = &exact_runs[i];
        let ann: Vec<MetricValues> = ann_runs
            .iter()
            .map(|(_, _, runs)| score(query, &runs[i]))
            .collect();
        let ann_recall_vs_exact = ann_runs
            .iter()
            .map(|(_, _, runs)| {
                let top = &runs[i][..runs[i].len().min(TOP)];
                top.iter()
                    .filter(|(s, _)| cos.of(&s.uid).is_some_and(|c| cos.in_top(c, TOP)))
                    .count() as f64
                    / TOP as f64
            })
            .collect();
        let test_share = |results: &[(CodeSymbol, f32)]| -> f64 {
            results
                .iter()
                .take(TOP)
                .filter(|(s, _)| is_test_file(&s.file_path))
                .count() as f64
                / TOP as f64
        };
        let without_tests: Vec<SymbolRef> = cos
            .top_where(RANKING_DEPTH, |s| !is_test_file(&s.file_path))
            .into_iter()
            .map(|(s, _)| symbol_ref(s))
            .collect();
        let real = set
            .queries
            .iter()
            .find(|q| q.id == query.id)
            .context("query vanished")?;
        per_query.push(QueryReport {
            id: query.id.clone(),
            intent: query.intent.label().to_string(),
            split: assignment
                .get(&query.id)
                .map(|s| s.label())
                .unwrap_or("?")
                .to_string(),
            query: real.query.clone(),
            labels: label_ranks(&table, cos, &query.relevant)?,
            exact: score(query, exact),
            ann,
            ann_recall_vs_exact,
            exact_without_tests: score_ranking(query, &mask_repeats(without_tests)).values,
            exact_test_share: test_share(exact),
            ann_test_share: ann_runs
                .iter()
                .map(|(_, _, runs)| test_share(&runs[i]))
                .collect(),
            exact_check: *check,
            exact_top: hits(exact, cos, &query.relevant),
            ann_top: hits(&ann_runs[0].2[i], cos, &query.relevant),
        });
    }

    let exact_per = |m: Metric| -> Vec<f64> { per_query.iter().map(|q| q.exact.get(m)).collect() };
    let mut builds = Vec::new();
    for (b, (partitions, seconds, _)) in ann_runs.iter().enumerate() {
        let ann_per =
            |m: Metric| -> Vec<f64> { per_query.iter().map(|q| q.ann[b].get(m)).collect() };
        let mut delta = BTreeMap::new();
        for metric in METRICS {
            delta.insert(
                metric.id().to_string(),
                rounded(paired_bootstrap_default(
                    &ann_per(metric),
                    &exact_per(metric),
                )?),
            );
        }
        let ndcg = |q: &QueryReport| (q.ann[b].ndcg_at_10, q.exact.ndcg_at_10);
        builds.push(BuildSummary {
            build: b + 1,
            partitions: *partitions,
            seconds: round(*seconds),
            mean: MetricValues::mean_of(per_query.iter().map(|q| &q.ann[b])).map(round),
            mean_recall_vs_exact: round(
                per_query
                    .iter()
                    .map(|q| q.ann_recall_vs_exact[b])
                    .sum::<f64>()
                    / per_query.len() as f64,
            ),
            test_share: round(mean(
                &per_query
                    .iter()
                    .map(|q| q.ann_test_share[b])
                    .collect::<Vec<_>>(),
            )),
            delta,
            better: per_query
                .iter()
                .filter(|q| {
                    let (a, e) = ndcg(q);
                    a > e + 1e-12
                })
                .count(),
            worse: per_query
                .iter()
                .filter(|q| {
                    let (a, e) = ndcg(q);
                    a + 1e-12 < e
                })
                .count(),
            same: per_query
                .iter()
                .filter(|q| {
                    let (a, e) = ndcg(q);
                    (a - e).abs() <= 1e-12
                })
                .count(),
        });
    }
    let mut mean_over_builds_delta = BTreeMap::new();
    for metric in METRICS {
        let averaged: Vec<f64> = per_query
            .iter()
            .map(|q| q.ann.iter().map(|v| v.get(metric)).sum::<f64>() / q.ann.len() as f64)
            .collect();
        mean_over_builds_delta.insert(
            metric.id().to_string(),
            rounded(paired_bootstrap_default(&averaged, &exact_per(metric))?),
        );
    }
    let mut exact_without_tests_delta = BTreeMap::new();
    for metric in METRICS {
        let filtered: Vec<f64> = per_query
            .iter()
            .map(|q| q.exact_without_tests.get(metric))
            .collect();
        exact_without_tests_delta.insert(
            metric.id().to_string(),
            rounded(paired_bootstrap_default(&filtered, &exact_per(metric))?),
        );
    }

    let report = Report {
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        platform: platform(),
        repo: pin.name.clone(),
        commit: pin.commit.clone(),
        symbols: table.len(),
        queries: queries.len(),
        embedding_batch: embedding_batch(),
        exact_tolerance: EXACT_TOLERANCE,
        worst_exact_check: worst,
        exact_mean: MetricValues::mean_of(per_query.iter().map(|q| &q.exact)).map(round),
        exact_test_share: round(mean(
            &per_query
                .iter()
                .map(|q| q.exact_test_share)
                .collect::<Vec<_>>(),
        )),
        exact_without_tests_mean: MetricValues::mean_of(
            per_query.iter().map(|q| &q.exact_without_tests),
        )
        .map(round),
        exact_without_tests_delta,
        builds,
        mean_over_builds_delta,
        per_query,
    };
    print!("{}", render(&report));
    if let Some(path) = &options.out {
        write_json(path, &report)?;
        println!("report written to {}", path.display());
    }
    ensure!(
        worst.holds(),
        "the exact path disagrees with brute-force cosine beyond {EXACT_TOLERANCE:e}"
    );
    println!("done in {:.0}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn uid_scores(results: &[(CodeSymbol, f32)]) -> Vec<(String, f32)> {
    results
        .iter()
        .map(|(s, score)| (s.uid.clone(), *score))
        .collect()
}

fn hits(results: &[(CodeSymbol, f32)], cos: &Cosines<'_>, relevant: &[SymbolRef]) -> Vec<Hit> {
    results
        .iter()
        .take(TOP)
        .map(|(s, score)| {
            let value = cos.of(&s.uid).unwrap_or(f64::NAN);
            Hit {
                file: s.file_path.clone(),
                symbol: s.qualified_name.clone(),
                cosine: value,
                exact_rank: cos.rank_of(value),
                score: *score,
                relevant: relevant.contains(&symbol_ref(s)),
            }
        })
        .collect()
}

fn render(report: &Report) -> String {
    let mut out = format!(
        "\n## Exact vs pre-#117 IVF-PQ, semantic search on {} ({} symbols, {} queries, {})\n\n",
        report.repo, report.symbols, report.queries, report.platform
    );
    out.push_str(&format!(
        "Exact path vs brute-force cosine: max score error {:.1e}, max rank error {:.1e} \
         over {} results.\n\n",
        report.worst_exact_check.max_score_error,
        report.worst_exact_check.max_rank_error,
        report.worst_exact_check.results
    ));
    out.push_str(
        "| Path | nDCG@10 | MRR@10 | recall@10 | top-10 recall vs exact | top 10 in test files | Δ nDCG@10 vs exact [95% CI] | better / worse / same |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n",
    );
    out.push_str(&format!(
        "| exact | {:.4} | {:.4} | {:.4} | 1.000 | {:.3} | — | — |\n",
        report.exact_mean.ndcg_at_10,
        report.exact_mean.mrr,
        report.exact_mean.recall_at_10,
        report.exact_test_share
    ));
    out.push_str(&format!(
        "| exact, test files excluded | {:.4} | {:.4} | {:.4} | — | 0.000 | {} | — |\n",
        report.exact_without_tests_mean.ndcg_at_10,
        report.exact_without_tests_mean.mrr,
        report.exact_without_tests_mean.recall_at_10,
        format_delta(&report.exact_without_tests_delta["ndcg_at_10"])
    ));
    for b in &report.builds {
        out.push_str(&format!(
            "| IVF-PQ build {} | {:.4} | {:.4} | {:.4} | {:.3} | {:.3} | {} | {} / {} / {} |\n",
            b.build,
            b.mean.ndcg_at_10,
            b.mean.mrr,
            b.mean.recall_at_10,
            b.mean_recall_vs_exact,
            b.test_share,
            format_delta(&b.delta["ndcg_at_10"]),
            b.better,
            b.worse,
            b.same
        ));
    }
    out.push_str(&format!(
        "\nIVF-PQ averaged over builds − exact: nDCG@10 {}, MRR@10 {}, recall@10 {}\n",
        format_delta(&report.mean_over_builds_delta["ndcg_at_10"]),
        format_delta(&report.mean_over_builds_delta["mrr"]),
        format_delta(&report.mean_over_builds_delta["recall_at_10"])
    ));

    out.push_str("\n### Queries whose nDCG@10 differs in any build\n");
    for q in &report.per_query {
        if q.ann
            .iter()
            .all(|a| (a.ndcg_at_10 - q.exact.ndcg_at_10).abs() <= 1e-12)
        {
            continue;
        }
        let ann: Vec<String> = q
            .ann
            .iter()
            .map(|a| format!("{:.3}", a.ndcg_at_10))
            .collect();
        out.push_str(&format!(
            "\n**{}** ({}, {}) `{}` — exact {:.3}, IVF-PQ {}\n\nlabels (exact rank, cosine): ",
            q.id,
            q.intent,
            q.split,
            q.query,
            q.exact.ndcg_at_10,
            ann.join(" / ")
        ));
        let labels: Vec<String> = q
            .labels
            .iter()
            .map(|l| format!("{} ({}, {:.4})", l.label.symbol, l.exact_rank, l.cosine))
            .collect();
        out.push_str(&labels.join("; "));
        out.push_str("\n\n| # | exact top 10 | cos | IVF-PQ build 1 top 10 | cos | exact rank |\n| ---: | --- | ---: | --- | ---: | ---: |\n");
        for r in 0..TOP {
            let cell = |h: Option<&Hit>| match h {
                Some(h) => (
                    format!("{}{}", if h.relevant { "**✓** " } else { "" }, short(h)),
                    format!("{:.4}", h.cosine),
                    h.exact_rank.to_string(),
                ),
                None => (String::new(), String::new(), String::new()),
            };
            let (e, ec, _) = cell(q.exact_top.get(r));
            let (a, ac, ar) = cell(q.ann_top.get(r));
            out.push_str(&format!("| {} | {e} | {ec} | {a} | {ac} | {ar} |\n", r + 1));
        }
    }
    out
}

fn short(hit: &Hit) -> String {
    format!("{} `{}`", hit.symbol, hit.file)
}

fn write_json(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(report)?;
    json.push('\n');
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}
