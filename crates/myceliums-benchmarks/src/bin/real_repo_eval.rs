//! Score search on the held-out real-repository set and tune hybrid search on
//! its development split (#95).
//!
//! ```text
//! crates/myceliums-benchmarks/real-repos/fetch.sh            # once: check out the pinned repos
//! cargo run -p myceliums-benchmarks --release --bin real-repo-eval
//! cargo run -p myceliums-benchmarks --release --bin real-repo-eval -- --record
//! ```
//!
//! For every repository pinned in `real-repos/queries.json` it verifies the
//! checkout, parses it the way `myc analyze` reads it, indexes it with the
//! shipped embedding model, and then:
//!
//! 1. scores lexical, semantic, hybrid and hybrid+rerank on every query, with
//!    the per-split, per-repository and mode-vs-mode breakdowns;
//! 2. sweeps the hybrid fusion parameters on the **dev** split only;
//! 3. confirms the dev winner against the shipped default on the **held-out**
//!    split through the production hybrid search, and says whether the
//!    defaults should change (see `eval::sweep` for the rule);
//! 4. scores every mode again with test code left in place, sweeps the
//!    test-code demotion weight on **dev** and confirms it on **held-out**
//!    (#146, see `eval::demotion_sweep`);
//! 5. reports what the embeddings cost — indexing time, texts and chunks,
//!    vector and disk bytes, query latency, peak memory — and scores the
//!    long-symbol slice (#157).
//!
//! `--model`, `--template` and `--chunking` evaluate another embedding
//! configuration than the shipped one; `--modes` and `--no-demotion` keep such
//! a variant run to what it needs (#157, see `real-repo-compare`).
//!
//! The run is report-only: numbers never fail it. It fails when it cannot
//! measure honestly — a checkout at the wrong commit, a label that names no
//! symbol in the corpus, or a sweep that does not reproduce the production
//! hybrid ranking.
//!
//! Model weights follow `retrieval-eval`'s rules: cached weights are used,
//! missing ones are downloaded only with `--allow-model-download` or
//! `MYC_EVAL_MODELS=1`; without them only the lexical mode runs and the sweep
//! is skipped.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::eval::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use myceliums_benchmarks::eval::corpus::{symbol_ref, Corpus};
use myceliums_benchmarks::eval::demotion_sweep::{
    self, mean_over_modes, objective, objective_delta, WeightSweepInput, OBJECTIVE,
    TEST_SEEKING_FLOOR, TEST_SEEKING_METRIC, WEIGHT_GRID,
};
use myceliums_benchmarks::eval::evaluator::{
    evaluate_queries, Metric, MetricValues, ModeScore, QueryScore, Retrieval, SearchMode,
    RANKING_DEPTH,
};
use myceliums_benchmarks::eval::golden_set::GoldenQuery;
use myceliums_benchmarks::eval::long_symbols::{self, LongSymbolReport};
use myceliums_benchmarks::eval::models::{
    embedding_batch, embedding_batch_tokens, IndexBuildStats, ModelIndex, ModelPolicy,
    ModelProvenance, ModelSetup, Variant, ALLOW_DOWNLOAD_FLAG,
};
use myceliums_benchmarks::eval::real_repos::{
    repos_dir, verify_checkout, RealRepoSet, Split, REPOS_DIR_ENV,
};
use myceliums_benchmarks::eval::report::{
    format_delta, platform, round, rounded, BootstrapConfig, ModeReport, MODE_COMPARISONS,
};
use myceliums_benchmarks::eval::sweep::{
    decide, deltas, grid, select, sweep, Config, Decision, SweepInput, PRIMARY_METRIC,
    REPORTED_METRICS,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the report lands unless `--out` says otherwise (git-ignored).
const DEFAULT_REPORT: &str = "real-repos/report.json";

/// Per mode, query scores in a fixed order.
type Scores = BTreeMap<SearchMode, Vec<QueryScore>>;

/// The test-seeking queries (#146), relative to the crate.
const TEST_SEEKING: &str = "real-repos/test-seeking.json";

/// The committed record of a run, written by `--record`.
const RECORDED_RUN: &str = "real-repos/recorded-run.json";

/// Layout version of the report.
const REPORT_SCHEMA_VERSION: &str = "1.0";

struct Options {
    allow_model_download: bool,
    record: bool,
    sweep: bool,
    /// Score with test code left in place, the test-seeking queries and the
    /// demotion sweep (#146).
    demotion: bool,
    repos_dir: Option<PathBuf>,
    report_path: PathBuf,
    summary_path: Option<PathBuf>,
    /// An embedding configuration other than the shipped one (#157).
    variant: Variant,
    /// The modes to score.
    modes: Vec<SearchMode>,
    /// Only these repositories (a quick local run); `None`: all of them.
    repos: Option<Vec<String>>,
}

impl Options {
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            allow_model_download: false,
            record: false,
            sweep: true,
            demotion: true,
            repos_dir: None,
            report_path: crate_dir.join(DEFAULT_REPORT),
            summary_path: None,
            variant: Variant::default(),
            modes: SearchMode::all().to_vec(),
            repos: None,
        };
        let (mut model, mut template, mut chunking) = (None, None, None);
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                ALLOW_DOWNLOAD_FLAG => options.allow_model_download = true,
                "--record" => options.record = true,
                "--no-sweep" => options.sweep = false,
                "--no-demotion" => options.demotion = false,
                "--repos" => {
                    options.repos = Some(
                        args.next()
                            .context("--repos requires a list")?
                            .split(',')
                            .map(|r| r.trim().to_string())
                            .filter(|r| !r.is_empty())
                            .collect(),
                    )
                }
                "--model" => model = Some(args.next().context("--model requires an id")?),
                "--template" => {
                    template = Some(args.next().context("--template requires a version")?)
                }
                "--chunking" => {
                    chunking = Some(args.next().context("--chunking requires on or off")?)
                }
                "--modes" => {
                    options.modes =
                        SearchMode::parse_list(&args.next().context("--modes requires a list")?)?
                }
                "--repos-dir" => {
                    options.repos_dir = Some(PathBuf::from(
                        args.next().context("--repos-dir requires a path")?,
                    ))
                }
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
                        "real-repo-eval [options]\n\n  \
                         --repos-dir <dir>       checkouts (default ${REPOS_DIR_ENV} or ~/.cache/myceliums-eval-repos)\n  \
                         {ALLOW_DOWNLOAD_FLAG}  download missing model weights\n  \
                         --no-sweep              score the modes only\n  \
                         --no-demotion           skip the tests-included scoring and the demotion sweep\n  \
                         --model <id>            embedding model to evaluate instead of the shipped one\n  \
                         --template <1|2>        embedding-document template\n  \
                         --chunking <on|off>     chunk long symbols\n  \
                         --modes <list>          modes to score, e.g. lexical,semantic,hybrid\n  \
                         --repos <list>          only these repositories (a quick local run)\n  \
                         --out <path>            JSON report (default {DEFAULT_REPORT})\n  \
                         --record                also write {RECORDED_RUN} (the committed record)\n  \
                         --summary <path>        append a Markdown summary (e.g. $GITHUB_STEP_SUMMARY)"
                    );
                    std::process::exit(0);
                }
                other => bail!("unknown argument '{other}' (try --help)"),
            }
        }
        options.variant =
            Variant::parse(model.as_deref(), template.as_deref(), chunking.as_deref())?;
        if options.record
            && (!options.variant.is_shipped() || !options.demotion || options.repos.is_some())
        {
            bail!(
                "--record writes the committed run: the shipped configuration, demotion included"
            );
        }
        Ok(options)
    }
}

/// The JSON report of one run.
#[derive(Serialize)]
struct Report {
    schema_version: String,
    generated_at: String,
    commit_sha: String,
    platform: String,
    dataset_digest: String,
    dataset_version: String,
    review: ReviewState,
    repos: Vec<RepoReport>,
    models: Option<ModelProvenance>,
    /// Whether the embedding configuration is the shipped one (#157).
    shipped_configuration: bool,
    /// Most texts per embedding batch (`MYC_EVAL_EMBEDDING_BATCH`, default 256).
    embedding_batch: usize,
    /// Most tokens per embedding batch (`MYC_EVAL_EMBEDDING_BATCH_TOKENS`,
    /// default 16384; #157).
    embedding_batch_tokens: usize,
    /// What the embeddings cost (#157); `null` without an embedding index.
    embedding: Option<EmbeddingReport>,
    /// The long-symbol slice (#157).
    long_symbols: LongSymbolReport,
    bootstrap: BootstrapConfig,
    split: SplitReport,
    /// Every mode over every query, per-query detail included.
    results: BTreeMap<String, ModeReport>,
    /// Mode means per split.
    by_split: BTreeMap<String, BTreeMap<String, MetricValues>>,
    /// Mode means per repository.
    by_repo: BTreeMap<String, BTreeMap<String, MetricValues>>,
    /// Mode-vs-mode differences, keyed by scope (`all`, `held-out`), then
    /// `"<treatment> vs <control>"`, then metric id.
    comparisons: BTreeMap<String, BTreeMap<String, BTreeMap<String, DeltaEstimate>>>,
    sweep: Option<SweepReport>,
    /// Test-code demotion (#146): the weight sweep, its held-out
    /// confirmation, and every mode with test code left in place.
    test_demotion: Option<DemotionReport>,
}

#[derive(Serialize)]
struct WeightRow {
    weight: f64,
    /// Dev means per mode.
    dev: BTreeMap<String, MetricValues>,
    /// Dev objective: nDCG@10 averaged over the modes.
    objective: f64,
    /// Paired bootstrap of the objective against weight 1 (no demotion).
    vs_no_demotion: DeltaEstimate,
}

#[derive(Serialize)]
struct SeekingRow {
    weight: f64,
    by_mode: BTreeMap<String, MetricValues>,
    /// hit@10 and nDCG@10 averaged over the modes.
    hit_at_10: f64,
    ndcg_at_10: f64,
}

#[derive(Serialize)]
struct ModeComparison {
    /// Test code left in place (`--tests include`).
    include: MetricValues,
    /// Demoted at the chosen weight.
    demote: MetricValues,
    /// demote − include, paired bootstrap, per reported metric.
    delta: BTreeMap<String, DeltaEstimate>,
}

#[derive(Serialize)]
struct DemotionReport {
    objective: String,
    modes: Vec<String>,
    shipped_weight: f64,
    dev_queries: usize,
    held_out_queries: usize,
    /// Labels that name a symbol the index classifies as test code.
    labels_in_test_code: usize,
    /// How the dev sweep was checked against the production search paths.
    sweep_check: String,
    rows: Vec<WeightRow>,
    /// Test-seeking queries (labels are tests) and the share of their
    /// no-demotion hit@10 the chosen weight must keep.
    test_seeking_queries: usize,
    test_seeking_floor: f64,
    /// Test-seeking means per weight.
    seeking_rows: Vec<SeekingRow>,
    chosen: f64,
    /// Chosen − no demotion on held-out through the production paths: the
    /// objective, then per mode; `null` when the chosen weight is 1.
    held_out_objective: Option<DeltaEstimate>,
    held_out: Option<BTreeMap<String, BTreeMap<String, DeltaEstimate>>>,
    /// Chosen weight vs test code left in place, by scope (`all`,
    /// `held-out`) and mode.
    chosen_vs_include: BTreeMap<String, BTreeMap<String, ModeComparison>>,
    /// Chosen weight vs test code left in place on the test-seeking queries.
    test_seeking: BTreeMap<String, ModeComparison>,
    /// Chosen weight vs test code left in place, by repository and mode.
    by_repo: BTreeMap<String, BTreeMap<String, ModeComparison>>,
    decision: demotion_sweep::Decision,
}

/// What indexing and querying with the embedding configuration cost (#157).
#[derive(Serialize)]
struct EmbeddingReport {
    /// Intra-op threads of the ONNX sessions.
    threads: usize,
    /// Megabytes a first use of the model downloads.
    download_mb: u32,
    /// License of the weights.
    license: String,
    /// Per repository: the index build.
    by_repo: BTreeMap<String, IndexBuildStats>,
    /// Over the repositories.
    total: CostTotals,
    /// Semantic query latency (embedding the query + exact vector search for
    /// 20 results), per repository and over all queries, after 3 warm-up
    /// queries per repository.
    latency_ms: BTreeMap<String, Latency>,
    /// Peak resident memory of the process when indexing finished, in bytes
    /// (`null` where the platform does not report it).
    peak_rss_bytes: Option<u64>,
}

#[derive(Serialize, Default)]
struct CostTotals {
    symbols: usize,
    texts: usize,
    chunked_symbols: usize,
    chunk_vectors: usize,
    tokens: usize,
    embed_seconds: f64,
    index_seconds: f64,
    /// Symbols embedded per second of embedding wall time.
    symbols_per_second: f64,
    vector_bytes: u64,
    disk_bytes: u64,
}

#[derive(Serialize, Clone, Copy)]
struct Latency {
    queries: usize,
    p50: f64,
    p95: f64,
    mean: f64,
}

impl Latency {
    fn of(mut samples: Vec<f64>) -> Self {
        samples.sort_by(f64::total_cmp);
        let at = |q: f64| -> f64 {
            if samples.is_empty() {
                return 0.0;
            }
            let i = ((samples.len() - 1) as f64 * q).round() as usize;
            samples[i]
        };
        Self {
            queries: samples.len(),
            p50: round(at(0.5)),
            p95: round(at(0.95)),
            mean: round(samples.iter().sum::<f64>() / samples.len().max(1) as f64),
        }
    }
}

/// Peak resident set size of this process, in bytes.
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[derive(Serialize)]
struct ReviewState {
    queries: usize,
    /// Reviewed by a person other than the labeller.
    reviewed: usize,
    /// Checked by agents, accepted under a named owner policy (#164).
    agent_verified: usize,
    note: String,
}

#[derive(Serialize)]
struct RepoReport {
    name: String,
    tag: String,
    commit: String,
    language: String,
    queries: usize,
    symbols: usize,
    /// Symbols the index classifies as test code (#146).
    test_symbols: usize,
    skipped_files: usize,
}

#[derive(Serialize)]
struct SplitReport {
    rule: String,
    dev: usize,
    held_out: usize,
    assignment: BTreeMap<String, Split>,
}

#[derive(Serialize)]
struct SweepRow {
    config: Config,
    dev: MetricValues,
    /// Paired bootstrap difference to the shipped default on dev.
    vs_shipped: BTreeMap<String, DeltaEstimate>,
}

#[derive(Serialize)]
struct SweepReport {
    primary_metric: String,
    dev_queries: usize,
    held_out_queries: usize,
    /// How the dev fusion was checked against the production hybrid path.
    fusion_check: String,
    rows: Vec<SweepRow>,
    chosen: Config,
    /// Chosen − shipped on held-out, through the production hybrid search;
    /// `null` when the chosen configuration is the default.
    held_out: Option<BTreeMap<String, DeltaEstimate>>,
    decision: Decision,
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args(std::env::args())?;
    let started = Instant::now();
    let dataset_path = RealRepoSet::default_path();
    let mut set = RealRepoSet::from_path(&dataset_path)?;
    let assignment = set.split();
    if let Some(repos) = &options.repos {
        // The split is the full set's; only the queries of these repositories
        // are scored.
        if let Some(unknown) = repos
            .iter()
            .find(|r| !set.repos.iter().any(|p| &p.name == *r))
        {
            bail!("--repos names '{unknown}', which the set does not pin");
        }
        set.repos.retain(|p| repos.contains(&p.name));
        set.queries.retain(|q| repos.contains(&q.repo));
    }
    let dev_ids = ids_in(&set, Split::Dev);
    let held_ids = ids_in(&set, Split::HeldOut);
    let review_note = set.review_summary();
    println!(
        "real-repo set {}: {} queries over {} repositories — dev {} / held-out {}\n{review_note}",
        set.dataset_version,
        set.queries.len(),
        set.repos.len(),
        dev_ids.len(),
        held_ids.len()
    );

    // Checkouts and corpora.
    let dir = match &options.repos_dir {
        Some(dir) => dir.clone(),
        None => repos_dir()?,
    };
    let mut corpora = Vec::new();
    for pin in &set.repos {
        let root = verify_checkout(&dir, pin)?;
        let corpus = Corpus::load_repository(&root, &pin.name)
            .with_context(|| format!("parsing {}", pin.name))?;
        let known: BTreeSet<_> = corpus.symbols().iter().map(symbol_ref).collect();
        let missing: Vec<String> = set
            .queries_in(None, Some(&pin.name))
            .iter()
            .flat_map(|q| {
                q.relevant
                    .iter()
                    .filter(|label| !known.contains(&label.symbol_ref()))
                    .map(move |label| format!("{}: {}", q.id, label.symbol_ref()))
            })
            .collect();
        if !missing.is_empty() {
            bail!(
                "{} label(s) name symbols the {} corpus does not contain — the parser or the \
                 checkout changed under the labels:\n  {}",
                missing.len(),
                pin.name,
                missing.join("\n  ")
            );
        }
        println!(
            "  {:<10} {:>6} symbols  {:>3} skipped files",
            pin.name,
            corpus.len(),
            corpus.skipped().len()
        );
        corpora.push(corpus);
    }

    // Embedding indexes, when the weights are there.
    let setup = ModelSetup::variant(&options.variant)?;
    if !options.variant.is_shipped() {
        println!(
            "variant: {} · template {} · chunking {}",
            setup.embedding.model,
            setup.embedding.text_version,
            setup
                .embedding
                .chunking
                .map(|c| c.label())
                .unwrap_or_else(|| "off".to_string())
        );
    }
    let policy = ModelPolicy::from_flag_and_env(options.allow_model_download);
    let mut indices: Vec<ModelIndex> = Vec::new();
    let wants_models = options.modes.iter().any(SearchMode::needs_embeddings);
    if wants_models && setup.blocker(SearchMode::Semantic, policy).is_none() {
        for corpus in &corpora {
            let t = Instant::now();
            let index = ModelIndex::build(corpus, &setup, policy).await?;
            let stats = index.stats();
            println!(
                "  indexed {} in {:.0}s ({} texts, {} chunk vectors, {:.1} symbols/s)",
                corpus.repo_id(),
                t.elapsed().as_secs_f64(),
                stats.embed.texts,
                stats.chunk_vectors,
                stats.embed.symbols as f64 / stats.embed.seconds.max(1e-9)
            );
            indices.push(index);
        }
    }
    let peak_rss = peak_rss_bytes();
    let modes: Vec<SearchMode> = SearchMode::all()
        .into_iter()
        .filter(|mode| options.modes.contains(mode))
        .filter(|mode| !mode.needs_embeddings() || !indices.is_empty())
        .filter(|mode| setup.blocker(*mode, policy).is_none())
        .collect();

    // Every mode over every query.
    let mut all_scores: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
    for (i, corpus) in corpora.iter().enumerate() {
        let retrieval = match indices.get(i) {
            Some(index) => Retrieval::with_models(index),
            None => Retrieval::lexical_only(corpus).await?,
        };
        let golden: Vec<GoldenQuery> = set
            .queries_in(None, Some(corpus.repo_id()))
            .iter()
            .map(|q| q.golden())
            .collect();
        for &mode in &modes {
            let t = Instant::now();
            let score = evaluate_queries(mode, &retrieval, &golden).await?;
            println!(
                "  scored {:<14} on {:<10} in {:.0}s",
                mode.id(),
                corpus.repo_id(),
                t.elapsed().as_secs_f64()
            );
            all_scores.entry(mode).or_default().extend(score.queries);
        }
    }
    let all_ids: Vec<&str> = set.queries.iter().map(|q| q.id.as_str()).collect();
    let score_of =
        |mode: SearchMode, ids: &[&str]| -> ModeScore { restrict(mode, &all_scores[&mode], ids) };

    let mut results = BTreeMap::new();
    for mode in SearchMode::all() {
        let report = if modes.contains(&mode) {
            ModeReport::measured(&score_of(mode, &all_ids))
        } else if !options.modes.contains(&mode) {
            ModeReport::unavailable("not requested (--modes)".to_string())
        } else {
            ModeReport::unavailable(
                setup
                    .blocker(mode, policy)
                    .unwrap_or_else(|| "no embedding index".to_string()),
            )
        };
        results.insert(mode.id().to_string(), report);
    }
    let mut by_split = BTreeMap::new();
    for (split, ids) in [(Split::Dev, &dev_ids), (Split::HeldOut, &held_ids)] {
        by_split.insert(
            split.label().to_string(),
            modes
                .iter()
                .map(|&m| (m.id().to_string(), score_of(m, ids).mean.map(round)))
                .collect(),
        );
    }
    let mut by_repo = BTreeMap::new();
    for pin in &set.repos {
        let ids: Vec<&str> = set
            .queries_in(None, Some(&pin.name))
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        by_repo.insert(
            pin.name.clone(),
            modes
                .iter()
                .map(|&m| (m.id().to_string(), score_of(m, &ids).mean.map(round)))
                .collect(),
        );
    }
    let mut comparisons = BTreeMap::new();
    for (scope, ids) in [("all", &all_ids), ("held-out", &held_ids)] {
        let mut by_pair = BTreeMap::new();
        for (treatment, control) in MODE_COMPARISONS {
            if !modes.contains(&treatment) || !modes.contains(&control) {
                continue;
            }
            let (t, c) = (score_of(treatment, ids), score_of(control, ids));
            let mut by_metric = BTreeMap::new();
            for metric in Metric::ALL {
                let estimate =
                    paired_bootstrap_default(&t.per_query(metric), &c.per_query(metric))?;
                by_metric.insert(metric.id().to_string(), rounded(estimate));
            }
            by_pair.insert(format!("{} vs {}", treatment.id(), control.id()), by_metric);
        }
        if !by_pair.is_empty() {
            comparisons.insert(scope.to_string(), by_pair);
        }
    }

    // What the embeddings cost (#157): index builds, query latency, memory.
    let embedding_report = if indices.is_empty() {
        None
    } else {
        let mut latency_ms = BTreeMap::new();
        let mut all_samples = Vec::new();
        for (corpus, index) in corpora.iter().zip(&indices) {
            let queries = set.queries_in(None, Some(corpus.repo_id()));
            for query in queries.iter().take(3) {
                let vector = index.embed_query(&query.query).await?;
                index.store().vector_search(&vector, RANKING_DEPTH).await?;
            }
            let mut samples = Vec::new();
            for query in &queries {
                let t = Instant::now();
                let vector = index.embed_query(&query.query).await?;
                index.store().vector_search(&vector, RANKING_DEPTH).await?;
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            all_samples.extend(samples.iter().copied());
            latency_ms.insert(corpus.repo_id().to_string(), Latency::of(samples));
        }
        latency_ms.insert("all".to_string(), Latency::of(all_samples));
        let mut total = CostTotals::default();
        let mut by_repo = BTreeMap::new();
        for (corpus, index) in corpora.iter().zip(&indices) {
            let s = index.stats().clone();
            total.symbols += s.embed.symbols;
            total.texts += s.embed.texts;
            total.chunked_symbols += s.embed.chunked_symbols;
            total.chunk_vectors += s.chunk_vectors;
            total.tokens += s.embed.tokens;
            total.embed_seconds += s.embed.seconds;
            total.index_seconds += s.seconds;
            total.vector_bytes += s.vector_bytes;
            total.disk_bytes += s.disk_bytes;
            by_repo.insert(corpus.repo_id().to_string(), s);
        }
        total.symbols_per_second = round(total.symbols as f64 / total.embed_seconds.max(1e-9));
        total.embed_seconds = round(total.embed_seconds);
        total.index_seconds = round(total.index_seconds);
        Some(EmbeddingReport {
            threads: myceliums_core::embeddings::session_threads(),
            download_mb: setup.embedding_spec.download_mb,
            license: setup.embedding_spec.license.to_string(),
            by_repo,
            total,
            latency_ms,
            peak_rss_bytes: peak_rss,
        })
    };
    // Which symbols the index embedded with body chunks.
    let mut chunked: Option<std::collections::HashSet<String>> = None;
    if setup.embedding.chunking.is_some() {
        let mut refs = std::collections::HashSet::new();
        for index in &indices {
            let uids: std::collections::HashSet<String> = index
                .store()
                .get_chunk_vectors()
                .await?
                .into_iter()
                .map(|c| c.symbol_uid)
                .collect();
            refs.extend(
                index
                    .symbols()
                    .iter()
                    .filter(|s| uids.contains(&s.uid))
                    .map(|s| symbol_ref(s).to_string()),
            );
        }
        chunked = Some(refs);
    }
    let long_symbols = long_symbols::report(
        &set,
        &corpora,
        &all_scores,
        &modes,
        &assignment,
        chunked.as_ref(),
    )?;

    // Test-seeking queries (#146): their labels are tests, the cost of
    // demotion the regular labels cannot show.
    let mut seeking =
        RealRepoSet::from_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join(TEST_SEEKING))?;
    if let Some(repos) = &options.repos {
        seeking.repos.retain(|p| repos.contains(&p.name));
        seeking.queries.retain(|q| repos.contains(&q.repo));
    }
    if seeking.repos != set.repos {
        bail!("{TEST_SEEKING} must pin the same repositories as queries.json");
    }
    for corpus in &corpora {
        let tests: BTreeSet<_> = corpus
            .symbols()
            .iter()
            .filter(|s| s.is_test())
            .map(symbol_ref)
            .collect();
        for query in seeking.queries_in(None, Some(corpus.repo_id())) {
            for label in &query.relevant {
                if !tests.contains(&label.symbol_ref()) {
                    bail!(
                        "test-seeking query {} labels {}, which is not test code in the corpus",
                        query.id,
                        label.symbol_ref()
                    );
                }
            }
        }
    }

    // Test code left in place (#146): the control for the demotion.
    let mut include_scores: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
    let mut seeking_shipped: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
    let mut seeking_include: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
    let mut retrievals = Vec::new();
    let demotion_corpora: &[Corpus] = if options.demotion { &corpora } else { &[] };
    for (i, corpus) in demotion_corpora.iter().enumerate() {
        let retrieval = match indices.get(i) {
            Some(index) => Retrieval::with_models(index),
            None => Retrieval::lexical_only(corpus).await?,
        };
        let golden: Vec<GoldenQuery> = set
            .queries_in(None, Some(corpus.repo_id()))
            .iter()
            .map(|q| q.golden())
            .collect();
        let include = retrieval
            .clone()
            .with_test_policy(myceliums_core::TestPolicy::include());
        for &mode in &modes {
            let t = Instant::now();
            let score = evaluate_queries(mode, &include, &golden).await?;
            println!(
                "  scored {:<14} on {:<10} with tests included in {:.0}s",
                mode.id(),
                corpus.repo_id(),
                t.elapsed().as_secs_f64()
            );
            include_scores
                .entry(mode)
                .or_default()
                .extend(score.queries);
        }
        let seeking_golden: Vec<GoldenQuery> = seeking
            .queries_in(None, Some(corpus.repo_id()))
            .iter()
            .map(|q| q.golden())
            .collect();
        for &mode in &modes {
            let shipped = evaluate_queries(mode, &retrieval, &seeking_golden).await?;
            seeking_shipped
                .entry(mode)
                .or_default()
                .extend(shipped.queries);
            let included = evaluate_queries(mode, &include, &seeking_golden).await?;
            seeking_include
                .entry(mode)
                .or_default()
                .extend(included.queries);
        }
        retrievals.push(retrieval);
    }

    let demotion_report = if options.sweep && options.demotion {
        Some(
            tune_demotion(
                &set,
                &seeking,
                (&seeking_shipped, &seeking_include),
                &corpora,
                &retrievals,
                &indices,
                &modes,
                (&dev_ids, &held_ids, &all_ids),
                &all_scores,
                &include_scores,
            )
            .await?,
        )
    } else {
        None
    };

    // Tune on dev, confirm on held-out.
    let sweep_report = if options.sweep && !indices.is_empty() {
        Some(
            tune(
                &set,
                &corpora,
                &indices,
                &dev_ids,
                &held_ids,
                &all_scores[&SearchMode::Hybrid],
            )
            .await?,
        )
    } else {
        None
    };

    let report = Report {
        schema_version: REPORT_SCHEMA_VERSION.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        commit_sha: commit_sha(),
        platform: platform(),
        dataset_digest: RealRepoSet::digest_of(&dataset_path)?,
        dataset_version: set.dataset_version.clone(),
        review: ReviewState {
            queries: set.queries.len(),
            reviewed: set.reviewed(),
            agent_verified: set.agent_verified(),
            note: review_note,
        },
        repos: set
            .repos
            .iter()
            .zip(&corpora)
            .map(|(pin, corpus)| RepoReport {
                name: pin.name.clone(),
                tag: pin.tag.clone(),
                commit: pin.commit.clone(),
                language: pin.language.clone(),
                queries: set.queries_in(None, Some(&pin.name)).len(),
                symbols: corpus.len(),
                test_symbols: corpus.symbols().iter().filter(|s| s.is_test()).count(),
                skipped_files: corpus.skipped().len(),
            })
            .collect(),
        models: (!indices.is_empty()).then(|| setup.provenance()),
        shipped_configuration: options.variant.is_shipped(),
        embedding_batch: embedding_batch(),
        embedding_batch_tokens: embedding_batch_tokens(),
        embedding: embedding_report,
        long_symbols,
        bootstrap: BootstrapConfig::current(),
        split: SplitReport {
            rule: format!(
                "per (repository, intent) stratum, queries ordered by salted SHA-256 of their \
                 id; the first round({} x n) are dev",
                myceliums_benchmarks::eval::real_repos::DEV_FRACTION
            ),
            dev: dev_ids.len(),
            held_out: held_ids.len(),
            assignment: assignment.clone(),
        },
        results,
        by_split,
        by_repo,
        comparisons,
        sweep: sweep_report,
        test_demotion: demotion_report,
    };

    let markdown = render_markdown(&report);
    println!("\n{markdown}");
    write_json(&options.report_path, &report)?;
    println!("report written to {}", options.report_path.display());
    if options.record {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORDED_RUN);
        write_json(&path, &report)?;
        println!("recorded run written to {}", path.display());
    }
    if let Some(path) = &options.summary_path {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(markdown.as_bytes())?;
    }
    println!("done in {:.0}s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Sweep on dev, select, confirm on held-out.
async fn tune(
    set: &RealRepoSet,
    corpora: &[Corpus],
    indices: &[ModelIndex],
    dev_ids: &[&str],
    held_ids: &[&str],
    hybrid_scores: &[QueryScore],
) -> Result<SweepReport> {
    let configs = grid();
    let t = Instant::now();
    let inputs: Vec<SweepInput> = corpora
        .iter()
        .zip(indices)
        .map(|(corpus, index)| SweepInput {
            index,
            queries: set
                .queries_in(Some(Split::Dev), Some(corpus.repo_id()))
                .iter()
                .map(|q| q.golden())
                .collect(),
        })
        .collect();
    let swept = sweep(&inputs, &configs).await?;
    println!(
        "  swept {} configurations on {} dev queries in {:.0}s",
        configs.len(),
        dev_ids.len(),
        t.elapsed().as_secs_f64()
    );
    let swept: Vec<ModeScore> = swept
        .iter()
        .map(|score| restrict(SearchMode::Hybrid, &score.queries, dev_ids))
        .collect();

    // The decomposed fusion must reproduce the production hybrid ranking.
    let shipped_index = configs
        .iter()
        .position(Config::is_shipped)
        .context("the grid lacks the shipped default")?;
    let production = restrict(SearchMode::Hybrid, hybrid_scores, dev_ids);
    for (swept_query, product_query) in swept[shipped_index].queries.iter().zip(&production.queries)
    {
        if swept_query.values != product_query.values {
            bail!(
                "the sweep's fusion at the shipped defaults scored '{}' differently from the \
                 production hybrid search — the sweep no longer measures what ships",
                swept_query.id
            );
        }
    }
    let fusion_check = format!(
        "at the shipped defaults the dev fusion reproduced the production hybrid_search \
         scores on all {} dev queries",
        dev_ids.len()
    );

    let shipped = &swept[shipped_index];
    let mut rows = Vec::new();
    for (config, score) in configs.iter().zip(&swept) {
        rows.push(SweepRow {
            config: *config,
            dev: score.mean.map(round),
            vs_shipped: metric_map(deltas(score, shipped)?),
        });
    }
    let chosen = select(&configs, &swept);

    let (held_out, primary) = if chosen.is_shipped() {
        (None, None)
    } else {
        let mut chosen_scores = Vec::new();
        for (corpus, index) in corpora.iter().zip(indices) {
            let retrieval = Retrieval::with_models(index).with_hybrid_params(chosen.params());
            let golden: Vec<GoldenQuery> = set
                .queries_in(Some(Split::HeldOut), Some(corpus.repo_id()))
                .iter()
                .map(|q| q.golden())
                .collect();
            chosen_scores.extend(
                evaluate_queries(SearchMode::Hybrid, &retrieval, &golden)
                    .await?
                    .queries,
            );
        }
        let chosen_held = restrict(SearchMode::Hybrid, &chosen_scores, held_ids);
        let shipped_held = restrict(SearchMode::Hybrid, hybrid_scores, held_ids);
        let estimates = deltas(&chosen_held, &shipped_held)?;
        let primary = estimates
            .iter()
            .find(|(metric, _)| *metric == PRIMARY_METRIC)
            .map(|(_, estimate)| *estimate);
        (Some(metric_map(estimates)), primary)
    };
    Ok(SweepReport {
        primary_metric: PRIMARY_METRIC.id().to_string(),
        dev_queries: dev_ids.len(),
        held_out_queries: held_ids.len(),
        fusion_check,
        rows,
        chosen,
        held_out,
        decision: decide(chosen, primary),
    })
}

/// Sweep the test-code demotion weight on dev, confirm on held-out (#146).
#[allow(clippy::too_many_arguments)]
async fn tune_demotion(
    set: &RealRepoSet,
    seeking: &RealRepoSet,
    (seeking_shipped, seeking_include): (&Scores, &Scores),
    corpora: &[Corpus],
    retrievals: &[Retrieval<'_>],
    indices: &[ModelIndex],
    modes: &[SearchMode],
    (dev_ids, held_ids, all_ids): (&[&str], &[&str], &[&str]),
    shipped_scores: &BTreeMap<SearchMode, Vec<QueryScore>>,
    include_scores: &BTreeMap<SearchMode, Vec<QueryScore>>,
) -> Result<DemotionReport> {
    let shipped_weight = myceliums_core::DEFAULT_TEST_WEIGHT;
    let t = Instant::now();
    let inputs: Vec<WeightSweepInput> = corpora
        .iter()
        .zip(retrievals)
        .enumerate()
        .map(|(i, (corpus, retrieval))| WeightSweepInput {
            retrieval,
            index: indices.get(i),
            queries: set
                .queries_in(Some(Split::Dev), Some(corpus.repo_id()))
                .iter()
                .map(|q| q.golden())
                .collect(),
        })
        .collect();
    let swept = demotion_sweep::sweep(&inputs, modes, &WEIGHT_GRID).await?;
    println!(
        "  swept {} demotion weights on {} dev queries in {:.0}s",
        WEIGHT_GRID.len(),
        dev_ids.len(),
        t.elapsed().as_secs_f64()
    );
    // Every mode's dev scores per weight, in dev-id order.
    let swept: Vec<BTreeMap<SearchMode, Vec<QueryScore>>> = swept
        .into_iter()
        .map(|by_mode| {
            by_mode
                .into_iter()
                .map(|(mode, scores)| (mode, restrict(mode, &scores, dev_ids).queries))
                .collect()
        })
        .collect();

    // The sweep must reproduce the production paths at the shipped weight
    // and with test code left in place.
    let position = |weight: f64| {
        WEIGHT_GRID
            .iter()
            .position(|&w| (w - weight).abs() < 1e-12)
            .with_context(|| format!("the weight grid lacks {weight}"))
    };
    for (weight, production) in [(shipped_weight, shipped_scores), (1.0, include_scores)] {
        let i = position(weight)?;
        for &mode in modes {
            let product = restrict(mode, &production[&mode], dev_ids);
            for (swept_query, product_query) in swept[i][&mode].iter().zip(&product.queries) {
                if swept_query.values != product_query.values {
                    bail!(
                        "the demotion sweep at weight {weight} scored '{}' in mode {} differently \
                         from the production search — the sweep no longer measures what ships",
                        swept_query.id,
                        mode.id()
                    );
                }
            }
        }
    }
    let sweep_check = format!(
        "at weight {shipped_weight} and at 1 the sweep reproduced the production scores of \
         every mode on all {} dev queries",
        dev_ids.len()
    );

    let objectives: Vec<Vec<f64>> = swept
        .iter()
        .map(|by_mode| objective(by_mode, modes))
        .collect();
    let none = &objectives[position(1.0)?];
    let mut rows = Vec::new();
    for ((&weight, by_mode), values) in WEIGHT_GRID.iter().zip(&swept).zip(&objectives) {
        rows.push(WeightRow {
            weight,
            dev: by_mode
                .iter()
                .map(|(mode, scores)| {
                    (
                        mode.id().to_string(),
                        ModeScore::from_queries(*mode, scores.clone())
                            .mean
                            .map(round),
                    )
                })
                .collect(),
            objective: round(values.iter().sum::<f64>() / values.len().max(1) as f64),
            vs_no_demotion: objective_delta(values, none)?,
        });
    }
    // Test-seeking queries: what each weight costs a search for tests.
    let seeking_ids: Vec<&str> = seeking.queries.iter().map(|q| q.id.as_str()).collect();
    let seeking_inputs: Vec<WeightSweepInput> = corpora
        .iter()
        .zip(retrievals)
        .enumerate()
        .map(|(i, (corpus, retrieval))| WeightSweepInput {
            retrieval,
            index: indices.get(i),
            queries: seeking
                .queries_in(None, Some(corpus.repo_id()))
                .iter()
                .map(|q| q.golden())
                .collect(),
        })
        .collect();
    let seeking_swept: Vec<BTreeMap<SearchMode, Vec<QueryScore>>> =
        demotion_sweep::sweep(&seeking_inputs, modes, &WEIGHT_GRID)
            .await?
            .into_iter()
            .map(|by_mode| {
                by_mode
                    .into_iter()
                    .map(|(mode, scores)| (mode, restrict(mode, &scores, &seeking_ids).queries))
                    .collect()
            })
            .collect();
    for (weight, production) in [(shipped_weight, seeking_shipped), (1.0, seeking_include)] {
        let i = position(weight)?;
        for &mode in modes {
            let product = restrict(mode, &production[&mode], &seeking_ids);
            for (swept_query, product_query) in seeking_swept[i][&mode].iter().zip(&product.queries)
            {
                if swept_query.values != product_query.values {
                    bail!(
                        "the demotion sweep at weight {weight} scored test-seeking query '{}' in \
                         mode {} differently from the production search",
                        swept_query.id,
                        mode.id()
                    );
                }
            }
        }
    }
    let seeking_hits: Vec<Vec<f64>> = seeking_swept
        .iter()
        .map(|by_mode| mean_over_modes(by_mode, modes, TEST_SEEKING_METRIC))
        .collect();
    let seeking_rows: Vec<SeekingRow> = WEIGHT_GRID
        .iter()
        .zip(&seeking_swept)
        .zip(&seeking_hits)
        .map(|((&weight, by_mode), hits)| SeekingRow {
            weight,
            by_mode: by_mode
                .iter()
                .map(|(mode, scores)| {
                    (
                        mode.id().to_string(),
                        ModeScore::from_queries(*mode, scores.clone())
                            .mean
                            .map(round),
                    )
                })
                .collect(),
            hit_at_10: round(demotion_sweep::mean_of(hits)),
            ndcg_at_10: round(demotion_sweep::mean_of(&mean_over_modes(
                by_mode,
                modes,
                Metric::NdcgAt10,
            ))),
        })
        .collect();
    let chosen = demotion_sweep::select(&WEIGHT_GRID, &objectives, &seeking_hits);

    // The chosen weight on every query, through the production paths.
    let same = |a: f64, b: f64| (a - b).abs() < 1e-12;
    let (chosen_all, chosen_seeking) = if same(chosen, shipped_weight) {
        (shipped_scores.clone(), seeking_shipped.clone())
    } else if same(chosen, 1.0) {
        (include_scores.clone(), seeking_include.clone())
    } else {
        let mut all: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
        let mut tests: BTreeMap<SearchMode, Vec<QueryScore>> = BTreeMap::new();
        for (corpus, retrieval) in corpora.iter().zip(retrievals) {
            let chosen_retrieval = retrieval
                .clone()
                .with_test_policy(demotion_sweep::policy(chosen));
            for (queries, out) in [(set, &mut all), (seeking, &mut tests)] {
                let golden: Vec<GoldenQuery> = queries
                    .queries_in(None, Some(corpus.repo_id()))
                    .iter()
                    .map(|q| q.golden())
                    .collect();
                for &mode in modes {
                    let score = evaluate_queries(mode, &chosen_retrieval, &golden).await?;
                    out.entry(mode).or_default().extend(score.queries);
                }
            }
        }
        (all, tests)
    };

    // Held-out: chosen vs no demotion.
    let include_held: BTreeMap<SearchMode, Vec<QueryScore>> = modes
        .iter()
        .map(|&m| (m, restrict(m, &include_scores[&m], held_ids).queries))
        .collect();
    let (held_out_objective, held_out) = if same(chosen, 1.0) {
        (None, None)
    } else {
        let chosen_held: BTreeMap<SearchMode, Vec<QueryScore>> = modes
            .iter()
            .map(|&m| (m, restrict(m, &chosen_all[&m], held_ids).queries))
            .collect();
        let objective_estimate = objective_delta(
            &objective(&chosen_held, modes),
            &objective(&include_held, modes),
        )?;
        let mut per_mode = BTreeMap::new();
        for &mode in modes {
            per_mode.insert(
                mode.id().to_string(),
                demotion_sweep::deltas(&chosen_held[&mode], &include_held[&mode])?,
            );
        }
        (Some(objective_estimate), Some(per_mode))
    };

    let compare = |chosen: &BTreeMap<SearchMode, Vec<QueryScore>>,
                   include: &BTreeMap<SearchMode, Vec<QueryScore>>,
                   ids: &[&str]|
     -> Result<BTreeMap<String, ModeComparison>> {
        let mut out = BTreeMap::new();
        for &mode in modes {
            let demote = restrict(mode, &chosen[&mode], ids);
            let include = restrict(mode, &include[&mode], ids);
            out.insert(
                mode.id().to_string(),
                ModeComparison {
                    include: include.mean.map(round),
                    demote: demote.mean.map(round),
                    delta: demotion_sweep::deltas(&demote.queries, &include.queries)?,
                },
            );
        }
        Ok(out)
    };
    let mut chosen_vs_include = BTreeMap::new();
    chosen_vs_include.insert(
        "all".to_string(),
        compare(&chosen_all, include_scores, all_ids)?,
    );
    chosen_vs_include.insert(
        "held-out".to_string(),
        compare(&chosen_all, include_scores, held_ids)?,
    );
    let test_seeking = compare(&chosen_seeking, seeking_include, &seeking_ids)?;
    let mut by_repo = BTreeMap::new();
    for pin in &set.repos {
        let ids: Vec<&str> = set
            .queries_in(None, Some(&pin.name))
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        by_repo.insert(
            pin.name.clone(),
            compare(&chosen_all, include_scores, &ids)?,
        );
    }

    let test_refs: BTreeSet<_> = corpora
        .iter()
        .flat_map(|c| c.symbols().iter().filter(|s| s.is_test()).map(symbol_ref))
        .collect();
    let labels_in_test_code = set
        .queries
        .iter()
        .flat_map(|q| q.relevant.iter())
        .filter(|label| test_refs.contains(&label.symbol_ref()))
        .count();

    Ok(DemotionReport {
        objective: format!(
            "{} averaged over the modes {}",
            OBJECTIVE.id(),
            modes.iter().map(|m| m.id()).collect::<Vec<_>>().join(", ")
        ),
        modes: modes.iter().map(|m| m.id().to_string()).collect(),
        shipped_weight,
        dev_queries: dev_ids.len(),
        held_out_queries: held_ids.len(),
        labels_in_test_code,
        sweep_check,
        rows,
        test_seeking_queries: seeking_ids.len(),
        test_seeking_floor: TEST_SEEKING_FLOOR,
        seeking_rows,
        chosen,
        held_out_objective,
        held_out,
        chosen_vs_include,
        test_seeking,
        by_repo,
        decision: demotion_sweep::decide(chosen, held_out_objective),
    })
}

fn metric_map(estimates: Vec<(Metric, DeltaEstimate)>) -> BTreeMap<String, DeltaEstimate> {
    estimates
        .into_iter()
        .map(|(metric, estimate)| (metric.id().to_string(), estimate))
        .collect()
}

/// The ids of `split`, in dataset order.
fn ids_in(set: &RealRepoSet, split: Split) -> Vec<&str> {
    set.queries_in(Some(split), None)
        .iter()
        .map(|q| q.id.as_str())
        .collect()
}

/// `scores` restricted to `ids`, in the order of `ids` — the one order every
/// paired comparison of this run uses.
fn restrict(mode: SearchMode, scores: &[QueryScore], ids: &[&str]) -> ModeScore {
    let by_id: HashMap<&str, &QueryScore> = scores.iter().map(|q| (q.id.as_str(), q)).collect();
    ModeScore::from_queries(
        mode,
        ids.iter()
            .map(|id| (*by_id.get(id).expect("every query is scored")).clone())
            .collect(),
    )
}

fn write_json(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(report)?;
    json.push('\n');
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

/// The summary: corpus, modes, comparisons, sweep and decision.
fn render_markdown(report: &Report) -> String {
    let mut out = format!(
        "## Real-repository retrieval (#95)\n\nDataset `{}` · {} queries · {} · `{}`\n\n> {}\n\n",
        report.dataset_version,
        report.review.queries,
        report.platform,
        &report.dataset_digest[..12],
        report.review.note
    );
    out.push_str("| Repository | Language | Commit | Queries | Symbols |\n");
    out.push_str("| --- | --- | --- | ---: | ---: |\n");
    for repo in &report.repos {
        out.push_str(&format!(
            "| {} ({}) | {} | `{}` | {} | {} |\n",
            repo.name,
            repo.tag,
            repo.language,
            &repo.commit[..10],
            repo.queries,
            repo.symbols
        ));
    }

    let headline = [
        Metric::Mrr,
        Metric::NdcgAt10,
        Metric::RecallAt10,
        Metric::HitAt1,
        Metric::HitAt10,
        Metric::RPrecision,
    ];
    let table = |title: &str, rows: Vec<(String, MetricValues)>| -> String {
        let mut t = format!("\n**{title}**\n\n| Mode |");
        for m in headline {
            t.push_str(&format!(" {} |", m.label()));
        }
        t.push_str("\n| --- |");
        for _ in headline {
            t.push_str(" ---: |");
        }
        t.push('\n');
        for (name, values) in rows {
            t.push_str(&format!("| `{name}` |"));
            for m in headline {
                t.push_str(&format!(" {:.4} |", values.get(m)));
            }
            t.push('\n');
        }
        t
    };
    let all_rows: Vec<(String, MetricValues)> = report
        .results
        .iter()
        .filter_map(|(mode, r)| r.metrics.map(|m| (mode.clone(), m)))
        .collect();
    out.push_str(&table(
        &format!("All {} queries", report.review.queries),
        sort_modes(all_rows),
    ));
    for (split, by_mode) in &report.by_split {
        out.push_str(&table(
            &format!("{split} split"),
            sort_modes(by_mode.iter().map(|(k, v)| (k.clone(), *v)).collect()),
        ));
    }
    for (repo, by_mode) in &report.by_repo {
        out.push_str(&table(
            repo,
            sort_modes(by_mode.iter().map(|(k, v)| (k.clone(), *v)).collect()),
        ));
    }
    for (scope, by_pair) in &report.comparisons {
        out.push_str(&format!(
            "\n**Mode vs mode, {scope} (paired bootstrap, 95% CI, uncorrected)**\n\n\
             | Comparison | Δ nDCG@10 | Δ MRR@10 | Δ recall@10 |\n| --- | ---: | ---: | ---: |\n"
        ));
        for (pair, by_metric) in by_pair {
            out.push_str(&format!(
                "| `{pair}` | {} | {} | {} |\n",
                format_delta(&by_metric["ndcg_at_10"]),
                format_delta(&by_metric["mrr"]),
                format_delta(&by_metric["recall_at_10"])
            ));
        }
    }
    if let Some(models) = &report.models {
        out.push_str(&format!(
            "\n### Embedding configuration (#157)\n\n`{}` (`{}`), {} dimensions, template {}, \
             chunking {}{}.\n",
            models.embedding_model,
            models.embedding_weights,
            models.embedding_dim,
            models.embedding_text_version,
            models.embedding_chunking.as_deref().unwrap_or("off"),
            if report.shipped_configuration {
                " — the shipped configuration"
            } else {
                " — a variant, not the shipped configuration"
            }
        ));
    }
    if let Some(e) = &report.embedding {
        out.push_str(
            "\n| Repository | Symbols | Texts | Chunk vectors | Embed s | Symbols/s | Vector MB | Disk MB | Query p50 / p95 ms |\n\
             | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        );
        for (repo, s) in &e.by_repo {
            let l = e.latency_ms.get(repo);
            out.push_str(&format!(
                "| {repo} | {} | {} | {} | {:.0} | {:.1} | {:.1} | {:.1} | {} |\n",
                s.embed.symbols,
                s.embed.texts,
                s.chunk_vectors,
                s.embed.seconds,
                s.embed.symbols as f64 / s.embed.seconds.max(1e-9),
                s.vector_bytes as f64 / 1e6,
                s.disk_bytes as f64 / 1e6,
                l.map(|l| format!("{:.1} / {:.1}", l.p50, l.p95))
                    .unwrap_or_default()
            ));
        }
        let t = &e.total;
        let l = e.latency_ms.get("all");
        out.push_str(&format!(
            "| **all** | {} | {} | {} | {:.0} | {:.1} | {:.1} | {:.1} | {} |\n\n\
             {} threads · download {} MB · license {} · peak RSS {}\n",
            t.symbols,
            t.texts,
            t.chunk_vectors,
            t.embed_seconds,
            t.symbols_per_second,
            t.vector_bytes as f64 / 1e6,
            t.disk_bytes as f64 / 1e6,
            l.map(|l| format!("{:.1} / {:.1}", l.p50, l.p95))
                .unwrap_or_default(),
            e.threads,
            e.download_mb,
            e.license,
            e.peak_rss_bytes
                .map(|b| format!("{:.2} GB", b as f64 / 1e9))
                .unwrap_or_else(|| "n/a".to_string())
        ));
    }
    out.push_str(&long_symbols::markdown(&report.long_symbols));
    if let Some(sweep) = &report.sweep {
        out.push_str(&format!(
            "\n### Hybrid parameter sweep — dev split ({} queries)\n\n{}.\n\n\
             Rows sorted by dev {}; Δ is against the shipped default on the same dev queries.\n\n\
             | Config | nDCG@10 | MRR@10 | recall@10 | Δ nDCG@10 vs shipped [95% CI] |\n\
             | --- | ---: | ---: | ---: | ---: |\n",
            sweep.dev_queries, sweep.fusion_check, sweep.primary_metric
        ));
        let mut rows: Vec<&SweepRow> = sweep.rows.iter().collect();
        rows.sort_by(|a, b| {
            b.dev
                .get(PRIMARY_METRIC)
                .total_cmp(&a.dev.get(PRIMARY_METRIC))
        });
        for row in rows {
            let marker = if row.config.is_shipped() {
                " (shipped)"
            } else if row.config == sweep.chosen {
                " (chosen)"
            } else {
                ""
            };
            out.push_str(&format!(
                "| {}{marker} | {:.4} | {:.4} | {:.4} | {} |\n",
                row.config.label(),
                row.dev.ndcg_at_10,
                row.dev.mrr,
                row.dev.recall_at_10,
                format_delta(&row.vs_shipped["ndcg_at_10"])
            ));
        }
        if let Some(held) = &sweep.held_out {
            out.push_str(&format!(
                "\n**Held-out confirmation ({} queries): {} − shipped**\n\n| Metric | Δ [95% CI] |\n| --- | ---: |\n",
                sweep.held_out_queries,
                sweep.chosen.label()
            ));
            for metric in REPORTED_METRICS {
                out.push_str(&format!(
                    "| {} | {} |\n",
                    metric.label(),
                    format_delta(&held[metric.id()])
                ));
            }
        }
        out.push_str(&format!(
            "\n**Decision:** {}.\n",
            sweep.decision.reason.trim_end_matches('.')
        ));
    }
    if let Some(demotion) = &report.test_demotion {
        out.push_str(&render_demotion(demotion));
    }
    out
}

/// The test-code demotion section (#146).
fn render_demotion(d: &DemotionReport) -> String {
    let mut out = format!(
        "\n### Test-code demotion (#146) — dev split ({} queries)\n\n{}.\n\n\
         Objective: {}. {} label(s) name test code. Cells are dev nDCG@10; Δ is the \
         objective against weight 1 (no demotion) on the same queries.\n\n| Weight |",
        d.dev_queries, d.sweep_check, d.objective, d.labels_in_test_code
    );
    for mode in &d.modes {
        out.push_str(&format!(" `{mode}` |"));
    }
    out.push_str(" Objective | Δ objective [95% CI] |\n| ---: |");
    for _ in &d.modes {
        out.push_str(" ---: |");
    }
    out.push_str(" ---: | ---: |\n");
    for row in &d.rows {
        let marker = if (row.weight - d.chosen).abs() < 1e-12 {
            " (chosen)"
        } else if (row.weight - 1.0).abs() < 1e-12 {
            " (none)"
        } else {
            ""
        };
        out.push_str(&format!("| {}{marker} |", row.weight));
        for mode in &d.modes {
            out.push_str(&format!(" {:.4} |", row.dev[mode].ndcg_at_10));
        }
        out.push_str(&format!(
            " {:.4} | {} |\n",
            row.objective,
            format_delta(&row.vs_no_demotion)
        ));
    }
    out.push_str(&format!(
        "\n**Test-seeking queries ({}; labels are tests), means over the modes.** A weight \
         qualifies when it keeps hit@10 at no less than {} × the value at weight 1 and raises \
         the dev objective.\n\n| Weight | hit@10 | nDCG@10 |",
        d.test_seeking_queries, d.test_seeking_floor
    ));
    for mode in &d.modes {
        out.push_str(&format!(" `{mode}` hit@10 / nDCG@10 |"));
    }
    out.push_str("\n| ---: | ---: | ---: |");
    for _ in &d.modes {
        out.push_str(" ---: |");
    }
    out.push('\n');
    for row in &d.seeking_rows {
        out.push_str(&format!(
            "| {} | {:.4} | {:.4} |",
            row.weight, row.hit_at_10, row.ndcg_at_10
        ));
        for mode in &d.modes {
            let v = &row.by_mode[mode];
            out.push_str(&format!(" {:.3} / {:.3} |", v.hit_at_10, v.ndcg_at_10));
        }
        out.push('\n');
    }
    if let (Some(objective), Some(per_mode)) = (&d.held_out_objective, &d.held_out) {
        out.push_str(&format!(
            "\n**Held-out confirmation ({} queries): weight {} − no demotion**\n\n\
             Objective: {}\n\n| Mode | Δ nDCG@10 | Δ MRR@10 | Δ recall@10 | Δ hit@10 |\n\
             | --- | ---: | ---: | ---: | ---: |\n",
            d.held_out_queries,
            d.chosen,
            format_delta(objective)
        ));
        for mode in &d.modes {
            let m = &per_mode[mode];
            out.push_str(&format!(
                "| `{mode}` | {} | {} | {} | {} |\n",
                format_delta(&m["ndcg_at_10"]),
                format_delta(&m["mrr"]),
                format_delta(&m["recall_at_10"]),
                format_delta(&m["hit_at_10"])
            ));
        }
    }
    let table = |title: String, rows: &BTreeMap<String, ModeComparison>| -> String {
        let mut t = format!(
            "\n**{title}: nDCG@10 / MRR@10, tests included → demoted (weight {})**\n\n\
             | Mode | included | demoted | Δ nDCG@10 [95% CI] |\n| --- | ---: | ---: | ---: |\n",
            d.chosen
        );
        for mode in &d.modes {
            let c = &rows[mode];
            t.push_str(&format!(
                "| `{mode}` | {:.3} / {:.3} | {:.3} / {:.3} | {} |\n",
                c.include.ndcg_at_10,
                c.include.mrr,
                c.demote.ndcg_at_10,
                c.demote.mrr,
                format_delta(&c.delta["ndcg_at_10"])
            ));
        }
        t
    };
    for (scope, rows) in &d.chosen_vs_include {
        out.push_str(&table(format!("Demoted vs tests included, {scope}"), rows));
    }
    out.push_str(&table("Test-seeking queries".to_string(), &d.test_seeking));
    out.push_str("\nTest-seeking hit@10, tests included → demoted: ");
    let hits: Vec<String> = d
        .modes
        .iter()
        .map(|mode| {
            let c = &d.test_seeking[mode];
            format!(
                "`{mode}` {:.3} → {:.3}",
                c.include.hit_at_10, c.demote.hit_at_10
            )
        })
        .collect();
    out.push_str(&hits.join(", "));
    out.push('\n');
    for (repo, rows) in &d.by_repo {
        out.push_str(&table(repo.clone(), rows));
    }
    out.push_str(&format!(
        "\n**Decision:** {}.\n",
        d.decision.reason.trim_end_matches('.')
    ));
    out
}

/// Rows in report order: lexical, semantic, hybrid, hybrid+rerank.
fn sort_modes(mut rows: Vec<(String, MetricValues)>) -> Vec<(String, MetricValues)> {
    let order = |id: &str| {
        SearchMode::all()
            .iter()
            .position(|m| m.id() == id)
            .unwrap_or(usize::MAX)
    };
    rows.sort_by_key(|(id, _)| order(id));
    rows
}

/// Commit the run measured, with `-dirty` for uncommitted changes to tracked
/// files; informational. Untracked files are ignored: CI writes its own log
/// into the checkout while the run is going.
fn commit_sha() -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    match (
        git(&["rev-parse", "HEAD"]),
        git(&["status", "--porcelain", "--untracked-files=no"]),
    ) {
        (Some(sha), Some(changes)) if !changes.is_empty() => format!("{sha}-dirty"),
        (Some(sha), _) => sha,
        _ => "unknown".to_string(),
    }
}
