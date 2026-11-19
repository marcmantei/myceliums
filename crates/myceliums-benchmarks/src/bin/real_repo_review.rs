//! Review the agent-drafted labels of the real-repo set, one query at a time.
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin real-repo-review -- --reviewer <your-handle>
//! cargo run -p myceliums-benchmarks --bin real-repo-review -- --reviewer marc --repo gin
//! cargo run -p myceliums-benchmarks --bin real-repo-review -- --show gin-paraphrase-stop-remaining-handlers
//! ```
//!
//! For every query not yet reviewed it prints the query, its intent and split,
//! the labelling rationale, and each labelled symbol with its reason and the
//! symbol's source from the pinned checkout. Answer:
//!
//! - `y` — the labels are right: `reviewed_by` becomes `human:<handle>` and the
//!   dataset is saved at once, so quitting loses nothing;
//! - `n` — something is wrong: type a note; the query stays unreviewed and is
//!   listed at the end with your note, for you to fix by hand;
//! - `s` — skip for now; `q` — stop.
//!
//! Reviewing changes the dataset file and therefore its digest; re-run
//! `real-repo-eval --record` afterwards so the recorded numbers name the
//! reviewed labels.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::eval::corpus::Corpus;
use myceliums_benchmarks::eval::real_repos::{
    repos_dir, reviewer_identity, verify_checkout, RealQuery, RealRepoSet,
};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Source lines shown per labelled symbol before the snippet is cut.
const SNIPPET_LINES: usize = 40;

fn main() -> Result<()> {
    let mut reviewer: Option<String> = None;
    let mut repo: Option<String> = None;
    let mut show: Option<String> = None;
    let mut dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reviewer" => reviewer = Some(args.next().context("--reviewer needs a handle")?),
            "--repo" => repo = Some(args.next().context("--repo needs a name")?),
            "--show" => show = Some(args.next().context("--show needs a query id")?),
            "--repos-dir" => {
                dir = Some(PathBuf::from(
                    args.next().context("--repos-dir needs a path")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "real-repo-review --reviewer <handle> [--repo <name>] [--repos-dir <dir>]\n\
                     real-repo-review --show <query-id>"
                );
                return Ok(());
            }
            other => bail!("unknown argument '{other}' (try --help)"),
        }
    }

    let path = RealRepoSet::default_path();
    let mut set = RealRepoSet::from_path(&path)?;
    let split = set.split();
    let dir = match dir {
        Some(dir) => dir,
        None => repos_dir()?,
    };
    let mut sources = Sources::new(dir);

    if let Some(id) = show {
        let query = set
            .queries
            .iter()
            .find(|q| q.id == id)
            .with_context(|| format!("no query '{id}'"))?
            .clone();
        print_query(&set, &query, split[&id].label(), &mut sources)?;
        return Ok(());
    }

    let reviewer = reviewer_identity(
        &reviewer.context("--reviewer <handle> is required: a review records who made it")?,
    )?;
    let pending: Vec<RealQuery> = set
        .queries
        .iter()
        .filter(|q| !q.is_reviewed())
        .filter(|q| repo.as_deref().is_none_or(|r| q.repo == r))
        .cloned()
        .collect();
    println!(
        "{} queries to review ({} of {} reviewed so far) — answer y / n / s / q\n",
        pending.len(),
        set.reviewed(),
        set.queries.len()
    );

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut rejected: Vec<(String, String)> = Vec::new();
    let mut accepted = 0;
    for (n, query) in pending.iter().enumerate() {
        println!("── {}/{} ──", n + 1, pending.len());
        print_query(&set, query, split[&query.id].label(), &mut sources)?;
        let answer = loop {
            print!("labels correct? [y/n/s/q] ");
            std::io::stdout().flush()?;
            let Some(line) = lines.next() else {
                break "q".to_string();
            };
            let line = line?.trim().to_lowercase();
            if ["y", "n", "s", "q"].contains(&line.as_str()) {
                break line;
            }
        };
        match answer.as_str() {
            "y" => {
                set.record_review(&query.id, &reviewer)?;
                save(&path, &set)?;
                accepted += 1;
            }
            "n" => {
                print!("what is wrong? ");
                std::io::stdout().flush()?;
                let note = lines.next().transpose()?.unwrap_or_default();
                rejected.push((query.id.clone(), note));
            }
            "q" => break,
            _ => {}
        }
        println!();
    }

    println!(
        "{accepted} accepted as {reviewer}; dataset saved to {}",
        path.display()
    );
    if !rejected.is_empty() {
        println!("\nto fix by hand in {} (still unreviewed):", path.display());
        for (id, note) in rejected {
            println!("  {id}: {note}");
        }
    }
    if accepted > 0 {
        println!(
            "\nre-run `real-repo-eval --record` so the recorded run names the reviewed labels"
        );
    }
    Ok(())
}

/// Write the dataset atomically: to a temporary file next to it, then renamed
/// over it, so an interrupted save never leaves a truncated dataset behind.
fn save(path: &std::path::Path, set: &RealRepoSet) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, set.to_json()?)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

/// `(file, qualified name)` → the line ranges of every symbol with that pair.
type LineRanges = HashMap<(String, String), Vec<(u32, u32)>>;

/// Parsed checkouts, loaded on first use.
struct Sources {
    dir: PathBuf,
    /// repo → (file, qualified name) → line ranges.
    ranges: HashMap<String, LineRanges>,
}

impl Sources {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            ranges: HashMap::new(),
        }
    }

    fn ranges(&mut self, set: &RealRepoSet, repo: &str) -> Result<&LineRanges> {
        if !self.ranges.contains_key(repo) {
            let pin = set.repo(repo).context("unknown repository")?;
            let root = verify_checkout(&self.dir, pin)?;
            let corpus = Corpus::load_repository(&root, repo)?;
            let mut map = LineRanges::new();
            for symbol in corpus.symbols() {
                map.entry((symbol.file_path.clone(), symbol.qualified_name.clone()))
                    .or_default()
                    .push((symbol.start_line, symbol.end_line));
            }
            self.ranges.insert(repo.to_string(), map);
        }
        Ok(&self.ranges[repo])
    }
}

fn print_query(
    set: &RealRepoSet,
    query: &RealQuery,
    split: &str,
    sources: &mut Sources,
) -> Result<()> {
    println!(
        "{}  [{} · {} · {split}]",
        query.id,
        query.repo,
        query.intent.label()
    );
    println!("query:     {}", query.query);
    println!("rationale: {}", query.rationale);
    println!("labelled by {}", query.labelled_by);
    let root = sources.dir.join(&query.repo);
    for (i, label) in query.relevant.iter().enumerate() {
        println!(
            "\n  [{}/{}] {}::{}",
            i + 1,
            query.relevant.len(),
            label.file,
            label.symbol
        );
        println!("  why: {}", label.why);
        let ranges = sources
            .ranges(set, &query.repo)?
            .get(&(label.file.clone(), label.symbol.clone()))
            .cloned()
            .unwrap_or_default();
        if ranges.is_empty() {
            println!("  !! not in the parsed checkout");
            continue;
        }
        let text = std::fs::read_to_string(root.join(&label.file))?;
        let lines: Vec<&str> = text.lines().collect();
        for (start, end) in ranges {
            let first = (start as usize).saturating_sub(1);
            let last = (end as usize).min(lines.len());
            for (offset, line) in lines[first..last].iter().take(SNIPPET_LINES).enumerate() {
                println!("  {:>5} │ {line}", first + offset + 1);
            }
            if last - first > SNIPPET_LINES {
                println!("        │ … {} more lines", last - first - SNIPPET_LINES);
            }
        }
    }
    println!();
    Ok(())
}
