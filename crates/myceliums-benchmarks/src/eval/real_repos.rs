//! The held-out real-repository query set (#95): labelled queries over
//! open-source repositories pinned by commit, split into a development half for
//! tuning and a held-out half for confirming.
//!
//! The golden set ([`super::golden_set`]) is 51 queries over five toy fixtures,
//! written by the same automation that wrote the search code — fine as a CI
//! regression guard, too small and too close to the code to tune parameters
//! on. This set exists for that: real code, real identifiers, and a split that
//! keeps the queries a decision is confirmed on out of the queries it was made
//! on.
//!
//! - The dataset is `real-repos/queries.json`: the repository pins and the
//!   labelled queries. Every query records who labelled it (`labelled_by`) and
//!   who reviewed the labels (`reviewed_by`, `null` until a human has).
//! - The sources are **fetched, not vendored**: `real-repos/fetch.sh` checks
//!   each repository out at its pinned commit into a cache directory
//!   ([`repos_dir`]), and [`verify_checkout`] refuses a checkout at any other
//!   commit.
//! - [`RealRepoSet::split`] assigns every query to [`Split::Dev`] or
//!   [`Split::HeldOut`] deterministically, stratified by repository and intent.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::evaluator::RANKING_DEPTH;
use super::golden_set::{GoldenQuery, QueryIntent, SymbolRef};

/// Dataset layouts this loader reads.
const SUPPORTED_SCHEMA_VERSION: &str = "1.0";

/// Environment variable naming the directory the repositories are checked out
/// in. Read by `real-repos/fetch.sh` as well.
pub const REPOS_DIR_ENV: &str = "MYC_EVAL_REPOS_DIR";

/// Fraction of each (repository, intent) stratum assigned to the development
/// split; the rest is held out.
pub const DEV_FRACTION: f64 = 0.6;

/// Mixed into the split hash so the assignment is a property of this dataset's
/// design, not of SHA-256 over bare ids. Changing it reshuffles the split and
/// invalidates every tuning result recorded against it — never change it.
const SPLIT_SALT: &str = "myceliums-#95-split-v1";

/// One repository of the corpus, pinned to the commit the labels were written
/// against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoPin {
    /// Short name; the checkout directory and the query-id prefix.
    pub name: String,
    /// Clone URL.
    pub url: String,
    /// Release tag the commit was taken from — informational.
    pub tag: String,
    /// Full 40-hex commit sha the checkout must be at.
    pub commit: String,
    /// Main language, for the report.
    pub language: String,
    /// SPDX licence id of the repository.
    pub license: String,
}

/// One labelled answer, with the labeller's reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    /// Path relative to the repository root.
    pub file: String,
    /// Qualified symbol name as the parser produces it.
    pub symbol: String,
    /// What in this symbol's code answers the query.
    pub why: String,
}

impl Label {
    /// The stable reference the metrics compare against.
    pub fn symbol_ref(&self) -> SymbolRef {
        SymbolRef::new(self.file.clone(), self.symbol.clone())
    }
}

/// One query of the real-repo set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealQuery {
    /// `<repo>-<intent>-<slug>`, kebab-case.
    pub id: String,
    /// Name of the [`RepoPin`] the query searches.
    pub repo: String,
    /// The text handed to the search engine.
    pub query: String,
    /// What the query probes — the golden set's vocabulary.
    pub intent: QueryIntent,
    /// The symbols that answer it. Never empty.
    pub relevant: Vec<Label>,
    /// Why these labels and not others.
    pub rationale: String,
    /// Who drafted the labels, e.g. `agent:claude` or `human:<handle>`.
    pub labelled_by: String,
    /// Who reviewed them; `null` until a human has.
    pub reviewed_by: Option<String>,
}

impl RealQuery {
    /// The query in the shape the evaluator scores.
    pub fn golden(&self) -> GoldenQuery {
        GoldenQuery {
            id: self.id.clone(),
            query: self.query.clone(),
            intent: self.intent,
            relevant: self.relevant.iter().map(Label::symbol_ref).collect(),
            rationale: self.rationale.clone(),
        }
    }

    /// How this query's labels were checked, read from `reviewed_by`.
    pub fn review_class(&self) -> ReviewClass {
        match self.reviewed_by.as_deref() {
            None => ReviewClass::Unreviewed,
            Some(r) if mentions_agent(r) || is_owner_policy(r) => ReviewClass::OwnerPolicy,
            Some(_) => ReviewClass::Human,
        }
    }

    /// True when a person other than the labeller checked these labels.
    /// [`RealRepoSet::validate`] guarantees that a recorded reviewer is not
    /// the labeller; an agent's check accepted under an owner policy is not a
    /// human review and does not count here.
    pub fn is_reviewed(&self) -> bool {
        self.review_class() == ReviewClass::Human
    }
}

/// How a query's labels were checked (#164).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewClass {
    /// `reviewed_by` is `null`.
    Unreviewed,
    /// A person other than the labeller checked every label (`human:<handle>`).
    Human,
    /// Agents checked some or all labels and the owner accepted that under a
    /// policy named by its exact id ([`OWNER_POLICIES`]), e.g.
    /// `agent:second-check, per marc's policy (owner-2026-11-22)` or
    /// `mixed, policy owner-2026-11-22: 2× human:marc; 1× agent:prescreen, …`.
    /// Never reported as a human review.
    OwnerPolicy,
}

/// Which half of the set a query belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Split {
    /// Parameters may be tuned on these queries.
    Dev,
    /// Only for confirming a decision made on dev.
    HeldOut,
}

impl Split {
    /// Report label.
    pub fn label(&self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::HeldOut => "held-out",
        }
    }
}

/// The dataset file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealRepoSet {
    /// Layout version of the file.
    pub schema_version: String,
    /// Content version, bumped when labels change.
    pub dataset_version: String,
    /// The pinned repositories.
    pub repos: Vec<RepoPin>,
    /// The labelled queries.
    pub queries: Vec<RealQuery>,
}

impl RealRepoSet {
    /// Where the dataset lives in this crate.
    pub fn default_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("real-repos/queries.json")
    }

    /// Load and validate the dataset shipped with this crate.
    pub fn load() -> Result<Self> {
        Self::from_path(&Self::default_path())
    }

    /// Load and validate a dataset file.
    pub fn from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading the real-repo set {}", path.display()))?;
        Self::from_json(&raw).with_context(|| format!("in {}", path.display()))
    }

    /// Parse and validate dataset JSON.
    pub fn from_json(raw: &str) -> Result<Self> {
        let set: Self = serde_json::from_str(raw).context("parsing the real-repo set")?;
        set.validate()?;
        Ok(set)
    }

    /// SHA-256 of a dataset file's bytes — what a recorded run is tied to.
    pub fn digest_of(path: &Path) -> Result<String> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(format!("{:x}", Sha256::digest(&bytes)))
    }

    /// Serialise exactly as the file is stored: two-space JSON, trailing newline.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    /// Reject a dataset that would produce meaningless or misleading numbers.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            bail!(
                "real-repo set schema '{}' is not supported (this harness reads '{}')",
                self.schema_version,
                SUPPORTED_SCHEMA_VERSION
            );
        }
        if self.repos.is_empty() || self.queries.is_empty() {
            bail!("the real-repo set needs repositories and queries");
        }
        let mut names = BTreeSet::new();
        for repo in &self.repos {
            if !is_kebab(&repo.name) || !names.insert(repo.name.as_str()) {
                bail!("repository name '{}' is not unique kebab-case", repo.name);
            }
            if repo.commit.len() != 40 || !repo.commit.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!(
                    "repository '{}' must be pinned to a full commit sha, not '{}'",
                    repo.name,
                    repo.commit
                );
            }
        }
        let mut ids = BTreeSet::new();
        for query in &self.queries {
            let id = &query.id;
            if !ids.insert(id.as_str()) {
                bail!("duplicate query id '{id}'");
            }
            if !names.contains(query.repo.as_str()) {
                bail!("query '{id}' names unknown repository '{}'", query.repo);
            }
            let prefix = format!("{}-{}-", query.repo, query.intent.label());
            if !is_kebab(id) || !id.starts_with(&prefix) {
                bail!("query id '{id}' must be kebab-case and start with '{prefix}'");
            }
            for (field, value) in [
                ("query", &query.query),
                ("rationale", &query.rationale),
                ("labelled_by", &query.labelled_by),
            ] {
                if value.trim().is_empty() {
                    bail!("query '{id}' has an empty {field}");
                }
            }
            if let Some(problem) = review_problem(query) {
                bail!("query '{id}': {problem}");
            }
            if query.relevant.is_empty() {
                bail!("query '{id}' has no relevant symbols labelled");
            }
            if query.relevant.len() > RANKING_DEPTH {
                bail!(
                    "query '{id}' labels {} answers; R-precision needs them to fit in the \
                     {RANKING_DEPTH} results every mode returns",
                    query.relevant.len()
                );
            }
            let mut labels = BTreeSet::new();
            for label in &query.relevant {
                if !labels.insert(label.symbol_ref()) {
                    bail!("query '{id}' labels {} twice", label.symbol_ref());
                }
                if label.why.trim().is_empty() {
                    bail!("query '{id}' gives no reason for {}", label.symbol_ref());
                }
            }
        }
        Ok(())
    }

    /// The pin of the repository named `name`.
    pub fn repo(&self, name: &str) -> Option<&RepoPin> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    /// Assign every query to dev or held-out.
    ///
    /// Within each (repository, intent) stratum the queries are ordered by
    /// SHA-256 of a fixed salt and their id, and the first
    /// `round(DEV_FRACTION × n)` go to dev. The result depends only on the ids,
    /// so it is the same on every machine and every run, and each stratum is
    /// split 60/40 as closely as its size allows — no repository or intent can
    /// end up mostly on one side by chance.
    ///
    /// The ordering is by hash, not by position in the file, so reordering the
    /// file changes nothing. Adding or removing a query in a stratum can move
    /// at most one other query of that stratum across the boundary; any such
    /// change alters the dataset digest a recorded run is tied to, and tuning
    /// must then be redone from the dev split.
    pub fn split(&self) -> BTreeMap<String, Split> {
        let mut strata: BTreeMap<(&str, QueryIntent), Vec<(String, &str)>> = BTreeMap::new();
        for query in &self.queries {
            strata
                .entry((query.repo.as_str(), query.intent))
                .or_default()
                .push((split_hash(&query.id), query.id.as_str()));
        }
        let mut assignment = BTreeMap::new();
        for members in strata.values_mut() {
            members.sort();
            let dev = dev_count(members.len());
            for (position, (_, id)) in members.iter().enumerate() {
                let split = if position < dev {
                    Split::Dev
                } else {
                    Split::HeldOut
                };
                assignment.insert(id.to_string(), split);
            }
        }
        assignment
    }

    /// The queries of `split`, optionally restricted to one repository, in
    /// dataset order.
    pub fn queries_in(&self, split: Option<Split>, repo: Option<&str>) -> Vec<&RealQuery> {
        let assignment = self.split();
        self.queries
            .iter()
            .filter(|q| split.is_none_or(|s| assignment.get(&q.id) == Some(&s)))
            .filter(|q| repo.is_none_or(|r| q.repo == r))
            .collect()
    }

    /// How many queries a person other than the labeller reviewed.
    pub fn reviewed(&self) -> usize {
        self.queries.iter().filter(|q| q.is_reviewed()).count()
    }

    /// How many queries were checked by agents and accepted under an owner policy.
    pub fn agent_verified(&self) -> usize {
        self.queries
            .iter()
            .filter(|q| q.review_class() == ReviewClass::OwnerPolicy)
            .count()
    }

    /// One line on the review state: human-reviewed, agent-verified under an
    /// owner policy, unreviewed. Agent provenance is never called human.
    pub fn review_summary(&self) -> String {
        let total = self.queries.len();
        let human = self.reviewed();
        let policy = self.agent_verified();
        if human == total {
            return "every query's labels were reviewed by a human".to_string();
        }
        format!(
            "{total} queries: human-reviewed {human}, agent-verified under owner policy {policy}, \
             unreviewed {} — numbers rest on labels no human checked one by one where not \
             human-reviewed (real-repos/README.md)",
            total - human - policy
        )
    }

    /// Record that `reviewer` checked the labels of query `id` and accepts them.
    ///
    /// A reviewer is a person: an `agent:` identity is refused, because an
    /// agent confirming agent-drafted labels is no review at all. A bare
    /// handle is stored as `human:<handle>`.
    pub fn record_review(&mut self, id: &str, reviewer: &str) -> Result<()> {
        let reviewer = reviewer_identity(reviewer)?;
        let query = self
            .queries
            .iter_mut()
            .find(|q| q.id == id)
            .with_context(|| format!("no query '{id}'"))?;
        let previous = query.reviewed_by.replace(reviewer);
        if let Some(problem) = review_problem(query) {
            query.reviewed_by = previous;
            bail!("query '{id}': {problem}");
        }
        Ok(())
    }
}

/// True when `identity` names an agent: `agent:<name>`, in any letter case.
fn is_agent(identity: &str) -> bool {
    identity.trim().to_ascii_lowercase().starts_with("agent:")
}

/// True when `identity` mentions an agent anywhere (`agent:<name>`), in any
/// letter case — a mixed provenance names one after a human.
fn mentions_agent(identity: &str) -> bool {
    identity.to_ascii_lowercase().contains("agent:")
}

/// Owner policies under which an agent's check may be recorded in
/// `reviewed_by` (#164). Each id names a written policy
/// (`evals/review/policy/<id>.json`, `evals/review/README.md`).
pub const OWNER_POLICIES: &[&str] = &["owner-2026-11-22"];

/// True when `identity` names one of the [`OWNER_POLICIES`] by its exact id,
/// e.g. `agent:second-check, per marc's policy (owner-2026-11-22)`.
fn is_owner_policy(identity: &str) -> bool {
    let lower = identity.to_ascii_lowercase();
    OWNER_POLICIES.iter().any(|id| lower.contains(id))
}

/// Why a query's `reviewed_by` does not count as a review, if it does not.
///
/// `null` is fine (unreviewed). A recorded reviewer must be non-empty, must not
/// be an agent, and must not be the labeller: an agent, or anyone, confirming
/// their own labels is no review at all. Checked on every load, so a hand edit
/// cannot make the evaluator report agent-drafted labels as human-reviewed.
fn review_problem(query: &RealQuery) -> Option<String> {
    let reviewer = query.reviewed_by.as_deref()?.trim();
    if reviewer.is_empty() {
        return Some("reviewed_by is empty — use null for unreviewed".to_string());
    }
    if (is_agent(reviewer) || mentions_agent(reviewer)) && !is_owner_policy(reviewer) {
        return Some(format!(
            "reviewed_by '{reviewer}' is an agent; only a human review, or an agent check \
             accepted under a named owner policy, counts"
        ));
    }
    if reviewer.eq_ignore_ascii_case(query.labelled_by.trim()) {
        return Some(format!(
            "reviewed_by '{reviewer}' is the labeller; labels need a second person"
        ));
    }
    None
}

/// Normalise a reviewer handle to `human:<handle>`, refusing agents.
pub fn reviewer_identity(reviewer: &str) -> Result<String> {
    let reviewer = reviewer.trim();
    if reviewer.is_empty() {
        bail!("a review needs a reviewer");
    }
    if is_agent(reviewer) {
        bail!("'{reviewer}' is an agent; only a human review marks labels as reviewed");
    }
    Ok(if reviewer.contains(':') {
        reviewer.to_string()
    } else {
        format!("human:{reviewer}")
    })
}

/// Dev members of a stratum of `n` queries: `round(DEV_FRACTION × n)`.
fn dev_count(n: usize) -> usize {
    ((n as f64) * DEV_FRACTION).round() as usize
}

/// The split-ordering key of a query id.
fn split_hash(id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{SPLIT_SALT}\0{id}").as_bytes())
    )
}

fn is_kebab(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The directory the repositories are checked out in: [`REPOS_DIR_ENV`], else
/// `$XDG_CACHE_HOME/myceliums-eval-repos`, else `~/.cache/myceliums-eval-repos`
/// — the same rule `real-repos/fetch.sh` follows.
pub fn repos_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(REPOS_DIR_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let cache = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".cache"),
    };
    Ok(cache.join("myceliums-eval-repos"))
}

/// The checkout of `pin` under `dir`, after confirming it is at the pinned
/// commit. A missing checkout or one at another commit is an error that names
/// the fetch script: scoring labels against code they were not written for
/// would report label drift as a retrieval regression.
pub fn verify_checkout(dir: &Path, pin: &RepoPin) -> Result<PathBuf> {
    let root = dir.join(&pin.name);
    let fetch_hint = "run crates/myceliums-benchmarks/real-repos/fetch.sh";
    if !root.join(".git").exists() {
        bail!(
            "repository '{}' is not checked out at {} — {fetch_hint}",
            pin.name,
            root.display()
        );
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .with_context(|| format!("running git in {}", root.display()))?;
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || head != pin.commit {
        bail!(
            "{} is at '{head}', not the pinned {} — {fetch_hint} into an empty directory",
            root.display(),
            pin.commit
        );
    }
    // The right commit with edited files is not the pinned code either.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(&root)
        .output()
        .with_context(|| format!("running git in {}", root.display()))?;
    let changes = String::from_utf8_lossy(&status.stdout).trim().to_string();
    if !status.status.success() || !changes.is_empty() {
        bail!(
            "{} has local modifications to tracked files — restore them \
             (git -C {} checkout -- .) or re-fetch:\n{changes}",
            root.display(),
            root.display()
        );
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> RealRepoSet {
        RealRepoSet::load().expect("the shipped real-repo set must validate")
    }

    #[test]
    fn the_set_meets_the_size_and_diversity_floor() {
        // #95: at least 150 queries over at least three repositories.
        let set = shipped();
        assert!(set.queries.len() >= 150, "{} queries", set.queries.len());
        assert!(set.repos.len() >= 3, "{} repositories", set.repos.len());
        let languages: BTreeSet<&str> = set.repos.iter().map(|r| r.language.as_str()).collect();
        assert!(languages.len() >= 3, "{languages:?}");
        for repo in &set.repos {
            let queries = set.queries_in(None, Some(&repo.name));
            assert!(queries.len() >= 30, "{}: {}", repo.name, queries.len());
            let intents: BTreeSet<QueryIntent> = queries.iter().map(|q| q.intent).collect();
            assert_eq!(intents.len(), QueryIntent::ALL.len(), "{}", repo.name);
        }
    }

    #[test]
    fn every_query_records_its_labeller_and_review_state() {
        // Agent-drafted labels must say so and stay unreviewed until a human
        // has looked: the README treats the set as non-authoritative until then.
        for query in &shipped().queries {
            assert!(!query.labelled_by.trim().is_empty(), "{}", query.id);
            if query.labelled_by.starts_with("agent:") {
                // An agent's check counts only under a named owner policy, and
                // then never as a human review.
                assert!(
                    query.reviewed_by.as_deref().is_none_or(|r| {
                        !r.to_ascii_lowercase().contains("agent:") || is_owner_policy(r)
                    }),
                    "{}: an agent cannot review its own labels",
                    query.id
                );
                if query.review_class() == ReviewClass::Human {
                    assert!(query
                        .reviewed_by
                        .as_deref()
                        .is_some_and(|r| !mentions_agent(r)));
                }
            }
        }
    }

    #[test]
    fn the_file_is_stored_in_canonical_form() {
        // The review helper rewrites the file with `to_json`; a hand-formatted
        // file would turn every review into a whole-file diff.
        let raw = std::fs::read_to_string(RealRepoSet::default_path()).unwrap();
        assert_eq!(shipped().to_json().unwrap(), raw);
    }

    #[test]
    fn the_split_is_deterministic_disjoint_and_complete() {
        let set = shipped();
        let first = set.split();
        assert_eq!(first, set.split());
        assert_eq!(first.len(), set.queries.len());
        let dev = set.queries_in(Some(Split::Dev), None).len();
        let held = set.queries_in(Some(Split::HeldOut), None).len();
        assert_eq!(dev + held, set.queries.len());
    }

    #[test]
    fn every_stratum_is_split_sixty_forty() {
        let set = shipped();
        let assignment = set.split();
        let mut strata: BTreeMap<(String, QueryIntent), (usize, usize)> = BTreeMap::new();
        for query in &set.queries {
            let counts = strata
                .entry((query.repo.clone(), query.intent))
                .or_default();
            match assignment[&query.id] {
                Split::Dev => counts.0 += 1,
                Split::HeldOut => counts.1 += 1,
            }
        }
        for ((repo, intent), (dev, held)) in strata {
            assert_eq!(dev, dev_count(dev + held), "{repo}/{}", intent.label());
            assert!(held >= 1, "{repo}/{} has nothing held out", intent.label());
        }
    }

    #[test]
    fn the_split_ignores_file_order() {
        let set = shipped();
        let mut reversed = set.clone();
        reversed.queries.reverse();
        assert_eq!(set.split(), reversed.split());
    }

    #[test]
    fn adding_a_query_moves_at_most_one_other() {
        let set = shipped();
        let before = set.split();
        let mut grown = set.clone();
        let mut extra = grown.queries[0].clone();
        extra.id = format!("{}-zz-added", extra.id);
        grown.queries.push(extra);
        let after = grown.split();
        let moved = before
            .iter()
            .filter(|(id, split)| after[*id] != **split)
            .count();
        assert!(moved <= 1, "{moved} existing queries changed split");
    }

    #[test]
    fn dev_count_rounds_to_the_nearest_query() {
        assert_eq!(dev_count(10), 6);
        assert_eq!(dev_count(9), 5);
        assert_eq!(dev_count(14), 8);
        assert_eq!(dev_count(2), 1);
    }

    fn minimal() -> RealRepoSet {
        RealRepoSet {
            schema_version: SUPPORTED_SCHEMA_VERSION.into(),
            dataset_version: "test".into(),
            repos: vec![RepoPin {
                name: "demo".into(),
                url: "https://example.invalid/demo".into(),
                tag: "v1".into(),
                commit: "0123456789abcdef0123456789abcdef01234567".into(),
                language: "Python".into(),
                license: "MIT".into(),
            }],
            queries: vec![RealQuery {
                id: "demo-paraphrase-thing".into(),
                repo: "demo".into(),
                query: "do the thing".into(),
                intent: QueryIntent::Paraphrase,
                relevant: vec![Label {
                    file: "a.py".into(),
                    symbol: "thing".into(),
                    why: "it does the thing".into(),
                }],
                rationale: "only candidate".into(),
                labelled_by: "agent:claude".into(),
                reviewed_by: None,
            }],
        }
    }

    #[test]
    fn a_minimal_set_validates() {
        minimal().validate().unwrap();
    }

    #[test]
    fn malformed_sets_are_rejected() {
        type Breakage = Box<dyn Fn(&mut RealRepoSet)>;
        let cases: Vec<(&str, Breakage)> = vec![
            (
                "short sha",
                Box::new(|s| s.repos[0].commit = "abc123".into()),
            ),
            (
                "unknown repo",
                Box::new(|s| s.queries[0].repo = "other".into()),
            ),
            (
                "id without repo/intent prefix",
                Box::new(|s| s.queries[0].id = "thing".into()),
            ),
            ("no labels", Box::new(|s| s.queries[0].relevant.clear())),
            (
                "label without reason",
                Box::new(|s| s.queries[0].relevant[0].why = " ".into()),
            ),
            (
                "duplicate label",
                Box::new(|s| {
                    let label = s.queries[0].relevant[0].clone();
                    s.queries[0].relevant.push(label);
                }),
            ),
            (
                "empty reviewer",
                Box::new(|s| s.queries[0].reviewed_by = Some(String::new())),
            ),
            (
                "agent reviewer",
                Box::new(|s| s.queries[0].reviewed_by = Some("agent:claude".into())),
            ),
            (
                "agent reviewer in another case",
                Box::new(|s| s.queries[0].reviewed_by = Some(" Agent:Other".into())),
            ),
            (
                "the labeller reviewing their own labels",
                Box::new(|s| {
                    s.queries[0].labelled_by = "human:ana".into();
                    s.queries[0].reviewed_by = Some("HUMAN:ana".into());
                }),
            ),
            (
                "no labeller",
                Box::new(|s| s.queries[0].labelled_by = String::new()),
            ),
            (
                "duplicate id",
                Box::new(|s| {
                    let query = s.queries[0].clone();
                    s.queries.push(query);
                }),
            ),
            (
                "too many labels for the ranking depth",
                Box::new(|s| {
                    s.queries[0].relevant = (0..=RANKING_DEPTH)
                        .map(|i| Label {
                            file: "a.py".into(),
                            symbol: format!("s{i}"),
                            why: "w".into(),
                        })
                        .collect();
                }),
            ),
        ];
        for (name, break_it) in cases {
            let mut set = minimal();
            break_it(&mut set);
            assert!(set.validate().is_err(), "{name} was accepted");
        }
    }

    #[test]
    fn owner_policy_provenance_is_accepted_but_never_counted_as_human() {
        let mut set = minimal();
        for (reviewed_by, class) in [
            ("human:marc", ReviewClass::Human),
            (
                "agent:prescreen, accepted by marc's policy (owner-2026-11-22); \
                 human sample in this dataset: 94 checked, 0 errors",
                ReviewClass::OwnerPolicy,
            ),
            (
                "agent:second-check, per marc's policy (owner-2026-11-22)",
                ReviewClass::OwnerPolicy,
            ),
            (
                "mixed, policy owner-2026-11-22: 2× human:marc; \
                 1× agent:second-check, per marc's policy (owner-2026-11-22)",
                ReviewClass::OwnerPolicy,
            ),
        ] {
            set.queries[0].reviewed_by = Some(reviewed_by.into());
            set.validate().unwrap();
            assert_eq!(set.queries[0].review_class(), class, "{reviewed_by}");
            assert_eq!(set.reviewed(), usize::from(class == ReviewClass::Human));
            assert_eq!(
                set.agent_verified(),
                usize::from(class == ReviewClass::OwnerPolicy)
            );
        }
        assert!(set
            .review_summary()
            .contains("agent-verified under owner policy 1"));
        assert!(!set.review_summary().contains("every query"));
        // an agent without a known owner policy id is still refused, also when
        // the string merely says "policy" or "accepted by"
        for bad in [
            "agent:claude",
            "mixed: human:marc; agent:claude",
            "agent:second-check, per marc's policy",
            "agent:prescreen, accepted by marc",
            "agent:x, per policy owner-2099-01-01",
        ] {
            set.queries[0].reviewed_by = Some(bad.into());
            assert!(set.validate().is_err(), "{bad} was accepted");
        }
        set.queries[0].reviewed_by = None;
        assert_eq!(set.queries[0].review_class(), ReviewClass::Unreviewed);
    }

    #[test]
    fn a_review_is_recorded_for_humans_only() {
        let mut set = minimal();
        assert!(set
            .record_review("demo-paraphrase-thing", "agent:claude")
            .is_err());
        assert!(set
            .record_review("demo-paraphrase-thing", "AGENT:claude")
            .is_err());
        assert!(reviewer_identity("Agent:x").is_err());
        assert!(set.record_review("demo-paraphrase-thing", " ").is_err());
        assert!(set.record_review("no-such-query", "marc").is_err());
        assert_eq!(set.reviewed(), 0);
        set.record_review("demo-paraphrase-thing", "marc").unwrap();
        assert_eq!(set.queries[0].reviewed_by.as_deref(), Some("human:marc"));
        set.validate().unwrap();

        // Labels drafted by a person cannot be reviewed by that person.
        let mut own = minimal();
        own.queries[0].labelled_by = "human:ana".into();
        assert!(own.record_review("demo-paraphrase-thing", "ana").is_err());
        assert_eq!(
            own.queries[0].reviewed_by, None,
            "a refused review leaves no trace"
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut json: serde_json::Value = serde_json::to_value(minimal()).unwrap();
        json["queries"][0]["reviewd_by"] = serde_json::Value::Null;
        assert!(RealRepoSet::from_json(&json.to_string()).is_err());
    }

    #[test]
    fn a_checkout_at_the_wrong_commit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pin = minimal().repos[0].clone();
        let missing = verify_checkout(dir.path(), &pin).unwrap_err().to_string();
        assert!(missing.contains("fetch.sh"), "{missing}");

        let root = dir.path().join(&pin.name);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
        };
        if !git(&["init", "--quiet"]).status.success() {
            return; // no git on this machine; the binary needs it anyway
        }
        std::fs::write(root.join("f"), "x").unwrap();
        git(&["add", "f"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "x",
        ]);
        let wrong = verify_checkout(dir.path(), &pin).unwrap_err().to_string();
        assert!(wrong.contains("not the pinned"), "{wrong}");

        // At the pinned commit, but with a tracked file edited: refused too.
        let head = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout).unwrap();
        let pinned = RepoPin {
            commit: head.trim().to_string(),
            ..pin
        };
        if pinned.commit.len() != 40 {
            return; // the commit above failed (no git identity); nothing to pin
        }
        std::fs::write(root.join("untracked"), "y").unwrap();
        verify_checkout(dir.path(), &pinned).expect("untracked files are not modifications");
        std::fs::write(root.join("f"), "edited").unwrap();
        let edited = verify_checkout(dir.path(), &pinned)
            .unwrap_err()
            .to_string();
        assert!(edited.contains("local modifications"), "{edited}");
    }
}
