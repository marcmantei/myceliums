//! The recorded baseline: reference scores plus the provenance to reproduce them.
//!
//! A baseline is only useful if a reader can get back to the tree that produced
//! it. These tests guard that property — the numbers themselves are compared by
//! the `retrieval-eval` binary, not here.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// Provenance header of `golden/baseline.json`.
///
/// Only the fields the tests reason about; the scores live under `results` and
/// are deliberately not modelled here.
#[derive(Debug, Deserialize)]
struct BaselineProvenance {
    /// Commit the measurement ran against.
    commit_sha: String,
    /// Golden-set version the scores belong to.
    dataset_version: String,
}

/// Read the committed baseline's provenance header.
fn provenance() -> Result<BaselineProvenance> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("golden/baseline.json");
    let json =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&json).context("parsing the baseline provenance header")
}

/// Run a git command in the crate directory, or `None` if git is unavailable.
fn git(args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
}

/// The recorded commit must exist, or the provenance is a dead reference.
///
/// This is exactly the failure a rebase introduces: the sha names a commit that
/// was rewritten away, so the baseline looks authoritative while pointing at
/// nothing. Skipped outside a git checkout (published crate, source tarball).
#[test]
fn recorded_commit_is_resolvable() {
    let Ok(provenance) = provenance() else {
        return;
    };
    let sha = provenance.commit_sha.trim_end_matches("-dirty");
    if sha == "unknown" {
        return;
    }
    let Some(output) = git(&["cat-file", "-t", sha]) else {
        return; // git unavailable — nothing to verify against
    };
    assert!(
        output.status.success(),
        "baseline commit_sha {sha} does not resolve — provenance that cannot be \
         checked out is worse than none. Re-record with --update-baseline."
    );
}

/// The recorded commit must be reachable from the current branch.
///
/// Resolving is not enough: a commit orphaned by an amend still resolves
/// locally until it is garbage-collected, but nobody else can fetch it. Only an
/// ancestor of `HEAD` is genuinely obtainable by a reader.
#[test]
fn recorded_commit_is_an_ancestor() {
    let Ok(provenance) = provenance() else {
        return;
    };
    let sha = provenance.commit_sha.trim_end_matches("-dirty");
    if sha == "unknown" {
        return;
    }
    // Only meaningful inside a git checkout that knows the commit at all.
    match git(&["cat-file", "-t", sha]) {
        Some(output) if output.status.success() => {}
        _ => return,
    }
    let Some(output) = git(&["merge-base", "--is-ancestor", sha, "HEAD"]) else {
        return;
    };
    assert!(
        output.status.success(),
        "baseline commit_sha {sha} resolves but is not an ancestor of HEAD — it \
         was probably orphaned by a rebase or amend, so a reader cannot fetch it."
    );
}

/// The baseline's scores must belong to the golden set they are compared against.
///
/// Comparing a score to a baseline recorded on different ground truth is
/// meaningless, so the two versions have to agree.
#[test]
fn baseline_matches_the_current_dataset_version() {
    let Ok(provenance) = provenance() else {
        return;
    };
    let golden = crate::eval::golden_set::GoldenSet::embedded().unwrap();
    assert_eq!(
        provenance.dataset_version, golden.dataset_version,
        "baseline was recorded on dataset {} but the golden set is now {} — \
         re-record with --update-baseline so the comparison is like-for-like",
        provenance.dataset_version, golden.dataset_version
    );
}
