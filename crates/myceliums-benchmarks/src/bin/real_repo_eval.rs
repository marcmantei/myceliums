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
//! shipped embedding model (the ANN index included, where the repository is
//! large enough), and then:
//!
//! 1. scores lexical, semantic, hybrid and hybrid+rerank on every query, with
//!    the per-split, per-repository and mode-vs-mode breakdowns;
//! 2. sweeps the hybrid fusion parameters on the **dev** split only;
//! 3. confirms the dev winner against the shipped default on the **held-out**
//!    split through the production hybrid search, and says whether the
//!    defaults should change (see `eval::sweep` for the rule).
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
use myceliums_benchmarks::eval::evaluator::{
    evaluate_queries, Metric, MetricValues, ModeScore, QueryScore, Retrieval, SearchMode,
};
use myceliums_benchmarks::eval::golden_set::GoldenQuery;
use myceliums_benchmarks::eval::models::{
    embedding_batch, ModelIndex, ModelPolicy, ModelProvenance, ModelSetup, ALLOW_DOWNLOAD_FLAG,
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
use myceliums_storage::store::ANN_INDEX_THRESHOLD;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the report lands unless `--out` says otherwise (git-ignored).
const DEFAULT_REPORT: &str = "real-repos/report.json";

/// The committed record of a run, written by `--record`.
const RECORDED_RUN: &str = "real-repos/recorded-run.json";

/// Layout version of the report.
const REPORT_SCHEMA_VERSION: &str = "1.0";

struct Options {
    allow_model_download: bool,
    record: bool,
    sweep: bool,
    repos_dir: Option<PathBuf>,
    report_path: PathBuf,
    summary_path: Option<PathBuf>,
}

impl Options {
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            allow_model_download: false,
            record: false,
            sweep: true,
            repos_dir: None,
            report_path: crate_dir.join(DEFAULT_REPORT),
            summary_path: None,
        };
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                ALLOW_DOWNLOAD_FLAG => options.allow_model_download = true,
                "--record" => options.record = true,
                "--no-sweep" => options.sweep = false,
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
                         --out <path>            JSON report (default {DEFAULT_REPORT})\n  \
                         --record                also write {RECORDED_RUN} (the committed record)\n  \
                         --summary <path>        append a Markdown summary (e.g. $GITHUB_STEP_SUMMARY)"
                    );
                    std::process::exit(0);
                }
                other => bail!("unknown argument '{other}' (try --help)"),
            }
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
    /// Symbols embedded per batch (`MYC_EVAL_EMBEDDING_BATCH`, default 256).
    embedding_batch: usize,
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
}

#[derive(Serialize)]
struct ReviewState {
    queries: usize,
    reviewed: usize,
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
    skipped_files: usize,
    /// Whether the vector search ran on the IVF-PQ index rather than a flat scan.
    ann_index: bool,
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
    let set = RealRepoSet::from_path(&dataset_path)?;
    let assignment = set.split();
    let dev_ids = ids_in(&set, Split::Dev);
    let held_ids = ids_in(&set, Split::HeldOut);
    let review_note = if set.reviewed() == set.queries.len() {
        "every query's labels were reviewed by a human".to_string()
    } else {
        format!(
            "{} of {} queries carry agent-drafted labels no human has reviewed — \
             the numbers are provisional until they are (real-repos/README.md)",
            set.queries.len() - set.reviewed(),
            set.queries.len()
        )
    };
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
            "  {:<10} {:>6} symbols  {:>3} skipped files  {}",
            pin.name,
            corpus.len(),
            corpus.skipped().len(),
            if corpus.len() >= ANN_INDEX_THRESHOLD {
                "IVF-PQ index"
            } else {
                "flat vector scan"
            }
        );
        corpora.push(corpus);
    }

    // Embedding indexes, when the weights are there.
    let setup = ModelSetup::shipped_default()?;
    let policy = ModelPolicy::from_flag_and_env(options.allow_model_download);
    let mut indices: Vec<ModelIndex> = Vec::new();
    if setup.blocker(SearchMode::Semantic, policy).is_none() {
        for corpus in &corpora {
            let t = Instant::now();
            indices.push(ModelIndex::build(corpus, &setup, policy).await?);
            println!(
                "  indexed {} in {:.0}s",
                corpus.repo_id(),
                t.elapsed().as_secs_f64()
            );
        }
    }
    let modes: Vec<SearchMode> = SearchMode::all()
        .into_iter()
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
                skipped_files: corpus.skipped().len(),
                ann_index: !indices.is_empty() && corpus.len() >= ANN_INDEX_THRESHOLD,
            })
            .collect(),
        models: (!indices.is_empty()).then(|| setup.provenance()),
        embedding_batch: embedding_batch(),
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
    out.push_str("| Repository | Language | Commit | Queries | Symbols | Vector search |\n");
    out.push_str("| --- | --- | --- | ---: | ---: | --- |\n");
    for repo in &report.repos {
        out.push_str(&format!(
            "| {} ({}) | {} | `{}` | {} | {} | {} |\n",
            repo.name,
            repo.tag,
            repo.language,
            &repo.commit[..10],
            repo.queries,
            repo.symbols,
            if repo.ann_index { "IVF-PQ" } else { "flat" }
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

/// Commit the run measured, with `-dirty` for uncommitted code; informational.
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
    match (git(&["rev-parse", "HEAD"]), git(&["status", "--porcelain"])) {
        (Some(sha), Some(changes)) if !changes.is_empty() => format!("{sha}-dirty"),
        (Some(sha), _) => sha,
        _ => "unknown".to_string(),
    }
}
