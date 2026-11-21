//! `myc session --yes`: the Claude Code SessionStart hook (#152).
//!
//! Output contract, per the Claude Code hooks reference
//! <https://code.claude.com/docs/en/hooks#sessionstart> (checked 2026-11-20):
//!
//! * stdout carries exactly one JSON object and nothing else. Claude Code adds
//!   `hookSpecificOutput.additionalContext` (with `hookEventName:
//!   "SessionStart"`) to the model's context before the first prompt, and shows
//!   `systemMessage` to the user only. Plain-text stdout would also reach the
//!   model, so no analysis output may ever land on the hook's stdout.
//! * The context line is paid for in every session, so it is one line of at
//!   most [`CONTEXT_BUDGET_CHARS`] characters (about 60 tokens): repo id, index
//!   state (freshness from [`myceliums_core::navigation::index_freshness`]),
//!   and the core tools for structural questions. The docs ask for facts, not
//!   imperative "system" instructions, which can trip prompt-injection
//!   defences.
//! * The human status line goes to `systemMessage` (the user sees it in the
//!   transcript) and to stderr (for a run by hand; the installed hook sends
//!   stderr to /dev/null, and Claude never sees stderr from a hook that exits 0).
//! * It never fails the session: every error becomes a status line without
//!   context, and the process exits 0.
//!
//! Runtime: Claude Code waits for SessionStart hooks before the first model
//! response ("hooks block Claude's execution until they complete",
//! <https://code.claude.com/docs/en/hooks#run-hooks-in-the-background>), so the
//! hook never analyzes in the foreground. It reads the registry, runs the
//! cache check and one `git rev-parse HEAD` (bounded to 1.5 s by the core).
//! A missing or stale index is rebuilt by a detached `myc analyze` (its own
//! session via `setsid`, stdout and stderr appended to
//! `<data dir>/logs/session-<repo id>.log`), and the hook returns at once
//! with a context line that says so. `myc analyze` takes the #98 analysis lock
//! itself; when the lock is already held (MCP `analyze`, another session, a
//! manual run) the hook starts nothing and reports that a re-index is running.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context as _, Result};
use myceliums_core::analyzer;
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_core::lock;
use myceliums_core::navigation::{index_freshness, IndexFreshness};
use myceliums_storage::RepoRegistry;
use serde_json::{json, Value};

use crate::commands::analyze::{check_session_safeguards, enforce_strict_embeddings};
use crate::repo::{data_dir, registry_path};

/// Upper bound for the model-facing context line (~60 tokens).
pub(crate) const CONTEXT_BUDGET_CHARS: usize = 280;

/// Replaces the program the background re-index runs (default: this `myc`).
/// A test seam: the integration tests point it at a slow script.
pub(crate) const REINDEX_EXE_ENV: &str = "MYC_SESSION_REINDEX_EXE";

/// A log above this size is cut before the next background run appends.
const LOG_MAX_BYTES: u64 = 1 << 20;

/// What one hook run reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookReport {
    /// One line for the user (`systemMessage`, stderr).
    pub status: String,
    /// One line for the model (`additionalContext`); `None` on errors and
    /// skips.
    pub context: Option<String>,
}

impl HookReport {
    fn status_only(status: String) -> Self {
        Self {
            status,
            context: None,
        }
    }
}

/// The JSON object the hook prints on stdout.
pub(crate) fn hook_json(report: &HookReport) -> Value {
    let mut out = json!({ "systemMessage": report.status });
    if let Some(context) = &report.context {
        out["hookSpecificOutput"] = json!({
            "hookEventName": "SessionStart",
            "additionalContext": context,
        });
    }
    out
}

/// Prints the report: JSON on stdout, the status on stderr. Write errors (a
/// closed pipe) are ignored: the hook must not fail.
pub(crate) fn emit(report: &HookReport) {
    let _ = writeln!(std::io::stderr(), "{}", report.status);
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", hook_json(report));
    let _ = stdout.flush();
}

/// The index as the session sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum IndexState<'a> {
    /// Current per the cache check.
    Fresh(&'a IndexFreshness),
    /// Stale; this hook started a background re-index.
    Refreshing,
    /// Stale; a re-index holding the analysis lock is already running.
    RefreshRunning,
    /// No index; this hook started a background build.
    Building,
    /// No index; a build holding the analysis lock is already running.
    BuildRunning,
}

/// The model-facing line. `tools_hint` adds the core tools (off when AI
/// instructions were declined in the setup wizard).
pub(crate) fn context_line(repo_id: &str, state: IndexState<'_>, tools_hint: bool) -> String {
    let mut line = match state {
        IndexState::Fresh(f) => {
            format!("myceliums code graph of repo {repo_id} ({}).", f.summary())
        }
        IndexState::Refreshing => format!(
            "myceliums code graph of repo {repo_id}: index stale, re-indexing in the \
             background; results may be outdated until done."
        ),
        IndexState::RefreshRunning => format!(
            "myceliums code graph of repo {repo_id}: index stale, re-index already running; \
             results may be outdated until done."
        ),
        IndexState::Building => format!(
            "myceliums: no index of repo {repo_id} yet, building in the background; the graph \
             tools answer once it is done."
        ),
        IndexState::BuildRunning => format!(
            "myceliums: no index of repo {repo_id} yet, a build is already running; the graph \
             tools answer once it is done."
        ),
    };
    if tools_hint {
        line.push_str(
            " MCP tools find_callers, impact_of_change and search_code answer structural questions.",
        );
    }
    // A very long repository name must not blow the per-session budget.
    if line.len() > CONTEXT_BUDGET_CHARS {
        let mut end = CONTEXT_BUDGET_CHARS - '\u{2026}'.len_utf8();
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('\u{2026}');
    }
    line
}

/// The tools sentence is left out only when the user said no to AI
/// instructions in the setup wizard; `myc setup-claude` alone leaves
/// `instructions_enabled` at its default (false) without asking.
fn tools_hint() -> bool {
    let setup = myceliums_core::GlobalConfig::load(&data_dir())
        .unwrap_or_default()
        .setup;
    setup.instructions_enabled || !setup.completed
}

/// Runs the hook. Never returns an error: failures become the status line.
pub(crate) async fn run(path: &Path, strict_embeddings: bool, no_git_check: bool) -> HookReport {
    let name = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());
    match run_inner(path, &name, strict_embeddings, no_git_check).await {
        Ok(report) => report,
        Err(e) => HookReport::status_only(format!("[myceliums] {name} error | {e:#}")),
    }
}

async fn run_inner(
    path: &Path,
    name: &str,
    strict_embeddings: bool,
    no_git_check: bool,
) -> Result<HookReport> {
    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if let Err(e) = check_session_safeguards(&abs_path, no_git_check) {
        return Ok(HookReport::status_only(format!(
            "[myceliums] {name} skipped | {e}"
        )));
    }
    let repo_id = analyzer::repo_id_from_path(&abs_path);
    RepoRegistry::validate_repo_id(&repo_id)?;

    let registry = RepoRegistry::load(&registry_path())?;
    let indexed = match registry.get(&repo_id) {
        Some(info) => match cache::check_cache(info, &abs_path, &CacheCheckConfig::default()) {
            CacheDecision::UseCached { .. } => {
                // A fresh cache must not launder a partial index into a
                // "ready" report when strict coverage was demanded.
                if strict_embeddings {
                    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                    enforce_strict_embeddings(&db_path, &repo_id).await?;
                }
                let freshness = index_freshness(info);
                return Ok(HookReport {
                    status: format!(
                        "[myceliums] {name} ready | {} files \u{00b7} {} symbols",
                        info.file_count, info.symbol_count
                    ),
                    context: Some(context_line(
                        &repo_id,
                        IndexState::Fresh(&freshness),
                        tools_hint(),
                    )),
                });
            }
            CacheDecision::ReanalyzeNeeded { .. } => true,
        },
        None => {
            if super::session::count_source_files(&abs_path) == 0 {
                return Ok(HookReport::status_only(format!(
                    "[myceliums] {name} skipped | no supported files found"
                )));
            }
            false
        }
    };

    let data = data_dir();
    if analysis_running(&data, &repo_id) {
        let (state, what) = if indexed {
            (IndexState::RefreshRunning, "re-index already running")
        } else {
            (IndexState::BuildRunning, "index build already running")
        };
        return Ok(HookReport {
            status: format!("[myceliums] {name} {what}"),
            context: Some(context_line(&repo_id, state, tools_hint())),
        });
    }

    let log = data.join("logs").join(format!("session-{repo_id}.log"));
    spawn_reindex(&abs_path, strict_embeddings, no_git_check, &log)?;
    let (state, what) = if indexed {
        (
            IndexState::Refreshing,
            "stale, re-indexing in the background",
        )
    } else {
        (
            IndexState::Building,
            "no index yet, building in the background",
        )
    };
    Ok(HookReport {
        status: format!("[myceliums] {name} {what} (log: {})", log.display()),
        context: Some(context_line(&repo_id, state, tools_hint())),
    })
}

/// Whether a live process holds the #98 analysis lock of `repo_id`.
fn analysis_running(data: &Path, repo_id: &str) -> bool {
    std::fs::read_to_string(lock::lock_path(data, repo_id))
        .ok()
        .and_then(|content| lock::parse_lock_pid(&content))
        .is_some_and(lock::is_process_alive)
}

/// Starts `myc analyze` for `abs_path` detached from the hook.
fn spawn_reindex(
    abs_path: &Path,
    strict_embeddings: bool,
    no_git_check: bool,
    log: &Path,
) -> Result<u32> {
    let program = match std::env::var_os(REINDEX_EXE_ENV) {
        Some(exe) => PathBuf::from(exe),
        None => std::env::current_exe().context("cannot locate the myc binary")?,
    };
    let mut args: Vec<OsString> = vec![
        "analyze".into(),
        abs_path.into(),
        "--force".into(),
        "--max-age".into(),
        "60".into(),
        // Session startup skips embeddings for speed unless the caller demands
        // a verified, fully-embedded index (EmbeddingPolicy::for_session).
        if strict_embeddings {
            "--strict-embeddings".into()
        } else {
            "--skip-embeddings".into()
        },
    ];
    if no_git_check {
        args.push("--no-git-check".into());
    }
    spawn_detached(&program, &args, log)
}

/// Spawns `program` in its own session, stdin closed, stdout and stderr
/// appended to `log`, and does not wait for it: when the hook exits, init
/// adopts and reaps the child. Returns its PID.
pub(crate) fn spawn_detached(program: &Path, args: &[OsString], log: &Path) -> Result<u32> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    if std::fs::metadata(log).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        let _ = std::fs::remove_file(log);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("cannot open {}", log.display()))?;
    let _ = writeln!(
        file,
        "--- {} myc session: {} {}",
        chrono::Utc::now().to_rfc3339(),
        program.display(),
        args.iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let err = file.try_clone()?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid is async-signal-safe and touches no memory of the
        // parent; it only moves the child into a new session and process
        // group, away from the hook's terminal and process group.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", program.display()))?;
    // Dropping a std Child neither waits nor kills.
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_carries_context_for_the_model_and_status_for_the_user() {
        let report = HookReport {
            status: "[myceliums] p ready | 3 files \u{00b7} 9 symbols".into(),
            context: Some("ctx".into()),
        };
        let v = hook_json(&report);
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "ctx");
        assert_eq!(v["systemMessage"], report.status);
        assert_eq!(v.as_object().unwrap().len(), 2);

        let v = hook_json(&HookReport::status_only("[myceliums] p skipped".into()));
        assert!(v.get("hookSpecificOutput").is_none());
        assert_eq!(v["systemMessage"], "[myceliums] p skipped");
    }

    #[test]
    fn context_lines_are_one_budgeted_line_in_every_state() {
        let behind = IndexFreshness::compare(
            "2026-11-20T10:00:00Z",
            Some("0123456789abcdef0123456789abcdef01234567"),
            Some("fedcba9876543210fedcba9876543210fedcba98"),
        );
        let current = IndexFreshness::compare("t", Some("abcdef0123"), Some("abcdef0123"));
        let unknown = IndexFreshness::compare("2026-11-20T10:00:00Z", None, None);
        let repo = "a-rather-long-repository-name-1a2b3c4d";
        let states = [
            IndexState::Fresh(&behind),
            IndexState::Fresh(&current),
            IndexState::Fresh(&unknown),
            IndexState::Refreshing,
            IndexState::RefreshRunning,
            IndexState::Building,
            IndexState::BuildRunning,
        ];
        for state in states {
            let line = context_line(repo, state, true);
            assert!(!line.contains('\n'), "{line}");
            assert!(
                line.len() <= CONTEXT_BUDGET_CHARS,
                "{} chars: {line}",
                line.len()
            );
            assert!(line.contains(repo), "{line}");
            assert!(!line.ends_with('\u{2026}'), "truncated: {line}");
            for tool in ["find_callers", "impact_of_change", "search_code"] {
                assert!(line.contains(tool), "{line}");
            }
        }
        let line = |s| context_line(repo, s, true);
        assert!(line(IndexState::Fresh(&current)).contains("abcdef0 = HEAD"));
        assert!(line(IndexState::Fresh(&behind)).contains("may be stale"));
        assert!(line(IndexState::Refreshing).contains("re-indexing in the background"));
        assert!(line(IndexState::RefreshRunning).contains("already running"));
        assert!(line(IndexState::Building).contains("building in the background"));
        assert!(line(IndexState::BuildRunning).contains("already running"));
        assert!(!context_line(repo, IndexState::Refreshing, false).contains("find_callers"));
        let huge = "x".repeat(400);
        assert_eq!(
            context_line(&huge, IndexState::Fresh(&current), true).len(),
            CONTEXT_BUDGET_CHARS
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_detached_child_runs_in_its_own_session_and_logs_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs").join("x.log");
        let started = std::time::Instant::now();
        let pid = spawn_detached(
            Path::new("/bin/sh"),
            &["-c".into(), "echo out; echo err >&2; sleep 2".into()],
            &log,
        )
        .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // SAFETY: getsid only reads the session id of a process.
        let sid = unsafe { libc::getsid(pid as libc::pid_t) };
        assert_eq!(sid, pid as libc::pid_t, "child leads its own session");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let text = std::fs::read_to_string(&log).unwrap();
            if text.contains("out\n") && text.contains("err\n") {
                assert!(text.starts_with("--- "), "{text}");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{text}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // This test process is the parent here (not init), so reap it.
        // SAFETY: waitpid on our own child.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
            libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0);
        }
    }
}
