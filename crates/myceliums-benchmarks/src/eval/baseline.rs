//! The recorded baseline: reference scores plus the provenance to reproduce them.
//!
//! A baseline is only useful if a reader can tell what it was measured against.
//! These tests guard that property — the numbers themselves are compared by the
//! `retrieval-eval` binary, not here.
//!
//! The guarantee is content-addressed, not history-addressed: see
//! [`recorded_commit_sha_is_well_formed`] for why a git reference cannot carry
//! it in a squash-merge repository.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Provenance header of `golden/baseline.json`.
///
/// Only the fields the tests reason about; the scores live under `results` and
/// are deliberately not modelled here.
#[derive(Debug, Deserialize)]
struct BaselineProvenance {
    /// Commit the measurement ran against. Informational: see
    /// [`recorded_commit_sha_is_well_formed`] for why this is not a gate.
    commit_sha: String,
    /// Golden-set version the scores belong to.
    dataset_version: String,
    /// Content digest of the golden set the scores were measured against.
    ///
    /// Optional so an older baseline still parses; the test that reads it
    /// fails loudly rather than silently skipping when it is absent.
    #[serde(default)]
    dataset_digest: Option<String>,
}

/// Read the committed baseline's provenance header.
fn provenance() -> Result<BaselineProvenance> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("golden/baseline.json");
    let json =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&json).context("parsing the baseline provenance header")
}

/// The recorded sha must look like a sha, or it is not provenance at all.
///
/// # Why reachability is deliberately not asserted
///
/// Earlier versions of this module required the recorded commit to resolve and
/// to be an ancestor of `HEAD`. Both assertions are unsatisfiable in this
/// repository, because every pull request lands via **squash merge** and its
/// branch is then deleted (see CLAUDE.md). The commit that produced a baseline
/// is therefore destroyed by the very merge that publishes it:
///
/// - in CI, the sha does not resolve at all — the branch it lived on is gone,
///   so a fresh clone has never seen the object;
/// - in the author's own clone it still resolves, but is not an ancestor of
///   `main`, because the squash replaced it with a different commit.
///
/// The two checks failed in complementary environments, which is why the break
/// was invisible until it reached `main`: the tests passed on the pull request
/// and turned red the moment it merged, then stayed red for every subsequent
/// commit. Re-recording could not fix it — the next baseline would be recorded
/// on a branch too.
///
/// Under a squash-merge workflow, a per-commit reference is simply not a stable
/// invariant. What *is* stable is the content the baseline was measured
/// against, and that is asserted by [`baseline_matches_the_current_dataset_digest`].
/// The sha remains recorded as a human breadcrumb — a reader can usually still
/// find it through the pull request — but it is not a gate.
///
/// So this test only rejects a value that could never identify a commit:
/// truncated, mistyped, or hand-edited into the file.
#[test]
fn recorded_commit_sha_is_well_formed() {
    let Ok(provenance) = provenance() else {
        return;
    };
    let sha = provenance.commit_sha.trim_end_matches("-dirty");
    if sha == "unknown" {
        return; // recorded outside a git checkout; honest about knowing nothing
    }
    assert!(
        sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
        "baseline commit_sha {sha:?} is not a 40-character hex sha — the header \
         was hand-edited or truncated. Re-record with --update-baseline."
    );
}

/// The baseline's scores must belong to the exact ground truth they are
/// compared against.
///
/// This is the invariant that actually protects the comparison, and unlike a
/// git reference it survives history rewriting. It is also stricter than the
/// version string next to it: `dataset_version` is typed by hand and stays
/// truthful only while nobody forgets to bump it, whereas any edit to the
/// golden set changes this digest.
#[test]
fn baseline_matches_the_current_dataset_digest() {
    let Ok(provenance) = provenance() else {
        return;
    };
    let Some(recorded) = provenance.dataset_digest else {
        panic!(
            "baseline has no dataset_digest — it predates content-addressed \
             provenance. Re-record with --update-baseline."
        );
    };
    assert_eq!(
        recorded,
        crate::eval::golden_set::GoldenSet::embedded_digest(),
        "baseline was recorded against a different golden set than the one that \
         ships now — the scores are not comparable. Re-record with \
         --update-baseline."
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
