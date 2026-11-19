//! Analysis lock to prevent concurrent indexing of the same repository.
//!
//! The lock file lives at `~/.myceliums/data/.locks/{repo_id}.lock` and
//! contains the PID plus an RFC 3339 timestamp. A stale lock (dead process)
//! is cleaned up automatically.
//!
//! The lock sits outside the index directory `data/{repo_id}/` on purpose: a
//! rebuild replaces that directory as a whole (see [`crate::staging`]), and
//! taking the lock never creates it.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// A guard that holds an analysis lock file. The lock is released when the
/// guard is dropped.
pub struct AnalysisLock {
    lock_path: PathBuf,
}

/// Result of attempting to acquire a lock.
pub enum LockOutcome {
    /// Lock acquired — caller should proceed with analysis.
    Acquired(AnalysisLock),
    /// Another live process already holds the lock.
    AlreadyRunning { pid: u32 },
}

impl AnalysisLock {
    /// Try to acquire the analysis lock for `repo_id`.
    ///
    /// - Returns `LockOutcome::Acquired` if the lock was obtained.
    /// - Returns `LockOutcome::AlreadyRunning` if another live process holds it.
    /// - Cleans up stale locks left by dead processes automatically.
    pub fn acquire(data_dir: &Path, repo_id: &str) -> Result<LockOutcome> {
        let lock_dir = locks_dir(data_dir);
        fs::create_dir_all(&lock_dir)
            .with_context(|| format!("Failed to create lock dir: {}", lock_dir.display()))?;

        let lock_path = lock_path(data_dir, repo_id);

        // Try to detect an existing lock
        if lock_path.exists() {
            if let Ok(content) = fs::read_to_string(&lock_path) {
                if let Some(pid) = parse_lock_pid(&content) {
                    if is_process_alive(pid) {
                        return Ok(LockOutcome::AlreadyRunning { pid });
                    }
                    // Stale lock — process is dead, clean it up
                    tracing::warn!(
                        "Removing stale analysis lock (PID {} is no longer running)",
                        pid
                    );
                }
            }
            // Lock file exists but is unreadable or unparseable — remove it
            let _ = fs::remove_file(&lock_path);
        }

        // Atomically create the lock file. create_new(true) fails if the file
        // already exists, avoiding TOCTOU races between the check above and
        // the write below.
        let lock_content = format!(
            "{}\n{}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_file) => {
                // File created atomically — now write contents
                fs::write(&lock_path, lock_content)?;
                Ok(LockOutcome::Acquired(AnalysisLock { lock_path }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Race: another process grabbed the lock between our check and
                // our create. Read the winner's PID.
                if let Ok(content) = fs::read_to_string(&lock_path) {
                    if let Some(pid) = parse_lock_pid(&content) {
                        return Ok(LockOutcome::AlreadyRunning { pid });
                    }
                }
                // Unreadable lock from the race winner — treat as locked
                Ok(LockOutcome::AlreadyRunning { pid: 0 })
            }
            Err(e) => Err(e).context("Failed to create analysis lock file"),
        }
    }

    /// Explicitly release the lock (also happens on Drop).
    pub fn release(self) {
        // Drop will handle it
        drop(self);
    }
}

impl Drop for AnalysisLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// The directory of the analysis locks: `data/.locks` under the data home.
pub fn locks_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("data").join(LOCKS_DIR)
}

/// The name of [`locks_dir`] inside `data/`.
pub const LOCKS_DIR: &str = ".locks";

/// The lock file of `repo_id`.
pub fn lock_path(data_dir: &Path, repo_id: &str) -> PathBuf {
    locks_dir(data_dir).join(format!("{repo_id}.lock"))
}

/// The PID on the first line of a lock file.
pub fn parse_lock_pid(content: &str) -> Option<u32> {
    content.lines().next()?.trim().parse().ok()
}

/// Whether a process with this PID exists.
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // Signal 0 checks for existence without sending anything; EPERM means
    // the process exists but belongs to someone else.
    // SAFETY: kill with signal 0 has no side effects.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether a process with this PID exists.
#[cfg(not(unix))]
pub fn is_process_alive(pid: u32) -> bool {
    // On Unix, kill(pid, 0) checks if the process exists without sending a signal.
    // A return code of 0 means the process is alive.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // First acquire should succeed
        let outcome = AnalysisLock::acquire(data_dir, "test-repo").unwrap();
        assert!(matches!(outcome, LockOutcome::Acquired(_)));

        // Lock file should exist
        let lock_path = data_dir.join("data/.locks/test-repo.lock");
        assert!(lock_path.exists());
        assert!(
            !data_dir.join("data/test-repo").exists(),
            "taking the lock does not create the index directory"
        );

        // Second acquire should report AlreadyRunning (our own PID is alive)
        let outcome2 = AnalysisLock::acquire(data_dir, "test-repo").unwrap();
        assert!(matches!(outcome2, LockOutcome::AlreadyRunning { .. }));

        // Drop the first lock
        if let LockOutcome::Acquired(lock) = outcome {
            drop(lock);
        }

        // Now acquire should succeed again
        let outcome3 = AnalysisLock::acquire(data_dir, "test-repo").unwrap();
        assert!(matches!(outcome3, LockOutcome::Acquired(_)));
    }

    #[test]
    fn test_stale_lock_cleanup() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let lock_dir = data_dir.join("data/.locks");
        fs::create_dir_all(&lock_dir).unwrap();

        // Write a lock file with a PID that definitely doesn't exist
        let fake_pid = 99999999;
        let lock_path = lock_dir.join("test-repo.lock");
        fs::write(&lock_path, format!("{}\n2026-01-01T00:00:00Z", fake_pid)).unwrap();

        // Acquire should clean up the stale lock and succeed
        let outcome = AnalysisLock::acquire(data_dir, "test-repo").unwrap();
        assert!(matches!(outcome, LockOutcome::Acquired(_)));
    }

    #[test]
    fn liveness_of_this_and_of_a_dead_process() {
        assert!(is_process_alive(std::process::id()));
        assert!(!is_process_alive(99_999_999));
        assert!(!is_process_alive(0));
    }

    #[test]
    fn test_parse_lock_pid() {
        assert_eq!(parse_lock_pid("12345\n2026-01-01T00:00:00Z"), Some(12345));
        assert_eq!(parse_lock_pid("12345"), Some(12345));
        assert_eq!(parse_lock_pid(""), None);
        assert_eq!(parse_lock_pid("not-a-number"), None);
    }

    #[test]
    fn test_different_repos_independent() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        let outcome_a = AnalysisLock::acquire(data_dir, "repo-a").unwrap();
        assert!(matches!(outcome_a, LockOutcome::Acquired(_)));

        // Different repo should get its own lock independently
        let outcome_b = AnalysisLock::acquire(data_dir, "repo-b").unwrap();
        assert!(matches!(outcome_b, LockOutcome::Acquired(_)));
    }
}
