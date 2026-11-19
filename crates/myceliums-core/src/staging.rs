//! Build a repository index next to the live one and swap it in on success.
//!
//! A full re-analysis used to clear the live index first and then rebuild it
//! in place, so a failed or aborted run left a half-written (or empty) index
//! behind. A [`StagedIndex`] is a fresh, empty database directory beside the
//! live one (`data/.build-…`, same file system, so the swap is a rename).
//! The caller builds the complete index there and calls
//! [`StagedIndex::commit`]; dropping it without a commit deletes the staging
//! directory and leaves the live index untouched.
//!
//! The swap is two renames (live → `data/.old-…`, staging → live), then the
//! old directory is deleted. Between the renames — microseconds — the live
//! path does not exist; if the second rename fails the first is undone. An
//! `analysis.lock` in the live directory is copied into the new one before
//! the swap, so the lock of the running analysis stays in place.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The name of the analysis lock file inside a repository's index directory
/// (see [`crate::lock`]).
const LOCK_FILE: &str = "analysis.lock";

/// Distinguishes staging directories created by one process.
static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

/// A staging directory for a new index of one repository.
#[derive(Debug)]
pub struct StagedIndex {
    staging: PathBuf,
    live: PathBuf,
    suffix: String,
    committed: bool,
}

impl StagedIndex {
    /// Creates an empty staging directory for the index at `live` (usually
    /// [`myceliums_storage::RepoRegistry::repo_db_path`]). The live directory
    /// need not exist.
    pub fn create(live: &Path) -> Result<Self> {
        let parent = live
            .parent()
            .context("the index directory has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        // Ids, not the repo id: a repo id can be up to 255 bytes, and a prefix
        // or suffix on it could exceed the file-name limit.
        let suffix = format!(
            "{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        );
        let staging = parent.join(format!(".build-{suffix}"));
        std::fs::create_dir(&staging)
            .with_context(|| format!("cannot create {}", staging.display()))?;
        Ok(Self {
            staging,
            live: live.to_path_buf(),
            suffix,
            committed: false,
        })
    }

    /// Where to build the new index.
    pub fn path(&self) -> &Path {
        &self.staging
    }

    /// Replaces the live index with the staged one. Every handle on either
    /// directory (open stores) should be closed first: renaming a directory
    /// with open files fails on Windows.
    ///
    /// On error the live index is unchanged and the staging directory is
    /// deleted.
    pub fn commit(mut self) -> Result<()> {
        let lock = self.live.join(LOCK_FILE);
        if lock.is_file() {
            std::fs::copy(&lock, self.staging.join(LOCK_FILE))
                .context("cannot carry the analysis lock into the new index")?;
        }
        let old = self
            .live
            .parent()
            .expect("checked in create")
            .join(format!(".old-{}", self.suffix));
        let had_live = self.live.exists();
        if had_live {
            std::fs::rename(&self.live, &old).with_context(|| {
                format!(
                    "cannot move the previous index {} aside",
                    self.live.display()
                )
            })?;
        }
        if let Err(e) = std::fs::rename(&self.staging, &self.live) {
            if had_live {
                if let Err(undo) = std::fs::rename(&old, &self.live) {
                    tracing::error!(
                        "cannot restore the previous index {} from {}: {undo}",
                        self.live.display(),
                        old.display()
                    );
                }
            }
            return Err(e).with_context(|| {
                format!("cannot move the new index into {}", self.live.display())
            });
        }
        self.committed = true;
        if had_live {
            if let Err(e) = std::fs::remove_dir_all(&old) {
                tracing::warn!("cannot delete the previous index {}: {e}", old.display());
            }
        }
        Ok(())
    }
}

impl Drop for StagedIndex {
    fn drop(&mut self) {
        if !self.committed && self.staging.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.staging) {
                tracing::warn!(
                    "cannot delete the unfinished index {}: {e}",
                    self.staging.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn dropping_without_commit_keeps_the_live_index_and_removes_the_staging_dir() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("repo");
        write(&live.join("symbols.lance"), "old");

        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "half-written");
        drop(staged);

        assert_eq!(
            std::fs::read_to_string(live.join("symbols.lance")).unwrap(),
            "old"
        );
        assert_eq!(entries(&data.path().join("data")), vec!["repo"]);
    }

    #[test]
    fn commit_swaps_in_the_new_index_and_keeps_the_lock() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("repo");
        write(&live.join("symbols.lance"), "old");
        write(&live.join("stale-table.lance"), "old");
        write(&live.join(LOCK_FILE), "123\n2026-01-01T00:00:00Z");

        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        staged.commit().unwrap();

        assert_eq!(
            std::fs::read_to_string(live.join("symbols.lance")).unwrap(),
            "new"
        );
        assert!(!live.join("stale-table.lance").exists());
        assert_eq!(
            std::fs::read_to_string(live.join(LOCK_FILE)).unwrap(),
            "123\n2026-01-01T00:00:00Z"
        );
        assert_eq!(
            entries(&data.path().join("data")),
            vec!["repo"],
            "no staging or old directory is left behind"
        );
    }

    #[test]
    fn commit_without_a_previous_index_creates_it() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("fresh");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        staged.commit().unwrap();
        assert_eq!(
            std::fs::read_to_string(live.join("symbols.lance")).unwrap(),
            "new"
        );
    }
}
