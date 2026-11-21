//! The Claude Code SessionStart hook, `myc session . --yes` (#152), run as
//! Claude Code runs it: stdout must be exactly one JSON object whose
//! `hookSpecificOutput.additionalContext` reaches the model, the hook must
//! return at once (a re-index runs detached, under the #98 analysis lock),
//! and the process must exit 0 whatever goes wrong.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/sample-ts-project")
        .canonicalize()
        .expect("fixture exists")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// A committed copy of the TS fixture in its own git repository.
fn git_project(root: &Path) -> PathBuf {
    let project = root.join("hookproj");
    copy_dir(&fixture(), &project);
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(
        &project,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    project
}

/// Runs the hook the way the installed SessionStart handler does.
fn session_hook(data: &Path, cwd: &Path, env: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_myc"));
    cmd.args(["session", ".", "--yes", "--timeout", "300"])
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: some code paths derive paths from HOME.
        .env("HOME", data)
        .env_remove("MYC_SESSION_REINDEX_EXE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run myc")
}

/// Polls `cond` every 100 ms until it holds or `limit` passes.
fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cond()
}

/// Whether a process with this PID exists (a zombie counts).
fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// An executable script standing in for the background `myc analyze`.
#[cfg(unix)]
fn script(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-myc");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn repo_id(project: &Path) -> String {
    myceliums_core::analyzer::repo_id_from_path(&project.canonicalize().unwrap())
}

/// stdout must be one JSON object and nothing else: Claude Code adds
/// plain-text SessionStart stdout to the model's context.
fn hook_output(out: &Output) -> Value {
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let trimmed = text.trim();
    assert!(
        trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.lines().count() == 1,
        "stdout is not a single JSON line: {text}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(trimmed).expect("valid JSON");
    assert!(v["systemMessage"]
        .as_str()
        .unwrap()
        .starts_with("[myceliums] "));
    v
}

fn context(v: &Value) -> &str {
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "{v}"
    );
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext")
}

#[test]
fn builds_in_the_background_then_reports_a_fresh_index() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("data");
    let project = git_project(tmp.path());
    let id = repo_id(&project);

    // No index yet: the hook returns at once and says a build is running.
    let started = Instant::now();
    let first = session_hook(&data, &project, &[]);
    assert!(started.elapsed() < Duration::from_secs(5), "{first:?}");
    let v = hook_output(&first);
    let ctx = context(&v);
    assert!(ctx.contains(&id), "{ctx}");
    assert!(ctx.contains("building in the background"), "{ctx}");
    assert!(ctx.contains("find_callers"), "{ctx}");
    assert!(ctx.len() <= 280 && !ctx.contains('\n'), "{ctx}");
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("building in the background"),
        "human status on stderr: {first:?}"
    );

    // The detached `myc analyze` registers the index and drops its lock.
    let registry = data.join("repos.json");
    let lock = data.join("data").join(".locks").join(format!("{id}.lock"));
    let log = data.join("logs").join(format!("session-{id}.log"));
    let done = wait_until(Duration::from_secs(240), || {
        std::fs::read_to_string(&registry).is_ok_and(|r| r.contains(&id)) && !lock.exists()
    });
    assert!(
        done,
        "background analysis did not finish; log:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(log_text.contains("analyze"), "{log_text}");

    // Index current: the cheap path, with freshness.
    let second = hook_output(&session_hook(&data, &project, &[]));
    assert!(
        second["systemMessage"]
            .as_str()
            .unwrap()
            .contains(" ready | "),
        "{second}"
    );
    let ctx = context(&second);
    assert!(ctx.contains("= HEAD") && ctx.contains(&id), "{ctx}");
}

/// A slow re-index does not hold up the hook, and once it ends nothing is
/// left behind: the detached child is adopted and reaped by init.
#[cfg(unix)]
#[test]
fn a_slow_reindex_returns_at_once_and_leaves_no_zombie() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("data");
    let project = git_project(tmp.path());
    let pid_file = tmp.path().join("child.pid");
    let fake = script(
        tmp.path(),
        &format!(
            "echo $$ > '{}'; echo fake-analyze \"$@\"; sleep 3",
            pid_file.display()
        ),
    );

    let started = Instant::now();
    let out = session_hook(&data, &project, &[("MYC_SESSION_REINDEX_EXE", &fake)]);
    let elapsed = started.elapsed();
    let v = hook_output(&out);
    assert!(elapsed < Duration::from_secs(3), "hook waited {elapsed:?}");
    assert!(context(&v).contains("building in the background"), "{v}");

    assert!(wait_until(Duration::from_secs(10), || pid_file.exists()));
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_string();
    let log = data
        .join("logs")
        .join(format!("session-{}.log", repo_id(&project)));
    assert!(wait_until(Duration::from_secs(10), || {
        std::fs::read_to_string(&log).is_ok_and(|t| t.contains("fake-analyze analyze "))
    }));
    assert!(
        wait_until(Duration::from_secs(20), || !alive(&pid)),
        "child {pid} still exists (running or a zombie)"
    );
}

/// When another process holds the #98 analysis lock, the hook starts nothing.
#[cfg(unix)]
#[test]
fn a_held_analysis_lock_means_already_running() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("data");
    let project = git_project(tmp.path());
    let marker = tmp.path().join("spawned");
    let fake = script(tmp.path(), &format!("touch '{}'", marker.display()));
    let locks = data.join("data").join(".locks");
    std::fs::create_dir_all(&locks).unwrap();
    // This test process is alive, so the lock is live.
    std::fs::write(
        locks.join(format!("{}.lock", repo_id(&project))),
        format!("{}\n2026-11-21T00:00:00Z", std::process::id()),
    )
    .unwrap();

    let v = hook_output(&session_hook(
        &data,
        &project,
        &[("MYC_SESSION_REINDEX_EXE", &fake)],
    ));
    assert!(
        v["systemMessage"]
            .as_str()
            .unwrap()
            .contains("already running"),
        "{v}"
    );
    assert!(context(&v).contains("already running"), "{v}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!marker.exists(), "a second analysis was started");
}

#[test]
fn never_fails_the_session() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("data");

    // Not a git repository: skipped, no context, exit 0.
    let plain = tmp.path().join("plain");
    copy_dir(&fixture(), &plain);
    let v = hook_output(&session_hook(&data, &plain, &[]));
    assert!(
        v["systemMessage"].as_str().unwrap().contains("skipped"),
        "{v}"
    );
    assert!(v.get("hookSpecificOutput").is_none(), "{v}");

    // An unreadable registry: an error status, no context, exit 0.
    let project = git_project(tmp.path());
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("repos.json"), "{ not json").unwrap();
    let v = hook_output(&session_hook(&data, &project, &[]));
    assert!(
        v["systemMessage"].as_str().unwrap().contains("error"),
        "{v}"
    );
    assert!(v.get("hookSpecificOutput").is_none(), "{v}");

    // The re-index cannot be started: an error status, exit 0.
    std::fs::remove_file(data.join("repos.json")).unwrap();
    let missing = tmp.path().join("no-such-myc");
    let v = hook_output(&session_hook(
        &data,
        &project,
        &[("MYC_SESSION_REINDEX_EXE", &missing)],
    ));
    assert!(
        v["systemMessage"].as_str().unwrap().contains("error"),
        "{v}"
    );
    assert!(v.get("hookSpecificOutput").is_none(), "{v}");

    // A path that does not exist.
    let out = Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(["session", "does-not-exist", "--yes"])
        .current_dir(tmp.path())
        .env("MYCELIUMS_DATA_DIR", &data)
        .env("HOME", &data)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let v = hook_output(&out);
    assert!(v.get("hookSpecificOutput").is_none(), "{v}");
}
