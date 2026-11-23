//! The real-repository CALLS sets: call sites drawn from pinned open-source
//! repositories and traced to their targets by reading the code
//! (`graph-gold/held-out/`). They are reported next to the fixture gate and
//! never gated: the pairs are few, and the labels are an agent's.
//!
//! - `real-repos.json` (#139, held out): 52 sites drawn at random from flask,
//!   requests, gin and hono.
//! - `java-spring-heldout.json` (#177, held out): 60 sites drawn at random
//!   from spring-framework after the #177 resolver changes were frozen. The
//!   headline Java numbers.
//! - `java-spring-dev.json` (#177, **not** held out): 105 sites of
//!   spring-framework, 60 at random and 5 per hard Java case (interface
//!   dispatch, overloads, generics, inner and anonymous classes, lambdas,
//!   method references, static imports, injected fields, library base
//!   classes). The #177 resolver fixes were made while reading its misses,
//!   so its scores are development numbers. Its random stratum is the
//!   headline row; the targeted strata are reported apart.
//!
//! A set's `role` says which it is. Each pair may name its cases and its
//! stratum; the report gives precision and recall per case and per stratum,
//! and the share of all gold targets reached ("targets reached": for a
//! dispatch call, how many of the implementations it may run got an edge).
//!
//! # Scoring
//!
//! Per (caller, callee name) pair. The caller is the extracted symbol in the
//! pair's file with the caller's name whose lines contain the caller's line
//! (the tightest). The pair's *edges* are the CALLS edges from it to symbols
//! named like the callee. An edge is correct when its target is one of the
//! pair's targets (same file and name, lines containing the target's line).
//!
//! - precision: correct edges / edges, over all pairs (a pair whose call
//!   has no project target makes every edge of it wrong);
//! - recall: pairs with a project target that got a correct edge / pairs
//!   with a project target. A caller myceliums did not extract is a miss.

use anyhow::{bail, Context, Result};
use myceliums_storage::CodeSymbol;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{extract, Extraction};
use super::report::wilson;

/// The sets, as committed.
const SET: &str = include_str!("../../graph-gold/held-out/real-repos.json");
const JAVA_SET: &str = include_str!("../../graph-gold/held-out/java-spring-heldout.json");
const JAVA_DEV_SET: &str = include_str!("../../graph-gold/held-out/java-spring-dev.json");

#[derive(Debug, Clone, Deserialize)]
pub struct HeldOutSet {
    /// The file it was read from (`held-out/real-repos.json`).
    #[serde(skip)]
    pub name: String,
    pub description: String,
    /// `held-out` (default) or `dev`: a set the resolver was changed against.
    #[serde(default)]
    pub role: Option<String>,
    pub labelled_by: String,
    pub reviewed_by: Option<String>,
    pub rules: String,
    /// Repository name → pinned commit.
    pub repos: BTreeMap<String, String>,
    /// How to fetch the checkouts (default: `real-repos/fetch.sh`).
    #[serde(default)]
    pub fetch: Option<String>,
    /// Case name → what it means, for a set whose pairs name cases.
    #[serde(default)]
    pub cases: BTreeMap<String, String>,
    /// Cases whose pairs are reported per case but left out of the totals
    /// (Java method references: not calls under the fixture rules).
    #[serde(default)]
    pub report_only: Vec<String>,
    pub pairs: Vec<Pair>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pair {
    pub repo: String,
    pub file: String,
    pub caller: String,
    pub caller_line: u32,
    pub call_line: u32,
    pub callee: String,
    pub targets: Vec<Target>,
    pub why: String,
    /// The stratum of the draw that produced the site (`random` or a case).
    #[serde(default)]
    pub stratum: Option<String>,
    /// The hard cases the pair is an instance of (empty: none).
    #[serde(default)]
    pub cases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub file: String,
    pub name: String,
    pub line: u32,
}

impl HeldOutSet {
    /// The four-repository set (#139).
    pub fn embedded() -> Result<Self> {
        Self::parse(SET, "held-out/real-repos.json")
    }

    /// The held-out spring-framework Java set (#177).
    pub fn java() -> Result<Self> {
        Self::parse(JAVA_SET, "held-out/java-spring-heldout.json")
    }

    /// The spring-framework Java dev set (#177): not held out.
    pub fn java_dev() -> Result<Self> {
        Self::parse(JAVA_DEV_SET, "held-out/java-spring-dev.json")
    }

    /// Every committed set, in report order.
    pub fn all() -> Result<Vec<Self>> {
        Ok(vec![Self::embedded()?, Self::java()?, Self::java_dev()?])
    }

    /// Whether the resolver was changed against this set.
    pub fn is_dev(&self) -> bool {
        self.role.as_deref() == Some("dev")
    }

    /// Whether the pairs come from more than one stratum (random and
    /// targeted): then the headline is the random stratum alone.
    pub fn stratified(&self) -> bool {
        self.pairs
            .iter()
            .any(|p| p.stratum.as_deref().is_some_and(|s| s != RANDOM))
    }

    fn parse(json: &str, name: &str) -> Result<Self> {
        let mut set: HeldOutSet =
            serde_json::from_str(json).with_context(|| format!("parsing {name}"))?;
        set.name = name.to_string();
        for pair in &set.pairs {
            if !set.repos.contains_key(&pair.repo) {
                bail!(
                    "pair {}::{} names unpinned repository {}",
                    pair.file,
                    pair.caller,
                    pair.repo
                );
            }
            if let Some(case) = pair.cases.iter().find(|c| !set.cases.contains_key(*c)) {
                bail!(
                    "pair {}::{} names undefined case {case} ({name})",
                    pair.file,
                    pair.caller
                );
            }
        }
        Ok(set)
    }

    /// Whether `pair` counts towards the totals.
    pub fn in_totals(&self, pair: &Pair) -> bool {
        !pair.cases.iter().any(|c| self.report_only.contains(c))
    }
}

/// The stratum of uniformly drawn sites.
pub const RANDOM: &str = "random";

fn is_random(pair: &Pair) -> bool {
    pair.stratum.as_deref().is_none_or(|s| s == RANDOM)
}

/// One repository's scores.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoScore {
    pub repo: String,
    pub pairs: usize,
    pub with_target: usize,
    pub found: usize,
    pub edges: usize,
    pub correct: usize,
    /// Pairs whose caller myceliums did not extract.
    pub callers_missing: usize,
    pub misses: Vec<String>,
    pub wrong: Vec<String>,
    /// Gold targets of the pairs with a project target, and how many of them
    /// got an edge: for a dispatch call, the implementations it may run.
    pub targets: usize,
    pub targets_reached: usize,
    /// Per case (`repo` holds the case name), for a set whose pairs name
    /// cases; the last entry, `(no hard case)`, holds the pairs with none.
    pub cases: Vec<RepoScore>,
    /// For a stratified set: the random stratum, then the targeted ones
    /// together (`repo` holds the stratum's name).
    pub strata: Vec<RepoScore>,
}

/// The whole run: scored repositories, and the ones that could not be.
#[derive(Debug, Clone, Default)]
pub struct HeldOutReport {
    pub repos: Vec<RepoScore>,
    pub skipped: Vec<String>,
}

/// The checkout directory: `$MYC_EVAL_REPOS_DIR`, else
/// `${XDG_CACHE_HOME:-$HOME/.cache}/myceliums-eval-repos`, as
/// `real-repos/fetch.sh` uses it.
pub fn repos_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("MYC_EVAL_REPOS_DIR") {
        return Some(dir.into());
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))?;
    Some(cache.join("myceliums-eval-repos"))
}

/// Analyze every pinned repository found at `repos_dir` and score it.
/// A repository that is missing or at another commit is skipped, with the
/// reason.
pub async fn run(set: &HeldOutSet, repos_dir: &Path) -> Result<HeldOutReport> {
    let mut report = HeldOutReport::default();
    for (repo, pin) in &set.repos {
        let dir = repos_dir.join(repo);
        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
        match head {
            None => {
                report
                    .skipped
                    .push(format!("{repo}: no checkout at {}", dir.display()));
                continue;
            }
            Some(head) if head != *pin => {
                report
                    .skipped
                    .push(format!("{repo}: checkout at {head}, not the pinned {pin}"));
                continue;
            }
            Some(_) => {}
        }
        let extraction = extract(&dir).await?;
        let pairs: Vec<&Pair> = set.pairs.iter().filter(|p| p.repo == *repo).collect();
        report.repos.push(score_set(set, repo, &pairs, &extraction));
    }
    Ok(report)
}

/// Score one repository's pairs: the totals over the pairs that count, and
/// one score per case (every pair of that case, report-only ones included).
pub fn score_set(
    set: &HeldOutSet,
    repo: &str,
    pairs: &[&Pair],
    extraction: &Extraction,
) -> RepoScore {
    // A stratified set's headline is its random stratum: the targeted
    // strata over-represent the hard cases.
    let stratified = set.stratified();
    let counted: Vec<&Pair> = pairs
        .iter()
        .copied()
        .filter(|p| set.in_totals(p) && (!stratified || is_random(p)))
        .collect();
    let mut total = score(repo, &counted, extraction);
    if stratified {
        let targeted: Vec<&Pair> = pairs
            .iter()
            .copied()
            .filter(|p| set.in_totals(p) && !is_random(p))
            .collect();
        total.strata.push(score(RANDOM, &counted, extraction));
        total
            .strata
            .push(score("targeted (case strata)", &targeted, extraction));
    }
    if !set.cases.is_empty() {
        for case in set.cases.keys() {
            let of_case: Vec<&Pair> = pairs
                .iter()
                .copied()
                .filter(|p| p.cases.contains(case))
                .collect();
            total.cases.push(score(case, &of_case, extraction));
        }
        let plain: Vec<&Pair> = pairs
            .iter()
            .copied()
            .filter(|p| p.cases.is_empty())
            .collect();
        total
            .cases
            .push(score("(no hard case)", &plain, extraction));
    }
    total
}

/// Score one repository's pairs against what was extracted from it.
pub fn score(repo: &str, pairs: &[&Pair], extraction: &Extraction) -> RepoScore {
    let by_uid: HashMap<&str, &CodeSymbol> = extraction
        .symbols
        .iter()
        .map(|s| (s.uid.as_str(), s))
        .collect();
    let mut score = RepoScore {
        repo: repo.to_string(),
        ..RepoScore::default()
    };
    for pair in pairs {
        score.pairs += 1;
        let has_target = !pair.targets.is_empty();
        score.with_target += usize::from(has_target);
        let caller = extraction
            .symbols
            .iter()
            .filter(|s| {
                s.file_path == pair.file
                    && s.name == pair.caller
                    && s.start_line <= pair.caller_line
                    && pair.caller_line <= s.end_line
            })
            .min_by_key(|s| s.end_line - s.start_line);
        let Some(caller) = caller else {
            score.callers_missing += 1;
            score.targets += pair.targets.len();
            if has_target {
                score.misses.push(format!(
                    "{}::{} → `{}` (caller not extracted)",
                    pair.file, pair.caller, pair.callee
                ));
            }
            continue;
        };
        let mut found = false;
        let mut reached = vec![false; pair.targets.len()];
        for edge in extraction
            .calls
            .iter()
            .filter(|r| r.source_uid == caller.uid)
        {
            let Some(target) = by_uid.get(edge.target_uid.as_str()) else {
                continue;
            };
            if target.name != pair.callee {
                continue;
            }
            score.edges += 1;
            let hits = |t: &Target| {
                t.file == target.file_path
                    && t.name == target.name
                    && target.start_line <= t.line
                    && t.line <= target.end_line
            };
            let correct = pair.targets.iter().any(hits);
            for (i, t) in pair.targets.iter().enumerate() {
                reached[i] |= hits(t);
            }
            if correct {
                score.correct += 1;
                found = true;
            } else {
                score.wrong.push(format!(
                    "{}::{} → {}::{} (line {})",
                    pair.file, pair.caller, target.file_path, target.qualified_name, pair.call_line
                ));
            }
        }
        score.targets += pair.targets.len();
        score.targets_reached += reached.iter().filter(|r| **r).count();
        if has_target && found {
            score.found += 1;
        } else if has_target {
            score.misses.push(format!(
                "{}::{} → `{}` (line {})",
                pair.file, pair.caller, pair.callee, pair.call_line
            ));
        }
    }
    score
}

fn rate(part: usize, whole: usize) -> String {
    match wilson(part, whole) {
        Some((low, high)) => format!("{:.2} ({low:.2}–{high:.2})", part as f64 / whole as f64),
        None => "n/a".to_string(),
    }
}

/// The report as Markdown.
pub fn markdown(set: &HeldOutSet, report: &HeldOutReport) -> String {
    let mut out = String::new();
    let role = if set.is_dev() {
        "dev set, not held out: the resolver was changed against it"
    } else {
        "held out"
    };
    let _ = writeln!(
        out,
        "\n### Real repositories: `{}` ({role}; report only, not gated)\n",
        set.name
    );
    if set.stratified() {
        let _ = writeln!(
            out,
            "Stratified: the table below is the `random` stratum alone; the targeted strata follow.\n"
        );
    }
    let _ = writeln!(
        out,
        "{} call sites drawn from {} pinned repositories and traced by reading the code \
         (`graph-gold/held-out/`; labelled by `{}`, reviewed by `{}`). Per (caller, callee name) pair: \
         precision over the edges drawn from those callers to symbols of that name, recall over the \
         pairs with a project target.\n",
        set.pairs.len(),
        set.repos.len(),
        set.labelled_by,
        set.reviewed_by.as_deref().unwrap_or("nobody yet"),
    );
    let fetch = set
        .fetch
        .as_deref()
        .unwrap_or("crates/myceliums-benchmarks/real-repos/fetch.sh");
    for skipped in &report.skipped {
        let _ = writeln!(out, "Skipped — {skipped} (fetch with `{fetch}`).\n");
    }
    if report.repos.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "| repository | pairs | with a project target | found | recall (95% CI) | edges | correct | precision (95% CI) | callers not extracted |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    let mut total = RepoScore {
        repo: "**all**".to_string(),
        ..RepoScore::default()
    };
    for s in &report.repos {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.repo,
            s.pairs,
            s.with_target,
            s.found,
            rate(s.found, s.with_target),
            s.edges,
            s.correct,
            rate(s.correct, s.edges),
            s.callers_missing
        );
        total.pairs += s.pairs;
        total.with_target += s.with_target;
        total.found += s.found;
        total.edges += s.edges;
        total.correct += s.correct;
        total.callers_missing += s.callers_missing;
    }
    let _ = writeln!(
        out,
        "| **all** | {} | {} | {} | {} | {} | {} | {} | {} |",
        total.pairs,
        total.with_target,
        total.found,
        rate(total.found, total.with_target),
        total.edges,
        total.correct,
        rate(total.correct, total.edges),
        total.callers_missing
    );
    if set.stratified() {
        out.push_str(&breakdown(
            "\nPer stratum (the targeted strata draw the hard cases on purpose):\n",
            "stratum",
            &[RANDOM, "targeted (case strata)"],
            report.repos.iter().flat_map(|r| &r.strata),
        ));
    }
    if !set.cases.is_empty() {
        let mut names: Vec<&str> = set.cases.keys().map(String::as_str).collect();
        names.push("(no hard case)");
        let title = format!(
            "\nPer case, all strata (a pair can be several cases; {} pairs are reported here only, not in the totals). \
             \"Targets reached\": gold targets with an edge — for dispatch, the implementations a call may run:\n",
            if set.report_only.is_empty() {
                "no".to_string()
            } else {
                set.report_only.join(", ")
            }
        );
        out.push_str(&breakdown(
            &title,
            "case",
            &names,
            report.repos.iter().flat_map(|r| &r.cases),
        ));
    }
    let misses: Vec<&String> = report.repos.iter().flat_map(|s| &s.misses).collect();
    let wrong: Vec<&String> = report.repos.iter().flat_map(|s| &s.wrong).collect();
    if !misses.is_empty() {
        let _ = writeln!(out, "\nMissed: {}", list(&misses));
    }
    if !wrong.is_empty() {
        let _ = writeln!(out, "\nWrong edges: {}", list(&wrong));
    }
    for case in &set.report_only {
        let scores: Vec<&RepoScore> = report
            .repos
            .iter()
            .flat_map(|r| &r.cases)
            .filter(|c| &c.repo == case)
            .collect();
        let misses: Vec<&String> = scores.iter().flat_map(|s| &s.misses).collect();
        let wrong: Vec<&String> = scores.iter().flat_map(|s| &s.wrong).collect();
        if !misses.is_empty() {
            let _ = writeln!(out, "\nMissed ({case}, report only): {}", list(&misses));
        }
        if !wrong.is_empty() {
            let _ = writeln!(out, "\nWrong edges ({case}, report only): {}", list(&wrong));
        }
    }
    out
}

/// Precision, recall and targets reached per named group (case or
/// stratum), summed over the repositories.
fn breakdown<'a>(
    title: &str,
    column: &str,
    names: &[&str],
    scores: impl Iterator<Item = &'a RepoScore> + Clone,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "| {column} | pairs | with a project target | found | recall (95% CI) | edges | correct | precision (95% CI) | targets reached |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for &name in names {
        let mut sum = RepoScore::default();
        for s in scores.clone().filter(|c| c.repo == name) {
            sum.pairs += s.pairs;
            sum.with_target += s.with_target;
            sum.found += s.found;
            sum.edges += s.edges;
            sum.correct += s.correct;
            sum.targets += s.targets;
            sum.targets_reached += s.targets_reached;
        }
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} |",
            sum.pairs,
            sum.with_target,
            sum.found,
            rate(sum.found, sum.with_target),
            sum.edges,
            sum.correct,
            rate(sum.correct, sum.edges),
            rate(sum.targets_reached, sum.targets),
        );
    }
    out
}

fn list(items: &[&String]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{Relationship, RelationshipKind, SymbolKind};

    #[test]
    fn the_set_is_balanced_and_pinned_like_the_retrieval_set() {
        let set = HeldOutSet::embedded().unwrap();
        assert_eq!(set.pairs.len(), 52);
        for repo in set.repos.keys() {
            assert_eq!(
                set.pairs.iter().filter(|p| &p.repo == repo).count(),
                13,
                "{repo}"
            );
        }
        // fetch.sh fetches the retrieval set's pins: they must be the same.
        let queries: serde_json::Value =
            serde_json::from_str(include_str!("../../real-repos/queries.json")).unwrap();
        for repo in queries["repos"].as_array().unwrap() {
            let name = repo["name"].as_str().unwrap();
            assert_eq!(set.repos[name], repo["commit"].as_str().unwrap(), "{name}");
        }
        assert!(set.pairs.iter().all(|p| p.caller_line <= p.call_line));
    }

    /// #177: the held-out Java set is 60 sites of the random stratum only,
    /// none of them a pair of the dev set, at the dev set's pinned release.
    #[test]
    fn the_java_held_out_set_is_random_and_disjoint_from_the_dev_set() {
        let set = HeldOutSet::java().unwrap();
        let dev = HeldOutSet::java_dev().unwrap();
        assert!(!set.is_dev() && dev.is_dev());
        assert!(!set.stratified() && dev.stratified());
        assert_eq!(set.repos, dev.repos);
        assert_eq!(set.pairs.len(), 60);
        assert!(set.report_only.is_empty());
        assert!(set
            .pairs
            .iter()
            .all(|p| p.stratum.as_deref() == Some(RANDOM) && set.in_totals(p)));
        for pair in &set.pairs {
            assert!(
                !dev.pairs.iter().any(|d| d.file == pair.file
                    && d.caller == pair.caller
                    && d.callee == pair.callee),
                "{}::{} is in the dev set",
                pair.file,
                pair.caller
            );
            assert!(pair.caller_line <= pair.call_line);
        }
    }

    /// #177: the dev set — 60 random sites and 5 per case stratum, every
    /// case represented by at least five pairs, one pinned release.
    #[test]
    fn the_java_dev_set_covers_every_case() {
        let set = HeldOutSet::java_dev().unwrap();
        assert_eq!(set.repos.len(), 1);
        assert_eq!(set.labelled_by, "agent");
        let counted = set.pairs.iter().filter(|p| set.in_totals(p)).count();
        assert!(counted >= 40, "{counted} scored pairs");
        let stratum = |name: &str| {
            set.pairs
                .iter()
                .filter(|p| p.stratum.as_deref() == Some(name))
                .count()
        };
        assert_eq!(stratum("random"), 60);
        for case in set.cases.keys() {
            assert_eq!(stratum(case), 5, "{case}");
            assert!(
                set.pairs.iter().filter(|p| p.cases.contains(case)).count() >= 5,
                "{case}"
            );
            // A case stratum's pairs are instances of that case.
            assert!(set
                .pairs
                .iter()
                .filter(|p| p.stratum.as_deref() == Some(case))
                .all(|p| p.cases.contains(case)));
        }
        assert_eq!(set.report_only, ["method-ref"]);
        assert!(set.pairs.iter().all(|p| p.caller_line <= p.call_line));
        let mut sites: Vec<(&str, &str, u32)> = set
            .pairs
            .iter()
            .map(|p| (p.file.as_str(), p.callee.as_str(), p.caller_line))
            .collect();
        sites.sort();
        sites.dedup();
        assert_eq!(sites.len(), set.pairs.len(), "a pair is listed twice");
    }

    fn symbol(uid: &str, file: &str, name: &str, lines: (u32, u32)) -> CodeSymbol {
        CodeSymbol {
            uid: uid.into(),
            name: name.into(),
            qualified_name: name.into(),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: lines.0,
            end_line: lines.1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    fn call(from: &str, to: &str) -> Relationship {
        Relationship {
            uid: format!("{from}->{to}"),
            source_uid: from.into(),
            target_uid: to.into(),
            kind: RelationshipKind::Calls,
            repo_id: "r".into(),
            metadata: "line:1;resolved:exact".into(),
        }
    }

    #[test]
    fn pairs_are_scored_by_caller_and_callee_name() {
        let pair = |caller: &str, callee: &str, targets: Vec<Target>| Pair {
            repo: "r".into(),
            file: "a.py".into(),
            caller: caller.into(),
            caller_line: 1,
            call_line: 2,
            callee: callee.into(),
            targets,
            why: String::new(),
            stratum: None,
            cases: Vec::new(),
        };
        let target = |name: &str| Target {
            file: "b.py".into(),
            name: name.into(),
            line: 1,
        };
        let pairs = [
            pair("main", "parse", vec![target("parse")]),
            pair("main", "get", vec![]),
            pair("gone", "parse", vec![target("parse")]),
        ];
        let extraction = Extraction {
            symbols: vec![
                symbol("main", "a.py", "main", (1, 9)),
                symbol("parse", "b.py", "parse", (1, 2)),
                symbol("get", "b.py", "get", (4, 5)),
                symbol("other", "b.py", "other", (7, 8)),
            ],
            calls: vec![
                call("main", "parse"),
                call("main", "get"),
                call("main", "other"),
            ],
        };
        let refs: Vec<&Pair> = pairs.iter().collect();
        let score = score("r", &refs, &extraction);
        assert_eq!(
            (
                score.pairs,
                score.with_target,
                score.found,
                score.edges,
                score.correct,
                score.callers_missing
            ),
            (3, 2, 1, 2, 1, 1),
            "{score:?}"
        );
        assert_eq!(score.wrong.len(), 1, "get has no project target");
        assert_eq!(score.misses.len(), 1, "gone was not extracted");

        assert_eq!((score.targets, score.targets_reached), (2, 1));

        // Per case, with report-only cases left out of the totals.
        let mut set = HeldOutSet::java_dev().unwrap();
        set.cases = BTreeMap::from([
            ("overload".to_string(), String::new()),
            ("method-ref".to_string(), String::new()),
        ]);
        let mut cased = pairs.clone();
        cased[0].cases = vec!["overload".into()];
        cased[1].cases = vec!["method-ref".into()];
        let refs: Vec<&Pair> = cased.iter().collect();
        let total = score_set(&set, "r", &refs, &extraction);
        assert_eq!((total.pairs, total.edges, total.correct), (2, 1, 1));
        let case = |name: &str| total.cases.iter().find(|c| c.repo == name).unwrap();
        assert_eq!((case("overload").pairs, case("overload").found), (1, 1));
        assert_eq!((case("method-ref").pairs, case("method-ref").edges), (1, 1));
        assert_eq!(case("(no hard case)").pairs, 1);

        // A stratified set: the headline is the random stratum alone.
        cased[1].cases.clear();
        cased[0].stratum = Some("overload".into());
        let refs: Vec<&Pair> = cased.iter().collect();
        let total = score_set(&set, "r", &refs, &extraction);
        assert_eq!((total.pairs, total.found), (2, 0), "{total:?}");
        let stratum = |name: &str| total.strata.iter().find(|c| c.repo == name).unwrap();
        assert_eq!(stratum(RANDOM).pairs, 2);
        assert_eq!(
            (
                stratum("targeted (case strata)").pairs,
                stratum("targeted (case strata)").found
            ),
            (1, 1)
        );
    }

    /// A dispatch pair with three gold targets, one linked: found, and one
    /// of three targets reached.
    #[test]
    fn targets_reached_counts_each_gold_target() {
        let target = |line: u32| Target {
            file: "b.java".into(),
            name: "run".into(),
            line,
        };
        let pair = Pair {
            repo: "r".into(),
            file: "a.java".into(),
            caller: "main".into(),
            caller_line: 1,
            call_line: 2,
            callee: "run".into(),
            targets: vec![target(1), target(4), target(7)],
            why: String::new(),
            stratum: None,
            cases: vec!["dispatch".into()],
        };
        let extraction = Extraction {
            symbols: vec![
                symbol("main", "a.java", "main", (1, 9)),
                symbol("iface", "b.java", "run", (1, 2)),
                symbol("impl", "b.java", "run", (4, 5)),
            ],
            calls: vec![call("main", "iface")],
        };
        let score = score("r", &[&pair], &extraction);
        assert_eq!(
            (score.found, score.targets, score.targets_reached),
            (1, 3, 1)
        );
    }
}
