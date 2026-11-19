//! The held-out real-repository CALLS set (#139): call sites drawn at random
//! from four pinned open-source repositories and traced to their targets by
//! reading the code (`graph-gold/held-out/`). The resolver was not developed
//! against them, so this is the outside check on the fixture numbers. It is
//! reported next to the fixture gate and never gated: 52 pairs are too few,
//! and the labels are an agent's, not yet reviewed.
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

/// The set, as committed.
const SET: &str = include_str!("../../graph-gold/held-out/real-repos.json");

#[derive(Debug, Clone, Deserialize)]
pub struct HeldOutSet {
    pub description: String,
    pub labelled_by: String,
    pub reviewed_by: Option<String>,
    pub rules: String,
    /// Repository name → pinned commit.
    pub repos: BTreeMap<String, String>,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub file: String,
    pub name: String,
    pub line: u32,
}

impl HeldOutSet {
    pub fn embedded() -> Result<Self> {
        let set: HeldOutSet =
            serde_json::from_str(SET).context("parsing held-out/real-repos.json")?;
        for pair in &set.pairs {
            if !set.repos.contains_key(&pair.repo) {
                bail!(
                    "pair {}::{} names unpinned repository {}",
                    pair.file,
                    pair.caller,
                    pair.repo
                );
            }
        }
        Ok(set)
    }
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
        report.repos.push(score(repo, &pairs, &extraction));
    }
    Ok(report)
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
            if has_target {
                score.misses.push(format!(
                    "{}::{} → `{}` (caller not extracted)",
                    pair.file, pair.caller, pair.callee
                ));
            }
            continue;
        };
        let mut found = false;
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
            let correct = pair.targets.iter().any(|t| {
                t.file == target.file_path
                    && t.name == target.name
                    && target.start_line <= t.line
                    && t.line <= target.end_line
            });
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
    let _ = writeln!(
        out,
        "\n### Held-out real repositories (report only, not gated)\n"
    );
    let _ = writeln!(
        out,
        "{} call sites drawn at random from {} pinned repositories and traced by reading the code \
         (`graph-gold/held-out/`; labelled by `{}`, reviewed by `{}`). Per (caller, callee name) pair: \
         precision over the edges drawn from those callers to symbols of that name, recall over the \
         pairs with a project target.\n",
        set.pairs.len(),
        set.repos.len(),
        set.labelled_by,
        set.reviewed_by.as_deref().unwrap_or("nobody yet"),
    );
    for skipped in &report.skipped {
        let _ = writeln!(
            out,
            "Skipped — {skipped} (fetch with `real-repos/fetch.sh`).\n"
        );
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
    let misses: Vec<&String> = report.repos.iter().flat_map(|s| &s.misses).collect();
    let wrong: Vec<&String> = report.repos.iter().flat_map(|s| &s.wrong).collect();
    if !misses.is_empty() {
        let _ = writeln!(out, "\nMissed: {}", list(&misses));
    }
    if !wrong.is_empty() {
        let _ = writeln!(out, "\nWrong edges: {}", list(&wrong));
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
    }
}
