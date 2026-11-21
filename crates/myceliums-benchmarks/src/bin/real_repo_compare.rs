//! Compare `real-repo-eval` reports of embedding variants (#157).
//!
//! ```text
//! real-repo-compare --baseline A.json B.json C.json ... [--out compare.json] [--summary summary.md]
//! ```
//!
//! Every report must score the same dataset. The baseline is the incumbent
//! (`multilingual-e5-small`, template 1, no chunking) measured in the same
//! workflow dispatch, so the cost ratios compare runs on the same runner class.
//! For each variant the tool prints, per the protocol fixed in
//! `benchmarks/METHODOLOGY.md` ("Embedding model, document template and
//! chunking"):
//!
//! - the objective — per query the mean of semantic and hybrid nDCG@10 — on
//!   dev, held-out and all queries, with paired bootstrap differences to the
//!   baseline;
//! - nDCG@10 and hit@10 per repository and mode;
//! - the costs against the budget (embedding time, vector bytes, query
//!   latency, peak memory, download, license);
//! - the long-symbol slice, chunking on against off for the same model and
//!   template;
//! - the selection on dev among in-budget variants and its held-out
//!   confirmation.
//!
//! It reads reports only; it never runs a model. `--same-scores A B` checks
//! instead that two reports of one configuration agree on every query.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::eval::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use myceliums_benchmarks::eval::report::{format_delta, round, rounded};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;

/// The budget of the protocol: ratios against the baseline, absolute limits.
const MAX_TIME_RATIO: f64 = 2.5;
const MAX_SIZE_RATIO: f64 = 3.0;
const MAX_P50_MS: f64 = 50.0;
const MAX_P95_MS: f64 = 150.0;
const MAX_RSS_RATIO: f64 = 1.5;
const MAX_DOWNLOAD_MB: f64 = 700.0;
const PERMISSIVE: [&str; 3] = ["MIT", "Apache-2.0", "BSD-3-Clause"];

/// The modes the objective averages.
const OBJECTIVE_MODES: [&str; 2] = ["semantic", "hybrid"];

struct Run {
    path: PathBuf,
    model: String,
    template: u64,
    chunking: Option<String>,
    /// Per mode, per query id: nDCG@10, hit@10, MRR@10.
    per_query: BTreeMap<String, BTreeMap<String, [f64; 3]>>,
    split: BTreeMap<String, String>,
    repos: Vec<String>,
    long: BTreeMap<String, String>,
    embedding: Value,
    digest: String,
    /// The incumbent indexed on the same runner (`calibration-<id>.json`
    /// next to `variant-<id>.json`), the denominator of the cost ratios.
    calibration: Option<Value>,
}

impl Run {
    fn load(path: PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let v: Value = serde_json::from_str(&text)?;
        let models = v.get("models").filter(|m| !m.is_null()).with_context(|| {
            format!(
                "{} has no embedding index: nothing to compare",
                path.display()
            )
        })?;
        let mut per_query = BTreeMap::new();
        for (mode, report) in v["results"].as_object().context("no results")? {
            let Some(queries) = report["per_query"].as_object() else {
                continue;
            };
            let values = queries
                .iter()
                .map(|(id, m)| {
                    let get = |k: &str| m[k].as_f64().unwrap_or(0.0);
                    (
                        id.clone(),
                        [get("ndcg_at_10"), get("hit_at_10"), get("mrr")],
                    )
                })
                .collect();
            per_query.insert(mode.clone(), values);
        }
        let split = v["split"]["assignment"]
            .as_object()
            .context("no split assignment")?
            .iter()
            .map(|(id, s)| (id.clone(), s.as_str().unwrap_or_default().to_string()))
            .collect();
        let long = v["long_symbols"]["queries"]
            .as_object()
            .map(|q| {
                q.iter()
                    .map(|(id, s)| (id.clone(), s.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            model: models["embedding_model"]
                .as_str()
                .unwrap_or("?")
                .to_string(),
            template: models["embedding_text_version"].as_u64().unwrap_or(0),
            chunking: models["embedding_chunking"].as_str().map(str::to_string),
            per_query,
            split,
            repos: v["repos"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(|x| x["name"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            long,
            embedding: v["embedding"].clone(),
            digest: v["dataset_digest"].as_str().unwrap_or_default().to_string(),
            calibration: calibration_of(&path),
            path,
        })
    }

    fn label(&self) -> String {
        format!(
            "{} · t{} · {}",
            self.model,
            self.template,
            if self.chunking.is_some() {
                "chunked"
            } else {
                "one vector"
            }
        )
    }

    /// Query ids in `scope` (`all`, `dev`, `held-out`, `slice`,
    /// `slice-dev`, `slice-held-out`, or a repository name), sorted.
    fn ids(&self, scope: &str) -> Vec<String> {
        let all = self.per_query.values().next().cloned().unwrap_or_default();
        all.keys()
            .filter(|id| match scope {
                "all" => true,
                "dev" | "held-out" => self.split.get(*id).map(String::as_str) == Some(scope),
                "slice" => self.long.contains_key(*id),
                "slice-dev" => self.long.get(*id).map(String::as_str) == Some("dev"),
                "slice-held-out" => self.long.get(*id).map(String::as_str) == Some("held-out"),
                repo => id.starts_with(&format!("{repo}-")),
            })
            .cloned()
            .collect()
    }

    /// Per query in `ids`: the objective (mean of semantic and hybrid nDCG@10).
    fn objective(&self, ids: &[String]) -> Result<Vec<f64>> {
        ids.iter()
            .map(|id| {
                let mut sum = 0.0;
                for mode in OBJECTIVE_MODES {
                    sum += self
                        .per_query
                        .get(mode)
                        .and_then(|q| q.get(id))
                        .with_context(|| format!("{}: {mode} lacks {id}", self.path.display()))?[0];
                }
                Ok(sum / OBJECTIVE_MODES.len() as f64)
            })
            .collect()
    }

    fn metric(&self, mode: &str, ids: &[String], which: usize) -> Option<f64> {
        let q = self.per_query.get(mode)?;
        let values: Vec<f64> = ids
            .iter()
            .filter_map(|id| q.get(id))
            .map(|v| v[which])
            .collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }

    fn cost(&self, path: &[&str]) -> Option<f64> {
        lookup(&self.embedding, path)
    }

    /// `path` in the same-runner incumbent's embedding report.
    fn calibration_cost(&self, path: &[&str]) -> Option<f64> {
        lookup(self.calibration.as_ref()?, path)
    }
}

fn lookup(mut v: &Value, path: &[&str]) -> Option<f64> {
    for key in path {
        v = v.get(key)?;
    }
    v.as_f64()
}

/// The embedding report of `calibration-<id>.json` beside `variant-<id>.json`.
fn calibration_of(path: &std::path::Path) -> Option<Value> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix("variant-")?;
    let text = std::fs::read_to_string(path.with_file_name(format!("calibration-{id}"))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(v["embedding"].clone())
}

#[derive(Serialize)]
struct VariantRow {
    report: String,
    label: String,
    objective: BTreeMap<String, f64>,
    vs_baseline: BTreeMap<String, DeltaEstimate>,
    by_repo: BTreeMap<String, BTreeMap<String, [f64; 2]>>,
    costs: BTreeMap<String, Option<f64>>,
    over_budget: Vec<String>,
}

#[derive(Serialize)]
struct SliceRow {
    label: String,
    queries: BTreeMap<String, usize>,
    chunked: BTreeMap<String, f64>,
    one_vector: BTreeMap<String, f64>,
    delta: BTreeMap<String, DeltaEstimate>,
}

#[derive(Serialize)]
struct Comparison {
    dataset_digest: String,
    baseline: String,
    variants: Vec<VariantRow>,
    long_symbol_slice: Vec<SliceRow>,
    selected: String,
    held_out_confirmation: Option<DeltaEstimate>,
    decision: String,
}

/// `--same-scores A B`: every query's scores of every mode agree between two
/// reports of the same configuration — a release-built run against a
/// debug-built one, which may differ in costs only (#157).
fn same_scores(a: &str, b: &str) -> Result<()> {
    let (a, b) = (Run::load(PathBuf::from(a))?, Run::load(PathBuf::from(b))?);
    if a.digest != b.digest || a.model != b.model || a.template != b.template {
        bail!("the two reports do not score the same configuration");
    }
    let mut compared = 0;
    for (mode, queries) in &a.per_query {
        let other = b
            .per_query
            .get(mode)
            .with_context(|| format!("{} lacks mode {mode}", b.path.display()))?;
        if queries.len() != other.len() {
            bail!("mode {mode}: {} vs {} queries", queries.len(), other.len());
        }
        for (id, values) in queries {
            let theirs = other
                .get(id)
                .with_context(|| format!("{mode}: {id} missing"))?;
            if values != theirs {
                bail!("mode {mode}, query {id}: {values:?} vs {theirs:?}");
            }
            compared += 1;
        }
    }
    println!(
        "{} and {} agree on all {compared} query scores",
        a.path.display(),
        b.path.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("--same-scores") {
        let [_, a, b] = raw.as_slice() else {
            bail!("--same-scores <report> <report>");
        };
        return same_scores(a, b);
    }
    let mut args = raw.into_iter();
    let (mut baseline, mut out, mut summary, mut others) = (None, None, None, Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => {
                baseline = Some(PathBuf::from(args.next().context("--baseline <report>")?))
            }
            "--out" => out = Some(PathBuf::from(args.next().context("--out <path>")?)),
            "--summary" => summary = Some(PathBuf::from(args.next().context("--summary <path>")?)),
            "--help" | "-h" => {
                println!("real-repo-compare --baseline <report.json> <variant.json>... [--out <json>] [--summary <md>]");
                return Ok(());
            }
            other if other.starts_with("--") => bail!("unknown argument '{other}'"),
            other => others.push(PathBuf::from(other)),
        }
    }
    let baseline = Run::load(baseline.context("--baseline is required")?)?;
    let mut runs = vec![];
    for path in others {
        let run = Run::load(path)?;
        if run.digest != baseline.digest {
            bail!(
                "{} scores another dataset than the baseline",
                run.path.display()
            );
        }
        runs.push(run);
    }

    let base_cost = |path: &[&str]| baseline.cost(path);
    let mut rows = Vec::new();
    for run in std::iter::once(&baseline).chain(&runs) {
        let mut objective = BTreeMap::new();
        let mut vs_baseline = BTreeMap::new();
        for scope in ["dev", "held-out", "all"] {
            let ids = baseline.ids(scope);
            let values = run.objective(&ids)?;
            objective.insert(scope.to_string(), round(mean(&values)));
            if !std::ptr::eq(run, &baseline) {
                vs_baseline.insert(
                    scope.to_string(),
                    rounded(paired_bootstrap_default(
                        &values,
                        &baseline.objective(&ids)?,
                    )?),
                );
            }
        }
        let mut by_repo = BTreeMap::new();
        for repo in &baseline.repos {
            let ids = baseline.ids(repo);
            let mut modes = BTreeMap::new();
            for mode in ["lexical", "semantic", "hybrid", "hybrid+rerank"] {
                if let (Some(ndcg), Some(hit)) =
                    (run.metric(mode, &ids, 0), run.metric(mode, &ids, 1))
                {
                    modes.insert(mode.to_string(), [round(ndcg), round(hit)]);
                }
            }
            by_repo.insert(repo.clone(), modes);
        }
        let mut costs = BTreeMap::new();
        for (key, path) in [
            ("embed_seconds", &["total", "embed_seconds"][..]),
            ("symbols_per_second", &["total", "symbols_per_second"][..]),
            ("texts", &["total", "texts"][..]),
            ("chunk_vectors", &["total", "chunk_vectors"][..]),
            ("vector_bytes", &["total", "vector_bytes"][..]),
            ("disk_bytes", &["total", "disk_bytes"][..]),
            ("p50_ms", &["latency_ms", "all", "p50"][..]),
            ("p95_ms", &["latency_ms", "all", "p95"][..]),
            ("peak_rss_bytes", &["peak_rss_bytes"][..]),
            ("download_mb", &["download_mb"][..]),
        ] {
            costs.insert(key.to_string(), run.cost(path));
        }
        // Same-runner ratios when the job measured the incumbent too, else
        // against the baseline job (another machine).
        let ratio = |key: &str, path: &[&str]| -> Option<f64> {
            let value = costs.get(key)?.as_ref()?;
            match run.calibration_cost(path) {
                Some(incumbent) => Some(value / incumbent),
                None => Some(value / base_cost(path)?),
            }
        };
        let time_ratio = ratio("embed_seconds", &["total", "embed_seconds"]);
        let size_ratio = ratio("vector_bytes", &["total", "vector_bytes"]);
        let rss_ratio = ratio("peak_rss_bytes", &["peak_rss_bytes"]);
        costs.insert(
            "same_runner".into(),
            Some(if run.calibration.is_some() { 1.0 } else { 0.0 }),
        );
        costs.insert("time_ratio".into(), time_ratio.map(round));
        costs.insert("size_ratio".into(), size_ratio.map(round));
        costs.insert("rss_ratio".into(), rss_ratio.map(round));
        let mut over_budget = Vec::new();
        let check = |name: &str, value: Option<f64>, limit: f64, over: &mut Vec<String>| {
            if let Some(v) = value {
                if v > limit {
                    over.push(format!("{name} {v:.2} > {limit}"));
                }
            }
        };
        check("index time ×", time_ratio, MAX_TIME_RATIO, &mut over_budget);
        check(
            "vector size ×",
            size_ratio,
            MAX_SIZE_RATIO,
            &mut over_budget,
        );
        check(
            "query p50 ms",
            costs["p50_ms"],
            MAX_P50_MS,
            &mut over_budget,
        );
        check(
            "query p95 ms",
            costs["p95_ms"],
            MAX_P95_MS,
            &mut over_budget,
        );
        check("peak RSS ×", rss_ratio, MAX_RSS_RATIO, &mut over_budget);
        check(
            "download MB",
            costs["download_mb"],
            MAX_DOWNLOAD_MB,
            &mut over_budget,
        );
        let license = run.embedding["license"].as_str().unwrap_or_default();
        if !PERMISSIVE.contains(&license) {
            over_budget.push(format!("license {license}"));
        }
        rows.push(VariantRow {
            report: run.path.display().to_string(),
            label: run.label(),
            objective,
            vs_baseline,
            by_repo,
            costs,
            over_budget,
        });
    }

    // Long-symbol slice: chunking on against off, same model and template.
    let mut slice_rows = Vec::new();
    let all_runs: Vec<&Run> = std::iter::once(&baseline).chain(&runs).collect();
    for on in all_runs.iter().filter(|r| r.chunking.is_some()) {
        let Some(off) = all_runs
            .iter()
            .find(|r| r.chunking.is_none() && r.model == on.model && r.template == on.template)
        else {
            continue;
        };
        let mut row = SliceRow {
            label: on.label(),
            queries: BTreeMap::new(),
            chunked: BTreeMap::new(),
            one_vector: BTreeMap::new(),
            delta: BTreeMap::new(),
        };
        for scope in ["slice", "slice-dev", "slice-held-out"] {
            let ids = baseline.ids(scope);
            if ids.is_empty() {
                continue;
            }
            let (a, b) = (on.objective(&ids)?, off.objective(&ids)?);
            row.queries.insert(scope.to_string(), ids.len());
            row.chunked.insert(scope.to_string(), round(mean(&a)));
            row.one_vector.insert(scope.to_string(), round(mean(&b)));
            row.delta.insert(
                scope.to_string(),
                rounded(paired_bootstrap_default(&a, &b)?),
            );
        }
        slice_rows.push(row);
    }

    // Selection on dev among in-budget variants; the incumbent wins ties.
    let mut selected = 0;
    for (i, row) in rows.iter().enumerate().skip(1) {
        if row.over_budget.is_empty() && row.objective["dev"] > rows[selected].objective["dev"] {
            selected = i;
        }
    }
    let confirmation = (selected > 0).then(|| rows[selected].vs_baseline["held-out"]);
    let decision = match confirmation {
        None => "the incumbent stays: no in-budget variant beats it on dev".to_string(),
        Some(d) if d.ci_low > 0.0 => format!(
            "{} replaces the incumbent: held-out objective {} lies wholly above zero",
            rows[selected].label,
            format_delta(&d)
        ),
        Some(d) => format!(
            "the incumbent stays: the dev winner {} does not hold on held-out ({})",
            rows[selected].label,
            format_delta(&d)
        ),
    };
    let comparison = Comparison {
        dataset_digest: baseline.digest.clone(),
        baseline: rows[0].label.clone(),
        selected: rows[selected].label.clone(),
        held_out_confirmation: confirmation,
        decision,
        variants: rows,
        long_symbol_slice: slice_rows,
    };
    let markdown = render(&comparison);
    println!("{markdown}");
    if let Some(path) = out {
        let mut json = serde_json::to_string_pretty(&comparison)?;
        json.push('\n');
        std::fs::write(&path, json)?;
    }
    if let Some(path) = summary {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(markdown.as_bytes())?;
    }
    Ok(())
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn render(c: &Comparison) -> String {
    let mut out = format!(
        "## Embedding variants (#157)\n\nBaseline: {}. Objective: per query the mean of semantic \
         and hybrid nDCG@10. Δ: paired bootstrap against the baseline, 95% CI, uncorrected.\n\n\
         | Variant | dev | held-out | all | Δ dev | Δ held-out | Budget |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | --- |\n",
        c.baseline
    );
    for row in &c.variants {
        let delta = |scope: &str| {
            row.vs_baseline
                .get(scope)
                .map(format_delta)
                .unwrap_or_else(|| "—".to_string())
        };
        out.push_str(&format!(
            "| {} | {:.4} | {:.4} | {:.4} | {} | {} | {} |\n",
            row.label,
            row.objective["dev"],
            row.objective["held-out"],
            row.objective["all"],
            delta("dev"),
            delta("held-out"),
            if row.over_budget.is_empty() {
                "within".to_string()
            } else {
                row.over_budget.join("; ")
            }
        ));
    }
    out.push_str(
        "\n**Costs** (hosted runner; × is against the incumbent indexed on the same runner \
         when the job measured it)\n\n\
         | Variant | Embed s | × | Symbols/s | Texts | Chunk vectors | Vector MB | × | Disk MB | Query p50 / p95 ms | Peak RSS GB | × | Download MB |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    let num = |v: Option<f64>, scale: f64, digits: usize| {
        v.map(|v| format!("{:.*}", digits, v / scale))
            .unwrap_or_else(|| "—".to_string())
    };
    for row in &c.variants {
        let k = |key: &str| row.costs.get(key).copied().flatten();
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} | {} | {} |\n",
            row.label,
            num(k("embed_seconds"), 1.0, 0),
            num(k("time_ratio"), 1.0, 2),
            num(k("symbols_per_second"), 1.0, 1),
            num(k("texts"), 1.0, 0),
            num(k("chunk_vectors"), 1.0, 0),
            num(k("vector_bytes"), 1e6, 1),
            num(k("size_ratio"), 1.0, 2),
            num(k("disk_bytes"), 1e6, 1),
            num(k("p50_ms"), 1.0, 1),
            num(k("p95_ms"), 1.0, 1),
            num(k("peak_rss_bytes"), 1e9, 2),
            num(k("rss_ratio"), 1.0, 2),
            num(k("download_mb"), 1.0, 0),
        ));
    }
    out.push_str(
        "\n**nDCG@10 / hit@10 by repository and mode, all queries**\n\n| Variant | Repository |",
    );
    let modes = ["lexical", "semantic", "hybrid", "hybrid+rerank"];
    for m in modes {
        out.push_str(&format!(" `{m}` |"));
    }
    out.push_str("\n| --- | --- |");
    for _ in modes {
        out.push_str(" ---: |");
    }
    out.push('\n');
    for row in &c.variants {
        for (repo, by_mode) in &row.by_repo {
            out.push_str(&format!("| {} | {repo} |", row.label));
            for m in modes {
                match by_mode.get(m) {
                    Some([ndcg, hit]) => out.push_str(&format!(" {ndcg:.3} / {hit:.3} |")),
                    None => out.push_str(" — |"),
                }
            }
            out.push('\n');
        }
    }
    if !c.long_symbol_slice.is_empty() {
        out.push_str(
            "\n**Long-symbol slice: chunking on − off, same model and template (objective)**\n\n\
             | Variant | Queries (all / dev / held-out) | Chunked | One vector | Δ all | Δ dev | Δ held-out |\n\
             | --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
        );
        for row in &c.long_symbol_slice {
            let q = |s: &str| row.queries.get(s).copied().unwrap_or(0);
            let d = |s: &str| {
                row.delta
                    .get(s)
                    .map(format_delta)
                    .unwrap_or_else(|| "—".to_string())
            };
            out.push_str(&format!(
                "| {} | {} / {} / {} | {:.4} | {:.4} | {} | {} | {} |\n",
                row.label,
                q("slice"),
                q("slice-dev"),
                q("slice-held-out"),
                row.chunked.get("slice").copied().unwrap_or(0.0),
                row.one_vector.get("slice").copied().unwrap_or(0.0),
                d("slice"),
                d("slice-dev"),
                d("slice-held-out"),
            ));
        }
    }
    out.push_str(&format!(
        "\n**Selected on dev (in budget):** {}.\n\n**Decision:** {}.\n",
        c.selected, c.decision
    ));
    out
}
