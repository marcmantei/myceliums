//! Instruction files that a file written by `myc` would hide (#200).
//!
//! Agents do not simply read every instruction file: some kinds replace
//! others.
//!
//! * **Claude Code** reads `AGENTS.md` (and `.claude/AGENTS.md`, in the
//!   working directory, above it, and in subdirectories on demand) only while
//!   no `CLAUDE.md`, `.claude/CLAUDE.md` or `CLAUDE.local.md` is in the
//!   working directory or above it. `.claude/rules/*.md` and
//!   `~/.claude/CLAUDE.md` do not count
//!   (<https://code.claude.com/docs/en/memory#agents-md>, checked 2026-11-24
//!   and against Claude Code 2.1.331 with `evals/agent/probe_instructions.py`).
//!   An `@AGENTS.md` import brings back the one file it names, not those in
//!   subdirectories or above.
//! * **Codex** reads one file per directory: `AGENTS.override.md`, else
//!   `AGENTS.md`, else a name from `project_doc_fallback_filenames`
//!   (<https://developers.openai.com/codex/guides/agents-md>).
//! * **Zed** reads the first match of a list in which `AGENTS.md` comes before
//!   `CLAUDE.md` and `GEMINI.md` (<https://zed.dev/docs/ai/instructions>);
//!   **OpenCode** reads a directory's `CLAUDE.md` only when it has no
//!   `AGENTS.md` (<https://opencode.ai/docs/rules/>).
//!
//! So `myc setup-claude` keeps its routing block in
//! `.claude/rules/myceliums.md`, and `myc setup-codex` does not create an
//! `AGENTS.md` beside an instruction file it would hide. Older versions did
//! both; [`findings`] detects what they left, and [`repair`] (a repeated
//! setup, or `myc doctor --fix`) undoes it. The other editors' setups write
//! MCP configuration only, no instruction file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use super::routing::{self, FileChange, Owner};
use super::setup_claude::{self, Targets};

/// Files of which one makes Claude Code skip AGENTS.md files.
const CLAUDE_FILES: &[&str] = &["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The files Claude Code then skips, per directory.
const AGENTS_FILES: &[&str] = &["AGENTS.md", ".claude/AGENTS.md"];
/// Files that Zed and OpenCode (`CLAUDE.md`) or Zed alone (`GEMINI.md`) do not
/// read beside an AGENTS.md.
const BEHIND_AGENTS_MD: &[&str] = &["CLAUDE.md", "GEMINI.md"];

/// Something an older `myc` left in a project's instruction files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `CLAUDE.md` consists of the routing block and makes Claude Code skip
    /// `hidden`.
    ClaudeHides {
        memory: PathBuf,
        hidden: Vec<PathBuf>,
    },
    /// `CLAUDE.md` holds the routing block where older versions wrote it, but
    /// hides nothing that another file would not hide as well.
    ClaudeOldPlace { memory: PathBuf },
    /// `AGENTS.md` consists of `myc setup-codex`'s routing block and makes
    /// other agents skip `hidden`.
    AgentsHides {
        agents: PathBuf,
        hidden: Vec<PathBuf>,
    },
}

impl Finding {
    /// Whether the project's own instructions are hidden: the case to repair,
    /// as opposed to a remark.
    pub fn hides(&self) -> bool {
        !matches!(self, Finding::ClaudeOldPlace { .. })
    }

    /// One line for `myc doctor`.
    pub fn describe(&self, root: &Path) -> String {
        match self {
            Finding::ClaudeHides { memory, hidden } => format!(
                "{} holds only the myceliums routing block and makes Claude Code skip {} — run 'myc doctor --fix' or 'myc setup-claude'",
                memory.display(),
                names(root, hidden)
            ),
            Finding::ClaudeOldPlace { memory } => format!(
                "{}: the myceliums routing block belongs in .claude/rules/myceliums.md now — 'myc doctor --fix' or 'myc setup-claude' moves it",
                memory.display()
            ),
            Finding::AgentsHides { agents, hidden } => format!(
                "{} holds only the myceliums routing block and makes agents that read one instruction file per directory skip {} — run 'myc doctor --fix' or 'myc setup-codex'",
                agents.display(),
                names(root, hidden)
            ),
        }
    }
}

/// `paths` relative to `root` where possible, the first three by name.
pub fn names(root: &Path, paths: &[PathBuf]) -> String {
    const SHOWN: usize = 3;
    let mut out = paths
        .iter()
        .take(SHOWN)
        .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOWN {
        out.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    out
}

/// What `myc` left in the instruction files of the project at `root`.
/// Files that cannot be read, or whose markers do not match, are no finding:
/// the setup commands report them.
pub fn findings(root: &Path, home: Option<&Path>) -> Vec<Finding> {
    let mut out = Vec::new();
    let memory = root.join("CLAUDE.md");
    if let Some(rest) = without_block(&memory, Owner::Claude) {
        let hidden = if rest.trim().is_empty() && !claude_file_besides(root, home) {
            hidden_from_claude(root)
        } else {
            Vec::new()
        };
        out.push(if hidden.is_empty() {
            Finding::ClaudeOldPlace { memory }
        } else {
            Finding::ClaudeHides { memory, hidden }
        });
    }
    let agents = root.join("AGENTS.md");
    if is_only_block(&agents) {
        let hidden = hidden_by_agents_md(root, home);
        if !hidden.is_empty() {
            out.push(Finding::AgentsHides { agents, hidden });
        }
    }
    out
}

/// Undoes `finding` in the project at `root`; returns what changed, for the
/// report.
pub fn repair(root: &Path, finding: &Finding) -> Result<String> {
    match finding {
        Finding::ClaudeHides { .. } | Finding::ClaudeOldPlace { .. } => {
            let targets = Targets::project(root);
            let subagent = targets.agent.exists();
            setup_claude::install_routing(&targets, subagent)
        }
        Finding::AgentsHides { agents, .. } => {
            routing::uninstall_block(agents, Owner::Any, true)?;
            Ok(format!("removed {}", agents.display()))
        }
    }
}

/// The text of `path` without the blocks of `owner`, when it holds one.
fn without_block(path: &Path, owner: Owner) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    routing::remove_block(&text, owner).ok().flatten()
}

/// Whether `path` holds a routing block and nothing else.
pub fn is_only_block(path: &Path) -> bool {
    without_block(path, Owner::Any).is_some_and(|rest| rest.trim().is_empty())
}

/// Whether Claude Code skips AGENTS.md files for a reason other than
/// `root`'s CLAUDE.md. `~/.claude/CLAUDE.md` is none.
fn claude_file_besides(root: &Path, home: Option<&Path>) -> bool {
    let memory = root.join("CLAUDE.md");
    let user_memory = home.map(|h| h.join(".claude").join("CLAUDE.md"));
    root.ancestors()
        .flat_map(|dir| CLAUDE_FILES.iter().map(move |name| dir.join(name)))
        .any(|p| p != memory && Some(&p) != user_memory.as_ref() && p.is_file())
}

/// The AGENTS.md files Claude Code would read in `root` without a CLAUDE.md:
/// in `root`, above it, and in subdirectories that have no Claude file of
/// their own (listed through git; a project outside git has those of `root`
/// and above only).
fn hidden_from_claude(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|dir| AGENTS_FILES.iter().map(move |name| dir.join(name)))
        .filter(|p| p.is_file())
        .collect();
    let listed = myceliums_core::git_read::git_read(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*/AGENTS.md",
        ],
        Duration::from_secs(5),
    )
    .unwrap_or_default();
    for rel in listed.split('\0').filter(|rel| !rel.is_empty()) {
        let path = root.join(rel);
        let dir = path.parent().map(|dir| match dir.file_name() {
            // `sub/.claude/AGENTS.md` belongs to `sub`.
            Some(name) if name == ".claude" => dir.parent().unwrap_or(dir),
            _ => dir,
        });
        let has_own = dir
            .is_some_and(|dir| dir != root && CLAUDE_FILES.iter().any(|f| dir.join(f).is_file()));
        if !has_own && path.is_file() && !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// The instruction files in `root` that some agent reads only while there is
/// no AGENTS.md beside them: [`BEHIND_AGENTS_MD`] and Codex's configured
/// fallbacks. Empty files, files made of a routing block alone and links to
/// AGENTS.md hold nothing to hide.
pub fn hidden_by_agents_md(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let agents = std::fs::canonicalize(root.join("AGENTS.md")).ok();
    let mut names: Vec<String> = BEHIND_AGENTS_MD.iter().map(|n| n.to_string()).collect();
    for name in codex_fallback_names(home) {
        if !names.contains(&name) && name != "AGENTS.md" {
            names.push(name);
        }
    }
    names
        .iter()
        .map(|name| root.join(name))
        .filter(|path| {
            let Ok(text) = std::fs::read_to_string(path) else {
                return false;
            };
            let same_file =
                agents.is_some() && std::fs::canonicalize(path).ok().as_ref() == agents.as_ref();
            !same_file && !text.trim().is_empty() && !is_only_block(path)
        })
        .collect()
}

/// `project_doc_fallback_filenames` of `~/.codex/config.toml`.
fn codex_fallback_names(home: Option<&Path>) -> Vec<String> {
    home.and_then(|h| std::fs::read_to_string(h.join(".codex").join("config.toml")).ok())
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|config| {
            Some(
                config
                    .get("project_doc_fallback_filenames")?
                    .as_array()?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            )
        })
        .unwrap_or_default()
}

/// What `myc setup-codex` did about the routing block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRouting {
    /// The block is in AGENTS.md (or was removed from it).
    Done(FileChange),
    /// No block: an AGENTS.md of the block alone would hide `hidden`.
    /// `removed`: one that an older version had created is gone.
    WouldHide { hidden: Vec<PathBuf>, removed: bool },
}

/// Writes the routing block into `root`'s AGENTS.md, unless that file would
/// consist of the block alone and hide another instruction file.
pub fn install_codex_routing(root: &Path, home: Option<&Path>) -> Result<CodexRouting> {
    let agents = root.join("AGENTS.md");
    let own_text = match std::fs::read_to_string(&agents) {
        Ok(text) => !routing::remove_block(&text, Owner::Any)
            .ok()
            .flatten()
            .unwrap_or(text)
            .trim()
            .is_empty(),
        Err(_) => false,
    };
    if !own_text {
        let hidden = hidden_by_agents_md(root, home);
        if !hidden.is_empty() {
            let change = routing::uninstall_block(&agents, Owner::Any, true)?;
            return Ok(CodexRouting::WouldHide {
                hidden,
                removed: change != FileChange::Absent,
            });
        }
    }
    let block = routing::routing_block(routing::Flavor::Agents);
    Ok(CodexRouting::Done(routing::install_block(&agents, &block)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::routing::{routing_block, Flavor, BEGIN};

    fn claude_block() -> String {
        routing_block(Flavor::Claude { subagent: true })
    }

    /// A project below a directory of its own, so nothing above it counts.
    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("project");
        std::fs::create_dir_all(&root).unwrap();
        (dir, root)
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_project_without_a_block_has_no_findings() {
        let (_dir, root) = project();
        assert_eq!(findings(&root, None), []);
        write(&root, "AGENTS.md", "rules\n");
        write(&root, "CLAUDE.md", "mine\n");
        assert_eq!(findings(&root, None), []);
    }

    #[test]
    fn a_block_only_claude_md_hides_every_agents_md_around_it() {
        let (_dir, root) = project();
        git(&root, &["init", "-q"]);
        write(&root, "CLAUDE.md", &claude_block());
        // Nothing to hide yet.
        assert_eq!(
            findings(&root, None),
            [Finding::ClaudeOldPlace {
                memory: root.join("CLAUDE.md")
            }]
        );
        write(&root, "AGENTS.md", "rules\n");
        write(&root, ".claude/AGENTS.md", "more rules\n");
        write(&root, "packages/app/AGENTS.md", "app rules\n");
        write(&root, "packages/web/.claude/AGENTS.md", "web rules\n");
        // Read by Claude Code in neither case: its directory has a CLAUDE.md.
        write(&root, "packages/own/AGENTS.md", "x\n");
        write(&root, "packages/own/CLAUDE.md", "x\n");
        write(&root, "../AGENTS.md", "above\n");
        let parent = root.parent().unwrap();
        let found = findings(&root, None);
        let [Finding::ClaudeHides { memory, hidden }] = &found[..] else {
            panic!("{found:?}");
        };
        assert_eq!(memory, &root.join("CLAUDE.md"));
        for expected in [
            root.join("AGENTS.md"),
            root.join(".claude/AGENTS.md"),
            parent.join("AGENTS.md"),
            root.join("packages/app/AGENTS.md"),
            root.join("packages/web/.claude/AGENTS.md"),
        ] {
            assert!(hidden.contains(&expected), "{expected:?} in {hidden:?}");
        }
        assert_eq!(hidden.len(), 5, "{hidden:?}");
        assert!(found[0].hides());
        let line = found[0].describe(&root);
        assert!(line.contains("skip AGENTS.md") && line.contains("and 2 more"));

        // Repairing moves the block and shows the files again.
        let said = repair(&root, &found[0]).unwrap();
        assert!(said.contains("CLAUDE.md"), "{said}");
        assert!(!root.join("CLAUDE.md").exists());
        assert!(
            std::fs::read_to_string(root.join(".claude/rules/myceliums.md"))
                .unwrap()
                .contains(BEGIN)
        );
        assert_eq!(findings(&root, None), []);
    }

    /// Another Claude file hides AGENTS.md anyway, by the project's choice;
    /// the user's own memory file does not.
    #[test]
    fn only_a_claude_md_that_is_the_sole_reason_is_blamed() {
        for other in ["CLAUDE.local.md", ".claude/CLAUDE.md", "../CLAUDE.md"] {
            let (_dir, root) = project();
            write(&root, "CLAUDE.md", &claude_block());
            write(&root, "AGENTS.md", "rules\n");
            assert!(findings(&root, None)[0].hides());
            write(&root, other, "mine\n");
            assert!(!findings(&root, None)[0].hides(), "{other}");
        }
        let (_dir, root) = project();
        let home = root.parent().unwrap().to_path_buf();
        write(&root, "CLAUDE.md", &claude_block());
        write(&root, "AGENTS.md", "rules\n");
        write(&home, ".claude/CLAUDE.md", "personal\n");
        assert!(findings(&root, Some(&home))[0].hides());
    }

    #[test]
    fn a_claude_md_with_own_text_is_a_remark_and_keeps_the_text() {
        let (_dir, root) = project();
        write(&root, "CLAUDE.md", &format!("mine\n\n{}", claude_block()));
        write(&root, "AGENTS.md", "rules\n");
        let found = findings(&root, None);
        assert!(!found[0].hides(), "{found:?}");
        repair(&root, &found[0]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "mine\n"
        );
        assert_eq!(findings(&root, None), []);
    }

    #[test]
    fn unbalanced_markers_are_left_alone() {
        let (_dir, root) = project();
        write(&root, "CLAUDE.md", &format!("{BEGIN}\nno end\n"));
        write(&root, "AGENTS.md", "rules\n");
        assert_eq!(findings(&root, None), []);
    }

    #[test]
    fn setup_codex_writes_into_an_agents_md_and_creates_one_that_hides_nothing() {
        // Neither: created. Only AGENTS.md: the block is appended.
        for (before, after) in [
            (None, FileChange::Created),
            (Some("rules\n"), FileChange::Updated),
        ] {
            let (_dir, root) = project();
            if let Some(text) = before {
                write(&root, "AGENTS.md", text);
            }
            let done = install_codex_routing(&root, None).unwrap();
            assert_eq!(done, CodexRouting::Done(after));
            let again = install_codex_routing(&root, None).unwrap();
            assert_eq!(again, CodexRouting::Done(FileChange::Unchanged));
            let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
            assert!(agents.starts_with(before.unwrap_or("")), "{agents}");
            assert_eq!(findings(&root, None), []);
        }
        // Both: AGENTS.md has text of its own, so agents that skip CLAUDE.md
        // did so before.
        let (_dir, root) = project();
        write(&root, "AGENTS.md", "rules\n");
        write(&root, "CLAUDE.md", "mine\n");
        let done = install_codex_routing(&root, None).unwrap();
        assert_eq!(done, CodexRouting::Done(FileChange::Updated));
    }

    #[test]
    fn setup_codex_creates_no_agents_md_that_hides_another_file() {
        for name in ["CLAUDE.md", "GEMINI.md"] {
            let (_dir, root) = project();
            write(&root, name, "mine\n");
            let would_hide = |removed| CodexRouting::WouldHide {
                hidden: vec![root.join(name)],
                removed,
            };
            assert_eq!(
                install_codex_routing(&root, None).unwrap(),
                would_hide(false)
            );
            assert!(!root.join("AGENTS.md").exists());

            // One that an older version created is found and removed.
            write(&root, "AGENTS.md", &routing_block(Flavor::Agents));
            let found = findings(&root, None);
            assert_eq!(
                found,
                [Finding::AgentsHides {
                    agents: root.join("AGENTS.md"),
                    hidden: vec![root.join(name)],
                }]
            );
            assert_eq!(
                install_codex_routing(&root, None).unwrap(),
                would_hide(true)
            );
            assert!(!root.join("AGENTS.md").exists());
            write(&root, "AGENTS.md", &routing_block(Flavor::Agents));
            repair(&root, &found[0]).unwrap();
            assert!(!root.join("AGENTS.md").exists());
            assert_eq!(std::fs::read_to_string(root.join(name)).unwrap(), "mine\n");
        }
        // A CLAUDE.md of our block alone, or an empty one, is nothing to hide.
        for text in ["", "\n", &claude_block()] {
            let (_dir, root) = project();
            write(&root, "CLAUDE.md", text);
            let done = install_codex_routing(&root, None).unwrap();
            assert_eq!(done, CodexRouting::Done(FileChange::Created));
        }
    }

    #[test]
    fn codex_fallback_file_names_count() {
        let (_dir, root) = project();
        let home = root.parent().unwrap().join("home");
        write(
            &home,
            ".codex/config.toml",
            "project_doc_fallback_filenames = [\"TEAM_GUIDE.md\"]\n",
        );
        write(&root, "TEAM_GUIDE.md", "team rules\n");
        assert!(matches!(
            install_codex_routing(&root, Some(&home)).unwrap(),
            CodexRouting::WouldHide { .. }
        ));
        // Without that setting the file is no instruction file.
        assert_eq!(
            install_codex_routing(&root, None).unwrap(),
            CodexRouting::Done(FileChange::Created)
        );
    }

    #[cfg(unix)]
    #[test]
    fn one_file_under_both_names_hides_nothing() {
        for (file, link) in [("AGENTS.md", "CLAUDE.md"), ("CLAUDE.md", "AGENTS.md")] {
            let (_dir, root) = project();
            write(&root, file, "rules\n");
            std::os::unix::fs::symlink(file, root.join(link)).unwrap();
            let done = install_codex_routing(&root, None).unwrap();
            assert_eq!(done, CodexRouting::Done(FileChange::Updated));
            // Codex's block seen through CLAUDE.md is not setup-claude's.
            assert_eq!(findings(&root, None), []);
        }
    }
}
