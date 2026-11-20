//! `myc session --yes`: the Claude Code SessionStart hook (#152).
//!
//! Output contract, per the Claude Code hooks reference
//! <https://code.claude.com/docs/en/hooks#sessionstart> (checked 2026-11-20):
//!
//! * stdout carries exactly one JSON object and nothing else. Claude Code adds
//!   `hookSpecificOutput.additionalContext` (with `hookEventName:
//!   "SessionStart"`) to the model's context before the first prompt, and shows
//!   `systemMessage` to the user only. Plain-text stdout would also reach the
//!   model, so the analysis output that a re-index prints must never land on
//!   stdout: the analysis runs as a child process whose stdout goes to stderr.
//! * The context line is paid for in every session, so it is one line of at
//!   most [`CONTEXT_BUDGET_CHARS`] characters (about 60 tokens): repo id, index
//!   freshness from [`myceliums_core::navigation::index_freshness`], and the
//!   core tools for structural questions. The docs ask for facts, not
//!   imperative "system" instructions, which can trip prompt-injection
//!   defences.
//! * The human status line goes to `systemMessage` (the user sees it in the
//!   transcript) and to stderr (for a run by hand; the installed hook sends
//!   stderr to /dev/null, and Claude never sees stderr from a hook that exits 0).
//! * It never fails the session: every error becomes a status line, no context
//!   is added on an error, a skip or a timeout, and the process exits 0.
//!
//! Runtime: Claude Code waits for a synchronous hook before the session
//! starts. With a current index the hook reads the registry, runs the cache
//! check and one `git rev-parse HEAD` (bounded to 1.5 s by the core). With a
//! missing or stale index it re-analyzes (embeddings skipped unless
//! `--strict-embeddings`), which blocks session start for at most `--timeout`
//! seconds (300 in the installed hook); on timeout the child is killed and the
//! session starts without fresh context.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result};
use myceliums_core::analyzer;
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_core::navigation::{index_freshness, IndexFreshness};
use myceliums_storage::{RepoInfo, RepoRegistry};
use serde_json::{json, Value};

use crate::commands::analyze::{check_session_safeguards, enforce_strict_embeddings};
use crate::repo::{data_dir, registry_path};

/// Upper bound for the model-facing context line (~60 tokens).
pub(crate) const CONTEXT_BUDGET_CHARS: usize = 280;

/// What one hook run reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookReport {
    /// One line for the user (`systemMessage`, stderr).
    pub status: String,
    /// One line for the model (`additionalContext`); `None` on errors, skips
    /// and timeouts.
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

/// The model-facing line for an indexed repository. `tools_hint` adds the
/// core tools (off when AI instructions were declined in the setup wizard).
pub(crate) fn context_line(repo_id: &str, freshness: &IndexFreshness, tools_hint: bool) -> String {
    let mut line = format!(
        "myceliums code graph of repo {repo_id} ({}).",
        freshness.summary()
    );
    if tools_hint {
        line.push_str(
            " MCP tools find_callers, impact_of_change and search_code answer structural \
             questions (callers, blast radius, where code lives).",
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

fn context_for(info: &RepoInfo) -> String {
    // The tools sentence is left out only when the user said no to AI
    // instructions in the setup wizard; `myc setup-claude` alone leaves
    // `instructions_enabled` at its default (false) without asking.
    let setup = myceliums_core::GlobalConfig::load(&data_dir())
        .unwrap_or_default()
        .setup;
    let tools_hint = setup.instructions_enabled || !setup.completed;
    context_line(&info.id, &index_freshness(info), tools_hint)
}

/// Runs the hook. Never returns an error: failures become the status line.
pub(crate) async fn run(
    path: &Path,
    timeout_secs: u64,
    strict_embeddings: bool,
    no_git_check: bool,
) -> HookReport {
    let name = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());
    match run_inner(path, &name, timeout_secs, strict_embeddings, no_git_check).await {
        Ok(report) => report,
        Err(e) => HookReport::status_only(format!("[myceliums] {name} error | {e:#}")),
    }
}

async fn run_inner(
    path: &Path,
    name: &str,
    timeout_secs: u64,
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
    let verb = match registry.get(&repo_id) {
        Some(info) => match cache::check_cache(info, &abs_path, &CacheCheckConfig::default()) {
            CacheDecision::UseCached { .. } => {
                // A fresh cache must not launder a partial index into a
                // "ready" report when strict coverage was demanded.
                if strict_embeddings {
                    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                    enforce_strict_embeddings(&db_path, &repo_id).await?;
                }
                return Ok(ready(info, name, "ready"));
            }
            CacheDecision::ReanalyzeNeeded { .. } => "updated",
        },
        None => {
            if super::session::count_source_files(&abs_path) == 0 {
                return Ok(HookReport::status_only(format!(
                    "[myceliums] {name} skipped | no supported files found"
                )));
            }
            "analyzed"
        }
    };

    let exe = std::env::current_exe().context("cannot locate the myc binary")?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("analyze")
        .arg(&abs_path)
        .args(["--force", "--max-age", "60"]);
    // Session startup skips embeddings for speed unless the caller demands a
    // verified, fully-embedded index (EmbeddingPolicy::for_session).
    cmd.arg(if strict_embeddings {
        "--strict-embeddings"
    } else {
        "--skip-embeddings"
    });
    if no_git_check {
        cmd.arg("--no-git-check");
    }
    let limit = (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs));
    match run_bounded(cmd, limit).await? {
        ChildOutcome::Finished(true) => {}
        ChildOutcome::Finished(false) => {
            return Ok(HookReport::status_only(format!(
                "[myceliums] {name} error | analysis failed (run `myc analyze .` to see why)"
            )))
        }
        ChildOutcome::TimedOut => {
            return Ok(HookReport::status_only(format!(
                "[myceliums] {name} timed out after {timeout_secs}s"
            )))
        }
    }

    let registry = RepoRegistry::load(&registry_path())?;
    match registry.get(&repo_id) {
        Some(info) => Ok(ready(info, name, verb)),
        None => Ok(HookReport::status_only(format!(
            "[myceliums] {name} error | analysis left no index"
        ))),
    }
}

fn ready(info: &RepoInfo, name: &str, verb: &str) -> HookReport {
    HookReport {
        status: format!(
            "[myceliums] {name} {verb} | {} files \u{00b7} {} symbols",
            info.file_count, info.symbol_count
        ),
        context: Some(context_for(info)),
    }
}

/// How a bounded child process ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChildOutcome {
    /// Exited on its own; `true` on success.
    Finished(bool),
    /// Killed after the time limit.
    TimedOut,
}

/// Runs `cmd` with stdin closed and its stdout sent to our stderr (stdout is
/// reserved for the hook's JSON), killing it after `limit`.
pub(crate) async fn run_bounded(
    mut cmd: tokio::process::Command,
    limit: Option<Duration>,
) -> Result<ChildOutcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("cannot start the analysis")?;
    let status = match limit {
        Some(limit) => match tokio::time::timeout(limit, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                let _ = child.kill().await;
                return Ok(ChildOutcome::TimedOut);
            }
        },
        None => child.wait().await?,
    };
    Ok(ChildOutcome::Finished(status.success()))
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
    fn context_line_is_one_budgeted_line() {
        let behind = IndexFreshness::compare(
            "2026-11-20T10:00:00Z",
            Some("0123456789abcdef0123456789abcdef01234567"),
            Some("fedcba9876543210fedcba9876543210fedcba98"),
        );
        let current = IndexFreshness::compare("t", Some("abcdef0123"), Some("abcdef0123"));
        let unknown = IndexFreshness::compare("2026-11-20T10:00:00Z", None, None);
        let repo = "a-rather-long-repository-name-1a2b3c4d";
        for f in [&behind, &current, &unknown] {
            let line = context_line(repo, f, true);
            assert!(!line.contains('\n'), "{line}");
            assert!(
                line.len() <= CONTEXT_BUDGET_CHARS,
                "{} chars: {line}",
                line.len()
            );
            assert!(line.contains(repo), "{line}");
            for tool in ["find_callers", "impact_of_change", "search_code"] {
                assert!(line.contains(tool), "{line}");
            }
        }
        assert!(context_line(repo, &current, true).contains("abcdef0 = HEAD"));
        assert!(context_line(repo, &behind, true).contains("may be stale"));
        assert!(!context_line(repo, &current, false).contains("find_callers"));
        let huge = "x".repeat(400);
        assert_eq!(
            context_line(&huge, &current, true).len(),
            CONTEXT_BUDGET_CHARS
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_past_its_limit_is_killed() {
        let started = std::time::Instant::now();
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30");
        let outcome = run_bounded(cmd, Some(Duration::from_millis(200)))
            .await
            .unwrap();
        assert_eq!(outcome, ChildOutcome::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));

        let mut ok = tokio::process::Command::new("true");
        ok.arg("x");
        assert_eq!(
            run_bounded(ok, None).await.unwrap(),
            ChildOutcome::Finished(true)
        );
        let fail = tokio::process::Command::new("false");
        assert_eq!(
            run_bounded(fail, Some(Duration::from_secs(10)))
                .await
                .unwrap(),
            ChildOutcome::Finished(false)
        );
    }
}
