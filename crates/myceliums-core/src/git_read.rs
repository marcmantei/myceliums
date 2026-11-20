//! Read-only git invocations that are safe to run beside the user's own git
//! commands: no index lock, and a timeout.
//!
//! `git status` and `git diff` refresh the index's stat cache and take
//! `.git/index.lock` to write it back. A tool that runs them while an agent
//! commits can make the commit fail on the lock, and killing a slow one can
//! leave the lock behind. `--no-optional-locks` turns that refresh off, which
//! also makes the timeout kill safe.

use crate::error::bail;
use crate::error::Result;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// `git --no-optional-locks -C <dir> <args>`, with stdin closed and output
/// piped.
pub fn git_read_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs a read-only git command in `dir` and returns its stdout.
///
/// Fails when git cannot start, exits unsuccessfully (the error carries its
/// stderr), or does not finish within `timeout` — then the process is killed.
pub fn git_read(dir: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = git_read_command(dir, args).spawn()?;
    // Drain both pipes on helper threads so a large output cannot fill a
    // pipe and block git while we wait for it.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let out = match stdout.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            // Safe to kill: with --no-optional-locks no lock file is held.
            let _ = child.kill();
            let _ = child.wait();
            bail!("git {} timed out after {:?}", args.join(" "), timeout);
        }
    };
    let status = child.wait()?;
    if !status.success() {
        let err = stderr
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        bail!("git {} failed: {}", args.join(" "), err.trim());
    }
    Ok(out)
}

/// Reads a child pipe to the end on its own thread.
fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).into_owned();
        }
        let _ = tx.send(text);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_never_take_the_index_lock() {
        let command = git_read_command(Path::new("."), &["status"]);
        let args: Vec<_> = command.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(args, ["--no-optional-locks", "-C", ".", "status"]);
    }

    #[test]
    fn failures_and_timeouts_are_errors() {
        let err = git_read(
            Path::new("/definitely/not/a/repo"),
            &["rev-parse", "HEAD"],
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("git rev-parse HEAD failed"),
            "{err}"
        );

        let dir = env!("CARGO_MANIFEST_DIR");
        let err = git_read(Path::new(dir), &["status"], Duration::ZERO).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[test]
    fn output_is_returned() {
        let dir = env!("CARGO_MANIFEST_DIR");
        let version = git_read(Path::new(dir), &["--version"], Duration::from_secs(10)).unwrap();
        assert!(version.starts_with("git version"), "{version}");
    }
}
