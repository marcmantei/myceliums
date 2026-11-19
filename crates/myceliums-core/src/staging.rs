//! Build a repository index next to the live one and swap it in on success.
//!
//! A full re-analysis used to clear the live index first and then rebuild it
//! in place, so a failed or aborted run left a half-written (or empty) index
//! behind. A [`StagedIndex`] is a fresh, empty database directory beside the
//! live one (`data/.build-<pid>-…`, same file system, so the swap is a
//! rename). The caller builds the complete index there and calls
//! [`StagedIndex::commit`]; dropping it without a commit deletes the staging
//! directory and leaves the live index untouched.
//!
//! **The swap.** Where the platform can exchange two directories in one
//! system call (`renameat2(RENAME_EXCHANGE)` on Linux, `renamex_np(RENAME_SWAP)`
//! on macOS), the live path never disappears. Elsewhere — or when the file
//! system refuses the exchange — it is two renames (live → `data/.old-…`,
//! staging → live, the first undone if the second fails), so for an instant
//! the live path does not exist. Before the swap a journal
//! `data/.swap-<pid>-…` records which directory is being replaced.
//!
//! **Recovery.** A process killed mid-build (no `Drop`) leaves its
//! `.build-` directory behind, and one killed between the two renames leaves
//! the only copy of the index in `.old-`. [`recover_leftovers`] — run when a
//! staging directory is created, when the MCP server starts and when the CLI
//! starts — finds the leftovers of processes that no longer run: it renames
//! an `.old-` directory back when its live directory is missing, and deletes
//! the rest. Leftovers of running processes are never touched.

use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes staging directories created by one process.
static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

const BUILD_PREFIX: &str = ".build-";
const OLD_PREFIX: &str = ".old-";
const SWAP_PREFIX: &str = ".swap-";

/// Whether `name` (an entry of `data/`) is one of the staging, old or
/// journal entries of a rebuild, or the lock directory — not a repository
/// index.
pub fn is_internal_entry(name: &str) -> bool {
    name == crate::lock::LOCKS_DIR || leftover_suffix(name).is_some()
}

/// The `<pid>-…` suffix of a staging, old or journal entry.
fn leftover_suffix(name: &str) -> Option<&str> {
    let suffix = [BUILD_PREFIX, OLD_PREFIX, SWAP_PREFIX]
        .iter()
        .find_map(|p| name.strip_prefix(p))?;
    suffix_pid(suffix).map(|_| suffix)
}

/// The PID that created the entry with this suffix.
fn suffix_pid(suffix: &str) -> Option<u32> {
    suffix.split('-').next()?.parse().ok()
}

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
    /// need not exist. Leftovers of dead processes beside it are recovered
    /// first (see [`recover_leftovers`]).
    pub fn create(live: &Path) -> Result<Self> {
        let parent = live
            .parent()
            .context("the index directory has no parent directory")?;
        live.file_name()
            .context("the index directory has no name")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        recover_leftovers(parent);
        // Ids, not the repo id: a repo id can be up to 255 bytes, and a prefix
        // or suffix on it could exceed the file-name limit.
        let suffix = format!(
            "{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        );
        let staging = parent.join(format!("{BUILD_PREFIX}{suffix}"));
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
        let parent = self.live.parent().expect("checked in create").to_path_buf();
        let journal = parent.join(format!("{SWAP_PREFIX}{}", self.suffix));
        let live_name = self.live.file_name().expect("checked in create");
        std::fs::write(&journal, live_name.as_encoded_bytes())
            .context("cannot write the swap journal")?;
        let result = self.swap(&parent);
        if result.is_ok() {
            self.committed = true;
        }
        if let Err(e) = std::fs::remove_file(&journal) {
            tracing::warn!("cannot delete the swap journal {}: {e}", journal.display());
        }
        result
    }

    fn swap(&self, parent: &Path) -> Result<()> {
        let rename = |from: &Path, to: &Path| std::fs::rename(from, to);
        if !self.live.exists() {
            return rename(&self.staging, &self.live).with_context(|| {
                format!("cannot move the new index into {}", self.live.display())
            });
        }
        // One system call: the live path never disappears. The staging path
        // then holds the previous index; delete it.
        match exchange(&self.staging, &self.live) {
            Ok(()) => {
                remove_best_effort(&self.staging, "previous index");
                return Ok(());
            }
            Err(e) => tracing::debug!("atomic exchange unavailable ({e}); swapping by renames"),
        }
        let old = parent.join(format!("{OLD_PREFIX}{}", self.suffix));
        swap_by_renames(&self.staging, &self.live, &old, &rename)?;
        remove_best_effort(&old, "previous index");
        Ok(())
    }
}

/// The fallback swap: `live` → `old`, then `staging` → `live`; if the second
/// rename fails, the first is undone. `rename` is injected by tests.
fn swap_by_renames(
    staging: &Path,
    live: &Path,
    old: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    rename(live, old)
        .with_context(|| format!("cannot move the previous index {} aside", live.display()))?;
    if let Err(e) = rename(staging, live) {
        if let Err(undo) = rename(old, live) {
            tracing::error!(
                "cannot restore the previous index {} from {}: {undo}; \
                 the next analysis or server start restores it",
                live.display(),
                old.display()
            );
        }
        return Err(e)
            .with_context(|| format!("cannot move the new index into {}", live.display()));
    }
    Ok(())
}

impl Drop for StagedIndex {
    fn drop(&mut self) {
        if !self.committed && self.staging.exists() {
            remove_best_effort(&self.staging, "unfinished index");
        }
    }
}

fn remove_best_effort(dir: &Path, what: &str) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        tracing::warn!("cannot delete the {what} {}: {e}", dir.display());
    }
}

/// Exchanges two directories in one system call.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
    let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: two valid NUL-terminated paths; renameat2 reads them only.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Exchanges two directories in one system call.
#[cfg(target_os = "macos")]
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
    let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: two valid NUL-terminated paths; renamex_np reads them only.
    let rc = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// No single-call exchange on this platform: the caller renames twice.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn exchange(_a: &Path, _b: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// What [`recover_leftovers`] did with one leftover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovered {
    /// An index whose swap was interrupted was moved back to this path.
    Restored(PathBuf),
    /// A leftover staging, old or journal entry was deleted.
    Removed(PathBuf),
}

/// Recovers the leftovers of rebuilds whose process no longer runs, in
/// `data_root` (the `data/` directory of a data home): an `.old-` directory
/// whose live directory is missing is renamed back, every other leftover is
/// deleted. Entries of running processes — this one included — are left
/// alone. Never fails: problems are logged.
pub fn recover_leftovers(data_root: &Path) -> Vec<Recovered> {
    recover_leftovers_where(data_root, &|pid| {
        pid == std::process::id() || crate::lock::is_process_alive(pid)
    })
}

fn recover_leftovers_where(data_root: &Path, running: &dyn Fn(u32) -> bool) -> Vec<Recovered> {
    let Ok(entries) = std::fs::read_dir(data_root) else {
        return Vec::new();
    };
    // Group by suffix: the journal names the live directory of its swap.
    let mut suffixes: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            leftover_suffix(&name).map(str::to_string)
        })
        .filter(|s| suffix_pid(s).is_some_and(|pid| !running(pid)))
        .collect();
    suffixes.sort();
    suffixes.dedup();

    let mut done = Vec::new();
    for suffix in suffixes {
        let build = data_root.join(format!("{BUILD_PREFIX}{suffix}"));
        let old = data_root.join(format!("{OLD_PREFIX}{suffix}"));
        let journal = data_root.join(format!("{SWAP_PREFIX}{suffix}"));
        let live = std::fs::read(&journal)
            .ok()
            .and_then(|name| String::from_utf8(name).ok())
            .filter(|name| {
                let mut parts = Path::new(name).components();
                matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
            })
            .map(|name| data_root.join(name));

        if old.exists() {
            match &live {
                Some(live) if !live.exists() => match std::fs::rename(&old, live) {
                    Ok(()) => {
                        tracing::warn!(
                            "restored the index {} from an interrupted rebuild",
                            live.display()
                        );
                        done.push(Recovered::Restored(live.clone()));
                    }
                    Err(e) => {
                        tracing::error!(
                            "cannot restore the index {} from {}: {e}",
                            live.display(),
                            old.display()
                        );
                        continue;
                    }
                },
                Some(_) => {
                    remove_best_effort(&old, "previous index");
                    done.push(Recovered::Removed(old.clone()));
                }
                None => {
                    // Without its journal nobody knows whose index this is:
                    // keep it rather than guess.
                    tracing::warn!(
                        "{} has no swap journal; leaving it for manual inspection",
                        old.display()
                    );
                }
            }
        }
        if build.exists() {
            remove_best_effort(&build, "unfinished index");
            done.push(Recovered::Removed(build));
        }
        // The journal goes once its old index is dealt with.
        if journal.exists() && !old.exists() && std::fs::remove_file(&journal).is_ok() {
            done.push(Recovered::Removed(journal));
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A PID that does not run.
    const DEAD: u32 = 99_999_999;

    #[test]
    fn dropping_without_commit_keeps_the_live_index_and_removes_the_staging_dir() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("repo");
        write(&live.join("symbols.lance"), "old");

        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "half-written");
        drop(staged);

        assert_eq!(read(&live.join("symbols.lance")), "old");
        assert_eq!(entries(&data.path().join("data")), vec!["repo"]);
    }

    #[test]
    fn commit_swaps_in_the_new_index() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("repo");
        write(&live.join("symbols.lance"), "old");
        write(&live.join("stale-table.lance"), "old");

        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        staged.commit().unwrap();

        assert_eq!(read(&live.join("symbols.lance")), "new");
        assert!(!live.join("stale-table.lance").exists());
        assert_eq!(
            entries(&data.path().join("data")),
            vec!["repo"],
            "no staging, old or journal entry is left behind"
        );
    }

    #[test]
    fn the_two_rename_fallback_swaps_or_undoes_a_failed_second_rename() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let (live, staging, old) = (root.join("repo"), root.join("stage"), root.join("old"));
        write(&live.join("symbols.lance"), "old");
        write(&staging.join("symbols.lance"), "new");

        let refuse_staging = |from: &Path, to: &Path| {
            if from == staging {
                Err(std::io::Error::other("injected"))
            } else {
                std::fs::rename(from, to)
            }
        };
        let err = swap_by_renames(&staging, &live, &old, &refuse_staging).unwrap_err();
        assert!(format!("{err:#}").contains("injected"), "{err:#}");
        assert_eq!(read(&live.join("symbols.lance")), "old", "undone");
        assert!(!old.exists());

        swap_by_renames(&staging, &live, &old, &|a, b| std::fs::rename(a, b)).unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "new");
        assert_eq!(read(&old.join("symbols.lance")), "old");
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    #[test]
    fn the_platform_exchange_swaps_two_directories_in_one_call() {
        let data = tempfile::tempdir().unwrap();
        let (a, b) = (data.path().join("a"), data.path().join("b"));
        write(&a.join("t"), "a");
        write(&b.join("t"), "b");
        exchange(&a, &b).unwrap();
        assert_eq!(read(&a.join("t")), "b");
        assert_eq!(read(&b.join("t")), "a");
    }

    #[test]
    fn commit_without_a_previous_index_creates_it() {
        let data = tempfile::tempdir().unwrap();
        let live = data.path().join("data").join("fresh");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        staged.commit().unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "new");
    }

    #[test]
    fn leftovers_of_dead_processes_are_recovered() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");

        // Killed mid-build: only the staging directory.
        write(
            &root.join(format!(".build-{DEAD}-1-0/symbols.lance")),
            "half",
        );
        // Killed between the renames: live gone, journal + old + build.
        write(&root.join(format!(".swap-{DEAD}-2-0")), "repo-a");
        write(
            &root.join(format!(".old-{DEAD}-2-0/symbols.lance")),
            "a-old",
        );
        write(
            &root.join(format!(".build-{DEAD}-2-0/symbols.lance")),
            "a-new",
        );
        // Killed after the swap, before cleanup: live is the new index.
        write(&root.join("repo-b/symbols.lance"), "b-new");
        write(&root.join(format!(".swap-{DEAD}-3-0")), "repo-b");
        write(
            &root.join(format!(".old-{DEAD}-3-0/symbols.lance")),
            "b-old",
        );
        // A running process's build: untouched.
        let me = std::process::id();
        write(&root.join(format!(".build-{me}-4-0/symbols.lance")), "busy");
        // No journal: whose index is this? Kept.
        write(
            &root.join(format!(".old-{DEAD}-5-0/symbols.lance")),
            "orphan",
        );

        let done = recover_leftovers(&root);
        assert!(
            done.contains(&Recovered::Restored(root.join("repo-a"))),
            "{done:?}"
        );
        assert_eq!(read(&root.join("repo-a/symbols.lance")), "a-old");
        assert_eq!(read(&root.join("repo-b/symbols.lance")), "b-new");
        assert_eq!(
            entries(&root),
            vec![
                format!(".build-{me}-4-0"),
                format!(".old-{DEAD}-5-0"),
                "repo-a".to_string(),
                "repo-b".to_string(),
            ]
        );
        assert!(recover_leftovers(&root).is_empty(), "idempotent");
    }

    #[test]
    fn internal_entries_are_recognised() {
        assert!(is_internal_entry(".locks"));
        assert!(is_internal_entry(".build-123-456-0"));
        assert!(is_internal_entry(".old-123-456-0"));
        assert!(is_internal_entry(".swap-123-456-0"));
        assert!(!is_internal_entry(".build-notapid"));
        assert!(!is_internal_entry("my-repo-1a2b3c4d"));
        assert!(!is_internal_entry(".hidden-project-1a2b3c4d"));
    }

    #[test]
    fn a_journal_cannot_point_outside_the_data_directory() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        write(&root.join(format!(".swap-{DEAD}-1-0")), "../escape");
        write(&root.join(format!(".old-{DEAD}-1-0/symbols.lance")), "x");
        recover_leftovers_where(&root, &|_| false);
        assert!(!data.path().join("escape").exists());
        assert!(root.join(format!(".old-{DEAD}-1-0")).exists());
    }
}
