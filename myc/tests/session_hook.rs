//! The Claude Code SessionStart hook, `myc session . --yes` (#152), run as
//! Claude Code runs it: stdout must be exactly one JSON object whose
//! `hookSpecificOutput.additionalContext` reaches the model, and the process
//! must exit 0 whatever goes wrong.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
fn session_hook(data: &Path, cwd: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(["session", ".", "--yes"])
        .args(extra)
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: some code paths derive paths from HOME.
        .env("HOME", data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run myc")
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
fn indexes_then_reports_a_budgeted_context_line_for_the_model() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("data");
    let project = git_project(tmp.path());

    // No index yet: the hook analyzes, and the analysis output (which
    // `myc analyze` prints on stdout) must not leak into the hook's stdout.
    let first = session_hook(&data, &project, &["--timeout", "300"]);
    let v = hook_output(&first);
    assert!(
        v["systemMessage"]
            .as_str()
            .unwrap()
            .contains(" analyzed | "),
        "{v}"
    );
    let ctx = context(&v);
    assert!(ctx.contains("hookproj-"), "{ctx}");
    assert!(ctx.contains("= HEAD"), "{ctx}");
    assert!(ctx.contains("find_callers"), "{ctx}");
    assert!(ctx.len() <= 280 && !ctx.contains('\n'), "{ctx}");
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("[myceliums] hookproj analyzed"),
        "human status on stderr: {first:?}"
    );

    // Index current: the cheap path.
    let second = hook_output(&session_hook(&data, &project, &[]));
    assert!(
        second["systemMessage"]
            .as_str()
            .unwrap()
            .contains(" ready | "),
        "{second}"
    );
    assert_eq!(context(&second), ctx);
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
