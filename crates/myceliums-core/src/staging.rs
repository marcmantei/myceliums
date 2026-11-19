//! Build a repository index next to the live one and swap it in on success.
//!
//! A full re-analysis used to clear the live index first and then rebuild it
//! in place, so a failed or aborted run left a half-written (or empty) index
//! behind. A [`StagedIndex`] is a fresh, empty database directory in
//! `data/.staging/` (same file system as the live `data/<repo-id>/`, so the
//! swap is a rename). The caller builds the complete index there and calls
//! [`StagedIndex::commit`]; dropping it without a commit deletes the staging
//! directory and leaves the live index untouched.
//!
//! Everything a rebuild creates lives in `data/.staging/` (and its lock in
//! `data/.locks/`). Repository ids never start with `.` (see
//! `RepoRegistry::validate_repo_id`), so no repository can be mistaken for a
//! rebuild's leftovers, whatever its folder is called.
//!
//! **The swap.** Where the platform can exchange two directories in one
//! system call (`renameat2(RENAME_EXCHANGE)` on Linux, `renamex_np(RENAME_SWAP)`
//! on macOS), the live path never disappears. Elsewhere — or when the file
//! system refuses the exchange — it is two renames (live →
//! `.staging/old-…`, staging → live, the first undone if the second fails),
//! so for an instant the live path does not exist.
//!
//! **The journal.** Before the swap, `.staging/swap-…` records the live
//! directory's name; once the new index is in place (and the renames are
//! flushed), a second line `committed` is added, and only then is the
//! previous index deleted. Journal, renames and marker are each made durable
//! (file and directory `fsync`) before the next step relies on them, so the
//! order holds across a power loss. The journal is kept as long as an
//! `old-` directory exists.
//!
//! **Recovery.** A process killed mid-build (no `Drop`) leaves its `build-`
//! directory behind; one killed during the swap, or whose undo failed,
//! leaves the only copy of the previous index in `old-`. [`recover_leftovers`]
//! — run when a staging directory is created, when the MCP server starts and
//! when the CLI starts — handles the leftovers of rebuilds that no longer
//! run. If the journal says `committed`, `old-` is garbage and deleted.
//! Otherwise `old-` is the last index known to be complete: whatever is at
//! the live path (nothing, an empty directory a reader recreated, or an
//! unconfirmed new index) is moved to `.staging/displaced-…` and `old-` is
//! renamed back. Everything else is deleted. Rebuilds still running are
//! never touched.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// The directory below `data/` that holds every rebuild's entries.
pub const STAGING_DIR: &str = ".staging";

const BUILD_PREFIX: &str = "build-";
const OLD_PREFIX: &str = "old-";
const SWAP_PREFIX: &str = "swap-";
/// Where recovery moves a live directory it replaces with the `old-` index
/// of an unfinished swap.
const DISPLACED_PREFIX: &str = "displaced-";

/// The second journal line, written once the new index is in place: from
/// then on the `old-` directory is garbage. Without it, recovery puts the
/// `old-` index back, whatever the live path holds (a reader may have
/// recreated it empty).
const COMMITTED: &str = "committed";

/// Distinguishes staging directories created by one process.
static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

/// Suffixes of the rebuilds this process is running: its own other entries
/// are leftovers of failed rebuilds, recoverable like a dead process's.
static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Whether `name` (an entry of `data/`) belongs to Myceliums itself — the
/// rebuild directory or the lock directory — rather than to a repository.
pub fn is_internal_entry(name: &str) -> bool {
    name == STAGING_DIR || name == crate::lock::LOCKS_DIR
}

/// The `<pid>-…` suffix of a `build-`, `old-` or `swap-` entry.
fn leftover_suffix(name: &str) -> Option<&str> {
    let suffix = [BUILD_PREFIX, OLD_PREFIX, SWAP_PREFIX, DISPLACED_PREFIX]
        .iter()
        .find_map(|p| name.strip_prefix(p))?;
    suffix_pid(suffix).map(|_| suffix)
}

/// The PID that created the entry with this suffix.
fn suffix_pid(suffix: &str) -> Option<u32> {
    suffix.split('-').next()?.parse().ok()
}

/// Whether the rebuild with this suffix is still running.
fn is_running(suffix: &str) -> bool {
    match suffix_pid(suffix) {
        Some(pid) if pid == std::process::id() => ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(suffix),
        Some(pid) => crate::lock::is_process_alive(pid),
        None => true,
    }
}

/// A staging directory for a new index of one repository.
#[derive(Debug)]
pub struct StagedIndex {
    staging: PathBuf,
    live: PathBuf,
    suffix: String,
    committed: bool,
}

/// The file-system operations of a swap (tests inject failures).
struct SwapOps<'a> {
    rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
    exchange: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
    /// Writes the journal durably.
    write: &'a dyn Fn(&Path, &[u8]) -> std::io::Result<()>,
}

const REAL_OPS: SwapOps<'static> = SwapOps {
    rename: &|from, to| std::fs::rename(from, to),
    exchange: &exchange,
    write: &write_durable,
};

/// Writes `bytes` to `path` and flushes the file and its directory entry to
/// disk before returning.
fn write_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_dir(path.parent().unwrap_or(Path::new(".")))
}

/// Flushes a directory's entries (renames, creations) to disk.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// Directory entries cannot be flushed this way here; a no-op.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

impl StagedIndex {
    /// Creates an empty staging directory for the index at `live` (usually
    /// [`myceliums_storage::RepoRegistry::repo_db_path`]). The live directory
    /// need not exist. Leftovers of rebuilds that no longer run are
    /// recovered first (see [`recover_leftovers`]).
    pub fn create(live: &Path) -> Result<Self> {
        let data_root = live
            .parent()
            .context("the index directory has no parent directory")?;
        live.file_name()
            .context("the index directory has no name")?;
        let root = data_root.join(STAGING_DIR);
        std::fs::create_dir_all(&root)
            .with_context(|| format!("cannot create {}", root.display()))?;
        recover_leftovers(data_root);
        let suffix = format!(
            "{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        );
        let staging = root.join(format!("{BUILD_PREFIX}{suffix}"));
        std::fs::create_dir(&staging)
            .with_context(|| format!("cannot create {}", staging.display()))?;
        ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(suffix.clone());
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

    fn entry(&self, prefix: &str) -> PathBuf {
        self.staging
            .parent()
            .expect("staging dirs live in .staging")
            .join(format!("{prefix}{}", self.suffix))
    }

    /// Replaces the live index with the staged one. Every handle on either
    /// directory (open stores) should be closed first: renaming a directory
    /// with open files fails on Windows.
    ///
    /// On error the staging directory is deleted and the live index is
    /// unchanged — or, if even undoing a half-done swap failed, the previous
    /// index waits in `.staging/old-…` with its journal and is put back by
    /// the next [`recover_leftovers`].
    pub fn commit(self) -> Result<()> {
        self.commit_with(&REAL_OPS)
    }

    fn commit_with(mut self, ops: &SwapOps<'_>) -> Result<()> {
        let journal = self.entry(SWAP_PREFIX);
        let old = self.entry(OLD_PREFIX);
        let data_root = self.live.parent().expect("checked in create").to_path_buf();
        let live_name = self.live.file_name().expect("checked in create");
        // 1. The journal is on disk before anything moves.
        (ops.write)(&journal, live_name.as_encoded_bytes())
            .context("cannot write the swap journal")?;
        // 2. The swap, flushed. `previous` is where the previous index now
        //    sits (`old-…`, or the staging path after an exchange).
        let previous = match self.swap(&old, ops) {
            Ok(previous) => previous,
            Err(e) => {
                self.forget_journal_unless_old_remains(&journal, &old);
                return Err(e);
            }
        };
        self.committed = true;
        let flushed = sync_dir(&data_root).and_then(|()| sync_dir(&journal_dir(&journal)));
        // 3. The marker, durable, before the previous index may go.
        let marked = flushed.and_then(|()| {
            let mut text = live_name.as_encoded_bytes().to_vec();
            text.extend_from_slice(format!("\n{COMMITTED}\n").as_bytes());
            (ops.write)(&journal, &text)
        });
        if let Err(e) = marked {
            // Unmarked, recovery puts the previous index back: older, but
            // complete, and it matches the registry, which the caller only
            // updates after a successful commit.
            return Err(anyhow::Error::from(e).context(
                "the new index is in place but its swap could not be recorded as committed; \
                 the previous index is kept and restored by the next recovery",
            ));
        }
        // 4. Only now the previous index goes.
        if let Some(previous) = previous {
            remove_best_effort(&previous, "previous index");
        }
        self.forget_journal_unless_old_remains(&journal, &old);
        Ok(())
    }

    /// Deletes the journal unless an `old-` directory still needs it.
    fn forget_journal_unless_old_remains(&self, journal: &Path, old: &Path) {
        if old.exists() {
            tracing::warn!(
                "{} is kept with its journal {} for recovery",
                old.display(),
                journal.display()
            );
        } else if journal.exists() {
            if let Err(e) = std::fs::remove_file(journal) {
                tracing::warn!("cannot delete the swap journal {}: {e}", journal.display());
            }
        }
    }

    /// Moves the staged index to the live path. Returns where the previous
    /// index now sits, if there was one.
    fn swap(&self, old: &Path, ops: &SwapOps<'_>) -> Result<Option<PathBuf>> {
        if !self.live.exists() {
            (ops.rename)(&self.staging, &self.live).with_context(|| {
                format!("cannot move the new index into {}", self.live.display())
            })?;
            return Ok(None);
        }
        // One system call: the live path never disappears. The staging path
        // then holds the previous index; move it to `old-`, where recovery
        // looks for it.
        match (ops.exchange)(&self.staging, &self.live) {
            Ok(()) => {
                return Ok(Some(match (ops.rename)(&self.staging, old) {
                    Ok(()) => old.to_path_buf(),
                    Err(_) => self.staging.clone(),
                }))
            }
            Err(e) => tracing::debug!("atomic exchange unavailable ({e}); swapping by renames"),
        }
        (ops.rename)(&self.live, old).with_context(|| {
            format!(
                "cannot move the previous index {} aside",
                self.live.display()
            )
        })?;
        if let Err(e) = (ops.rename)(&self.staging, &self.live) {
            if let Err(undo) = (ops.rename)(old, &self.live) {
                tracing::error!(
                    "cannot restore the previous index {} from {}: {undo}; \
                     the next analysis, server start or myc command restores it",
                    self.live.display(),
                    old.display()
                );
            }
            return Err(e).with_context(|| {
                format!("cannot move the new index into {}", self.live.display())
            });
        }
        Ok(Some(old.to_path_buf()))
    }
}

fn journal_dir(journal: &Path) -> PathBuf {
    journal.parent().unwrap_or(Path::new(".")).to_path_buf()
}

impl Drop for StagedIndex {
    fn drop(&mut self) {
        if !self.committed && self.staging.exists() {
            remove_best_effort(&self.staging, "unfinished index");
        }
        ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.suffix);
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

/// Recovers the leftovers of rebuilds that no longer run, below `data_root`
/// (the `data/` directory of a data home): an `old-` index whose live
/// directory is missing is renamed back, every other leftover is deleted.
/// Rebuilds still running — in any process, this one included — are left
/// alone. Never fails: problems are logged.
pub fn recover_leftovers(data_root: &Path) -> Vec<Recovered> {
    recover_leftovers_where(data_root, &is_running)
}

fn recover_leftovers_where(data_root: &Path, running: &dyn Fn(&str) -> bool) -> Vec<Recovered> {
    let root = data_root.join(STAGING_DIR);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut suffixes: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            leftover_suffix(&name).map(str::to_string)
        })
        .filter(|s| !running(s))
        .collect();
    suffixes.sort();
    suffixes.dedup();

    let mut done = Vec::new();
    for suffix in suffixes {
        let build = root.join(format!("{BUILD_PREFIX}{suffix}"));
        let old = root.join(format!("{OLD_PREFIX}{suffix}"));
        let journal = root.join(format!("{SWAP_PREFIX}{suffix}"));
        let displaced = root.join(format!("{DISPLACED_PREFIX}{suffix}"));
        // The journal names one directory directly below `data/`, then
        // possibly says the swap committed.
        let text = std::fs::read(&journal)
            .ok()
            .and_then(|t| String::from_utf8(t).ok())
            .unwrap_or_default();
        let mut lines = text.lines();
        let live = lines
            .next()
            .filter(|name| {
                let mut parts = Path::new(name).components();
                matches!(parts.next(), Some(Component::Normal(_)))
                    && parts.next().is_none()
                    && !is_internal_entry(name)
            })
            .map(|name| data_root.join(name));
        let committed = lines.any(|l| l == COMMITTED);

        if old.exists() {
            match &live {
                Some(_) if committed => {
                    remove_best_effort(&old, "previous index");
                    done.push(Recovered::Removed(old.clone()));
                }
                Some(live) => {
                    // Unfinished swap: `old-` is the last complete index.
                    // Whatever is at the live path now — nothing, an empty
                    // directory a reader recreated, or an unconfirmed new
                    // index — is moved aside, and `old-` goes back.
                    // A stale `displaced-` from an earlier attempt must not
                    // block this one: take the first free name.
                    let displaced = (0..)
                        .map(|n| match n {
                            0 => displaced.clone(),
                            n => root.join(format!("{DISPLACED_PREFIX}{suffix}-{n}")),
                        })
                        .find(|p| !p.exists())
                        .expect("an unused name exists");
                    if live.exists() {
                        if let Err(e) = std::fs::rename(live, &displaced) {
                            tracing::error!(
                                "cannot move {} aside to restore {}: {e}",
                                live.display(),
                                old.display()
                            );
                            continue;
                        }
                    }
                    if let Err(e) = std::fs::rename(&old, live) {
                        tracing::error!(
                            "cannot restore the index {} from {}: {e}",
                            live.display(),
                            old.display()
                        );
                        continue;
                    }
                    if let Err(e) = sync_dir(data_root).and_then(|()| sync_dir(&root)) {
                        tracing::warn!("cannot flush the restored index to disk: {e}");
                    }
                    tracing::warn!(
                        "restored the index {} from an interrupted rebuild",
                        live.display()
                    );
                    done.push(Recovered::Restored(live.clone()));
                    if displaced.exists() {
                        remove_best_effort(&displaced, "replaced directory");
                        done.push(Recovered::Removed(displaced));
                    }
                }
                None => {
                    // Without a usable journal nobody knows whose index this
                    // is: keep it rather than guess.
                    tracing::warn!(
                        "{} has no usable swap journal; leaving it for manual inspection",
                        old.display()
                    );
                }
            }
        }
        if build.exists() {
            remove_best_effort(&build, "unfinished index");
            done.push(Recovered::Removed(build));
        }
        if displaced.exists() && !old.exists() {
            remove_best_effort(&displaced, "replaced directory");
            done.push(Recovered::Removed(displaced));
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

    /// Rename that fails for `from == refuse` (or for every `from` in it).
    fn refusing<'a>(refuse: &'a [&'a Path]) -> impl Fn(&Path, &Path) -> std::io::Result<()> + 'a {
        move |from, to| {
            if refuse.contains(&from) {
                Err(std::io::Error::other("injected"))
            } else {
                std::fs::rename(from, to)
            }
        }
    }

    fn no_exchange(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    #[test]
    fn dropping_without_commit_keeps_the_live_index_and_removes_the_staging_dir() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");

        let staged = StagedIndex::create(&live).unwrap();
        assert!(staged.path().starts_with(root.join(STAGING_DIR)));
        write(&staged.path().join("symbols.lance"), "half-written");
        drop(staged);

        assert_eq!(read(&live.join("symbols.lance")), "old");
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    #[test]
    fn commit_swaps_in_the_new_index() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        write(&live.join("stale-table.lance"), "old");

        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        staged.commit().unwrap();

        assert_eq!(read(&live.join("symbols.lance")), "new");
        assert!(!live.join("stale-table.lance").exists());
        assert!(
            entries(&root.join(STAGING_DIR)).is_empty(),
            "no staging, old or journal entry is left behind"
        );
    }

    #[test]
    fn the_two_rename_fallback_swaps() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        let rename = refusing(&[]);
        let ops = SwapOps {
            rename: &rename,
            exchange: &no_exchange,
            write: &write_durable,
        };
        staged.commit_with(&ops).unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "new");
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    #[test]
    fn a_failed_second_rename_is_undone() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        let staging = staged.path().to_path_buf();
        let refuse = [staging.as_path()];
        let rename = refusing(&refuse);
        let ops = SwapOps {
            rename: &rename,
            exchange: &no_exchange,
            write: &write_durable,
        };
        let err = staged.commit_with(&ops).unwrap_err();
        assert!(format!("{err:#}").contains("injected"), "{err:#}");
        assert_eq!(read(&live.join("symbols.lance")), "old", "undone");
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    #[test]
    fn a_failed_undo_keeps_the_journal_and_recovery_restores_the_index() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        let staging = staged.path().to_path_buf();
        let old = staged.entry(OLD_PREFIX);
        // The move into place and the undo both fail.
        let refuse = [staging.as_path(), old.as_path()];
        let rename = refusing(&refuse);
        let ops = SwapOps {
            rename: &rename,
            exchange: &no_exchange,
            write: &write_durable,
        };
        staged.commit_with(&ops).unwrap_err();
        assert!(!live.exists(), "the live index is gone");
        let left = entries(&root.join(STAGING_DIR));
        assert!(left.iter().any(|n| n.starts_with(OLD_PREFIX)), "{left:?}");
        assert!(left.iter().any(|n| n.starts_with(SWAP_PREFIX)), "{left:?}");

        // This process's rebuild has ended: the next recovery (here the
        // next rebuild's) puts the index back.
        let next = StagedIndex::create(&live).unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "old", "restored");
        drop(next);
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    /// After a failed swap the live path is missing; any read that opens the
    /// store (LanceDB connect) may recreate it empty. Recovery must still
    /// bring the previous index back, not delete it as superseded.
    #[tokio::test]
    async fn a_live_dir_recreated_by_a_reader_after_a_failed_swap_loses_nothing() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        let staging = staged.path().to_path_buf();
        let old = staged.entry(OLD_PREFIX);
        let refuse = [staging.as_path(), old.as_path()];
        let rename = refusing(&refuse);
        let ops = SwapOps {
            rename: &rename,
            exchange: &no_exchange,
            write: &write_durable,
        };
        staged.commit_with(&ops).unwrap_err();
        assert!(!live.exists());

        // A reader opens the store, then writes nothing.
        drop(myceliums_storage::Store::open(&live, "repo").await.unwrap());
        std::fs::create_dir_all(&live).unwrap();

        let next = StagedIndex::create(&live).unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "old", "restored");
        drop(next);
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    #[test]
    fn a_failed_commit_marker_keeps_the_previous_index_for_recovery() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let live = root.join("repo");
        write(&live.join("symbols.lance"), "old");
        let staged = StagedIndex::create(&live).unwrap();
        write(&staged.path().join("symbols.lance"), "new");
        let rename = refusing(&[]);
        let fail_marker = |path: &Path, bytes: &[u8]| {
            if String::from_utf8_lossy(bytes).contains(COMMITTED) {
                Err(std::io::Error::other("disk full"))
            } else {
                write_durable(path, bytes)
            }
        };
        let ops = SwapOps {
            rename: &rename,
            exchange: &exchange,
            write: &fail_marker,
        };
        let err = staged.commit_with(&ops).unwrap_err();
        assert!(format!("{err:#}").contains("disk full"), "{err:#}");
        assert_eq!(read(&live.join("symbols.lance")), "new", "swapped");
        let left = entries(&root.join(STAGING_DIR));
        assert!(
            left.iter().any(|n| n.starts_with(OLD_PREFIX)),
            "old kept: {left:?}"
        );
        assert!(left.iter().any(|n| n.starts_with(SWAP_PREFIX)), "{left:?}");

        // Unmarked: recovery brings the previous index back.
        let next = StagedIndex::create(&live).unwrap();
        assert_eq!(read(&live.join("symbols.lance")), "old");
        drop(next);
        assert!(entries(&root.join(STAGING_DIR)).is_empty());
    }

    #[test]
    fn an_unmarked_swap_is_undone_even_when_the_new_index_is_live() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let st = root.join(STAGING_DIR);
        // Killed after both renames, before the marker: the full new index
        // is live, the previous one in old-.
        write(&root.join("repo/symbols.lance"), "new");
        write(&st.join(format!("swap-{DEAD}-1-0")), "repo");
        write(&st.join(format!("old-{DEAD}-1-0/symbols.lance")), "old");
        // A stale displaced- from an earlier, interrupted recovery.
        write(&st.join(format!("displaced-{DEAD}-1-0/x")), "stale");

        let done = recover_leftovers_where(&root, &|_| false);
        assert!(
            done.contains(&Recovered::Restored(root.join("repo"))),
            "{done:?}"
        );
        assert_eq!(read(&root.join("repo/symbols.lance")), "old");
        assert!(entries(&st).is_empty(), "{:?}", entries(&st));
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
        let st = root.join(STAGING_DIR);

        // Killed mid-build: only the staging directory.
        write(&st.join(format!("build-{DEAD}-1-0/symbols.lance")), "half");
        // Killed between the renames: live gone, journal + old + build.
        write(&st.join(format!("swap-{DEAD}-2-0")), "repo-a");
        write(&st.join(format!("old-{DEAD}-2-0/symbols.lance")), "a-old");
        write(&st.join(format!("build-{DEAD}-2-0/symbols.lance")), "a-new");
        // Killed after the committed swap, before cleanup: live is the
        // new index.
        write(&root.join("repo-b/symbols.lance"), "b-new");
        write(&st.join(format!("swap-{DEAD}-3-0")), "repo-b\ncommitted\n");
        write(&st.join(format!("old-{DEAD}-3-0/symbols.lance")), "b-old");
        // Killed between the renames, and a reader has since recreated an
        // empty live directory: the old index comes back.
        std::fs::create_dir_all(root.join("repo-c")).unwrap();
        write(&st.join(format!("swap-{DEAD}-6-0")), "repo-c");
        write(&st.join(format!("old-{DEAD}-6-0/symbols.lance")), "c-old");
        // A running process's build: untouched.
        let busy = format!("build-{}-4-0", std::process::id());
        ACTIVE
            .lock()
            .unwrap()
            .insert(busy["build-".len()..].to_string());
        write(&st.join(format!("{busy}/symbols.lance")), "busy");
        // No journal: whose index is this? Kept.
        write(&st.join(format!("old-{DEAD}-5-0/symbols.lance")), "orphan");

        let done = recover_leftovers(&root);
        assert!(
            done.contains(&Recovered::Restored(root.join("repo-a"))),
            "{done:?}"
        );
        assert_eq!(read(&root.join("repo-a/symbols.lance")), "a-old");
        assert_eq!(read(&root.join("repo-b/symbols.lance")), "b-new");
        assert!(done.contains(&Recovered::Restored(root.join("repo-c"))));
        assert_eq!(read(&root.join("repo-c/symbols.lance")), "c-old");
        assert_eq!(entries(&st), vec![busy.clone(), format!("old-{DEAD}-5-0")]);
        assert!(recover_leftovers(&root).is_empty(), "idempotent");
        ACTIVE.lock().unwrap().remove(&busy["build-".len()..]);
    }

    /// A project folder named like a rebuild entry is an ordinary
    /// repository: its index lives directly in `data/` under a sanitized id,
    /// never in `.staging/`, and recovery leaves it alone.
    #[test]
    fn a_repository_named_like_a_leftover_is_never_touched() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let id = crate::analyzer::repo_id_from_path(Path::new("/projects/.build-123"));
        assert!(id.starts_with("_build-123-"), "{id}");
        myceliums_storage::RepoRegistry::validate_repo_id(&id).unwrap();
        // Even a legacy index registered as `.build-123-…` stays put.
        for name in [id.as_str(), ".build-123-1a2b3c4d", ".old-123-1-0"] {
            write(&root.join(name).join("symbols.lance"), "live");
        }
        write(&root.join(STAGING_DIR).join("build-123-9-0/x"), "dead");
        recover_leftovers_where(&root, &|_| false);
        for name in [id.as_str(), ".build-123-1a2b3c4d", ".old-123-1-0"] {
            assert_eq!(read(&root.join(name).join("symbols.lance")), "live");
        }
        assert!(!root.join(STAGING_DIR).join("build-123-9-0").exists());
    }

    #[test]
    fn internal_entries_are_recognised() {
        assert!(is_internal_entry(".locks"));
        assert!(is_internal_entry(".staging"));
        assert!(!is_internal_entry(".build-123-456-0"));
        assert!(!is_internal_entry("my-repo-1a2b3c4d"));
        assert!(!is_internal_entry(".dotfiles-1a2b3c4d"));
    }

    #[test]
    fn a_journal_cannot_point_outside_the_data_directory() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("data");
        let st = root.join(STAGING_DIR);
        write(&st.join(format!("swap-{DEAD}-1-0")), "../escape");
        write(&st.join(format!("old-{DEAD}-1-0/symbols.lance")), "x");
        write(&st.join(format!("swap-{DEAD}-2-0")), ".locks");
        write(&st.join(format!("old-{DEAD}-2-0/symbols.lance")), "y");
        recover_leftovers_where(&root, &|_| false);
        assert!(!data.path().join("escape").exists());
        assert!(!root.join(".locks").exists());
        assert!(st.join(format!("old-{DEAD}-1-0")).exists());
        assert!(st.join(format!("old-{DEAD}-2-0")).exists());
    }
}
