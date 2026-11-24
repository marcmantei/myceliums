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
//! both; [`findings`] detects what they left, and a repeated setup or `myc
//! doctor --fix` ([`repair`]) undoes it. The other editors' setups write MCP
//! configuration only, no instruction file.
//!
//! Two names may be one file (a symbolic or hard link), and one file may
//! import another. A block in such a shared file serves every agent that
//! reads the file under either name, whoever wrote it last, so `myc
//! setup-claude` never removes it there, and no file that another instruction
//! file points to ([`pointers_to`]) is ever deleted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};

use super::routing::{self, FileChange, Owner};
use super::setup_claude::{self, Targets};

/// Files of which one makes Claude Code skip AGENTS.md files.
const CLAUDE_FILES: &[&str] = &["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The files Claude Code then skips, per directory.
const AGENTS_FILES: &[&str] = &["AGENTS.md", ".claude/AGENTS.md"];
/// Files that Zed and OpenCode (`CLAUDE.md`) or Zed alone (`GEMINI.md`) do not
/// read beside an AGENTS.md.
const BEHIND_AGENTS_MD: &[&str] = &["CLAUDE.md", "GEMINI.md"];
/// Names under which some agent reads a directory's instructions, besides
/// [`CLAUDE_FILES`] and [`AGENTS_FILES`].
const OTHER_FILES: &[&str] = &[
    "AGENTS.override.md",
    "AGENT.md",
    "GEMINI.md",
    ".github/copilot-instructions.md",
    ".rules",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
];
/// Instruction files in the home directory that may link into a project.
const HOME_FILES: &[&str] = &[".claude/CLAUDE.md", ".codex/AGENTS.md"];

/// Something an older `myc` left in a project's instruction files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `CLAUDE.md` consists of the routing block and makes Claude Code skip
    /// `hidden`. It cannot be deleted while `pointed_by` is not empty.
    ClaudeHides {
        memory: PathBuf,
        hidden: Vec<PathBuf>,
        pointed_by: Vec<PathBuf>,
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

    /// Whether `myc` repairs it. A file that another one points to is the
    /// user's to sort out.
    pub fn is_repairable(&self) -> bool {
        !matches!(self, Finding::ClaudeHides { pointed_by, .. } if !pointed_by.is_empty())
    }

    /// One line for `myc doctor`: what is wrong, without the remedy.
    pub fn describe(&self, root: &Path) -> String {
        match self {
            Finding::ClaudeHides {
                memory,
                hidden,
                pointed_by,
            } => {
                let mut line = format!(
                    "{} holds only the myceliums routing block and makes Claude Code skip {}",
                    memory.display(),
                    names(root, hidden)
                );
                if !pointed_by.is_empty() {
                    line.push_str(&format!(
                        "; it is not deleted because {} points to it: remove that import or link first",
                        names(root, pointed_by)
                    ));
                }
                line
            }
            Finding::ClaudeOldPlace { memory } => format!(
                "{}: the myceliums routing block belongs in .claude/rules/myceliums.md now; 'myc setup-claude' moves it (nothing is hidden)",
                memory.display()
            ),
            Finding::AgentsHides { agents, hidden } => format!(
                "{} holds only the myceliums routing block and makes agents that read one instruction file per directory skip {}",
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
        let pointers = pointers_to(root, &memory, home);
        let link = setup_claude::is_symlink(&memory);
        let hidden = if rest.trim().is_empty() && !link && !claude_file_besides(root, home) {
            hidden_from_claude(root)
        } else {
            Vec::new()
        };
        // One file with an AGENTS.md: the project made CLAUDE.md a name of
        // its instructions, and the block in it stays.
        let is_agents = |p: &PathBuf| p.file_name().is_some_and(|n| n == "AGENTS.md");
        if !pointers.same.iter().any(is_agents) {
            if !hidden.is_empty() {
                out.push(Finding::ClaudeHides {
                    memory,
                    hidden,
                    pointed_by: pointers.all(),
                });
            } else if pointers.same.is_empty() {
                out.push(Finding::ClaudeOldPlace { memory });
            }
        }
    }
    let agents = root.join("AGENTS.md");
    if is_only_block(&agents)
        && !setup_claude::is_symlink(&agents)
        && pointers_to(root, &agents, home).all().is_empty()
    {
        let hidden = hidden_by_agents_md(root, home);
        if !hidden.is_empty() {
            out.push(Finding::AgentsHides { agents, hidden });
        }
    }
    out
}

/// Undoes `finding` in the project at `root`; returns what changed, for the
/// report. Fails, with nothing changed, when a block was edited, a file is
/// read-only, or another instruction file points to the one to delete.
pub fn repair(root: &Path, finding: &Finding, home: Option<&Path>) -> Result<String> {
    if !finding.is_repairable() {
        bail!("{}", finding.describe(root));
    }
    match finding {
        Finding::ClaudeHides { .. } | Finding::ClaudeOldPlace { .. } => {
            let mut targets = Targets::project(root);
            targets.home = home.map(Path::to_path_buf);
            setup_claude::check_routing(&targets)?;
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

/// Whether `path` holds a routing block, whoever wrote it.
pub fn has_block(path: &Path) -> bool {
    without_block(path, Owner::Any).is_some()
}

/// Whether `path` holds a routing block, as we wrote it, and nothing else.
fn is_only_block(path: &Path) -> bool {
    without_block(path, Owner::Any).is_some_and(|rest| rest.trim().is_empty())
        && routing::check_unedited(path, Owner::Any).is_ok()
}

/// The instruction files that point to a file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pointers {
    /// The same file under another name (symbolic or hard link).
    pub same: Vec<PathBuf>,
    /// Files that import it (`@CLAUDE.md`).
    pub imports: Vec<PathBuf>,
}

impl Pointers {
    /// Both kinds.
    pub fn all(&self) -> Vec<PathBuf> {
        [self.same.clone(), self.imports.clone()].concat()
    }
}

/// The instruction files in and around the project at `root` that point to
/// `path`: in `root` and above it, in subdirectories (through git) and in the
/// home directory.
pub fn pointers_to(root: &Path, path: &Path, home: Option<&Path>) -> Pointers {
    let names = || {
        [CLAUDE_FILES, AGENTS_FILES, OTHER_FILES]
            .into_iter()
            .flatten()
    };
    let mut candidates: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|dir| names().map(move |name| dir.join(name)))
        .collect();
    candidates.extend(
        codex_fallback_names(home)
            .iter()
            .map(|name| root.join(name)),
    );
    if let Some(home) = home {
        candidates.extend(HOME_FILES.iter().map(|name| home.join(name)));
    }
    candidates.extend(listed(root, &["*/CLAUDE.md", "*/AGENTS.md", "*/GEMINI.md"]));
    candidates.extend(listed(root, &[".claude/rules/*"]));
    let mut out = Pointers::default();
    for candidate in candidates {
        if candidate == path || out.same.contains(&candidate) || out.imports.contains(&candidate) {
            continue;
        }
        if same_file::is_same_file(&candidate, path).unwrap_or(false) {
            out.same.push(candidate);
        } else if imports(&candidate, path) {
            out.imports.push(candidate);
        }
    }
    out
}

/// Whether `file` imports `path` (`@relative/path`). A mention in a code span
/// counts too: this only ever keeps a file from being deleted.
fn imports(file: &Path, path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    text.split_whitespace()
        .filter_map(|word| word.split_once('@'))
        .map(|(_, target)| target.trim_end_matches(|c: char| "`.,;:)\"'".contains(c)))
        .filter(|target| !target.is_empty())
        .any(|target| same_file::is_same_file(dir.join(target), path).unwrap_or(false))
}

/// The files of `root` that git knows or would add and that match
/// `pathspecs`; none outside git.
fn listed(root: &Path, pathspecs: &[&str]) -> Vec<PathBuf> {
    let mut args = vec![
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ];
    args.extend(pathspecs);
    myceliums_core::git_read::git_read(root, &args, Duration::from_secs(5))
        .unwrap_or_default()
        .split('\0')
        .filter(|rel| !rel.is_empty())
        .map(|rel| root.join(rel))
        .collect()
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
/// their own (a project outside git has those of `root` and above only).
/// Other names of the CLAUDE.md itself are not hidden.
fn hidden_from_claude(root: &Path) -> Vec<PathBuf> {
    let memory = root.join("CLAUDE.md");
    let mut out: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|dir| AGENTS_FILES.iter().map(move |name| dir.join(name)))
        .filter(|p| p.is_file())
        .collect();
    for path in listed(root, &["*/AGENTS.md"]) {
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
    out.retain(|p| !same_file::is_same_file(p, &memory).unwrap_or(false));
    out
}

/// The instruction files in `root` that some agent reads only while there is
/// no AGENTS.md beside them: [`BEHIND_AGENTS_MD`] and Codex's configured
/// fallbacks. Empty files, files made of a routing block alone and other
/// names of AGENTS.md hold nothing to hide.
pub fn hidden_by_agents_md(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let agents = root.join("AGENTS.md");
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
            std::fs::read_to_string(path).is_ok_and(|text| !text.trim().is_empty())
                && !same_file::is_same_file(path, &agents).unwrap_or(false)
                && !is_only_block(path)
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
    /// The block is in AGENTS.md.
    Done(FileChange),
    /// No block: an AGENTS.md of the block alone would hide `hidden`.
    /// `removed`: one that an older version had created is gone.
    WouldHide { hidden: Vec<PathBuf>, removed: bool },
}

/// Writes the routing block into `root`'s AGENTS.md, unless that file would
/// consist of the block alone and hide another instruction file. An AGENTS.md
/// that is a link, or that another file points to, is one the project set up.
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
    let set_up =
        setup_claude::is_symlink(&agents) || !pointers_to(root, &agents, home).all().is_empty();
    if !own_text && !set_up {
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

/// Removes the routing block from `root`'s AGENTS.md, and the file when
/// nothing is left and no other instruction file points to it.
pub fn uninstall_codex_routing(root: &Path, home: Option<&Path>) -> Result<FileChange> {
    let agents = root.join("AGENTS.md");
    let delete_if_empty = pointers_to(root, &agents, home).all().is_empty();
    routing::uninstall_block(&agents, Owner::Any, delete_if_empty)
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
        let [Finding::ClaudeHides {
            memory,
            hidden,
            pointed_by,
        }] = &found[..]
        else {
            panic!("{found:?}");
        };
        assert_eq!(memory, &root.join("CLAUDE.md"));
        assert!(pointed_by.is_empty(), "{pointed_by:?}");
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
        let said = repair(&root, &found[0], None).unwrap();
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
        repair(&root, &found[0], None).unwrap();
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
            repair(&root, &found[0], None).unwrap();
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

    /// Makes `link` another name of `file`, both relative to `root`.
    #[cfg(unix)]
    fn link(root: &Path, kind: &str, file: &str, link: &str) {
        let path = root.join(link);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match kind {
            "hard" => std::fs::hard_link(root.join(file), path).unwrap(),
            _ => std::os::unix::fs::symlink(root.join(file), path).unwrap(),
        }
    }

    /// `CLAUDE.md -> AGENTS.md` and the reverse, as symbolic and hard links,
    /// with the block of either setup (or of `setup-claude` over
    /// `setup-codex`, which relabels it), with and without text of their own.
    #[cfg(unix)]
    #[test]
    fn a_file_under_both_names_is_never_deleted_and_keeps_the_block() {
        for kind in ["symbolic", "hard"] {
            for (file, other) in [("AGENTS.md", "CLAUDE.md"), ("CLAUDE.md", "AGENTS.md")] {
                for block in [claude_block(), routing_block(Flavor::Agents)] {
                    for own in ["", "rules\n\n"] {
                        let case = format!("{kind} {file} {other} {own:?}");
                        let (_dir, root) = project();
                        git(&root, &["init", "-q"]);
                        let text = format!("{own}{block}");
                        write(&root, file, &text);
                        link(&root, kind, file, other);
                        // Hidden by nothing of ours: CLAUDE.md is the
                        // project's AGENTS.md.
                        write(&root, "packages/app/AGENTS.md", "app rules\n");
                        assert_eq!(findings(&root, None), [], "{case}");

                        let targets = Targets {
                            home: None,
                            ..Targets::project(&root)
                        };
                        setup_claude::check_routing(&targets).unwrap();
                        let said = setup_claude::install_routing(&targets, true).unwrap();
                        assert!(
                            said.contains("CLAUDE.md") && said.contains("AGENTS.md"),
                            "{case}: {said}"
                        );
                        for name in [file, other] {
                            let now = std::fs::read_to_string(root.join(name)).unwrap();
                            assert_eq!(now, text, "{case}");
                        }
                        // Claude Code reads that block; a second would repeat it.
                        assert!(!targets.memory.exists(), "{case}");

                        // Codex refreshes the block in place...
                        let done = install_codex_routing(&root, None).unwrap();
                        assert!(matches!(done, CodexRouting::Done(_)), "{case}: {done:?}");
                        let now = std::fs::read_to_string(root.join(other)).unwrap();
                        assert_eq!(now, format!("{own}{}", routing_block(Flavor::Agents)));
                        // ... and removes it, but neither name of the file.
                        uninstall_codex_routing(&root, None).unwrap();
                        for name in [file, other] {
                            let now = std::fs::read_to_string(root.join(name)).unwrap();
                            assert_eq!(now, own.replace("\n\n", "\n"), "{case}");
                        }
                        // Then Claude Code gets the block from the rules file.
                        setup_claude::install_routing(&targets, true).unwrap();
                        assert!(targets.memory.exists(), "{case}");
                        assert_eq!(findings(&root, None), [], "{case}");
                    }
                }
            }
        }
    }

    /// A CLAUDE.md of the block alone that another file points to hides
    /// AGENTS.md, but is not ours to delete.
    #[cfg(unix)]
    #[test]
    fn a_claude_md_another_file_points_to_is_reported_not_deleted() {
        for (pointer, imports) in [
            ("packages/app/CLAUDE.md", false),
            ("GEMINI.md", false),
            ("GEMINI.md", true),
            ("AGENTS.md", true),
            (".claude/rules/mine.md", true),
        ] {
            let (_dir, root) = project();
            git(&root, &["init", "-q"]);
            write(&root, "CLAUDE.md", &claude_block());
            write(&root, "AGENTS.md", "rules\n");
            if imports {
                let up = "../".repeat(pointer.matches('/').count());
                write(&root, pointer, &format!("rules\nSee @{up}CLAUDE.md.\n"));
            } else {
                link(&root, "symbolic", "CLAUDE.md", pointer);
            }
            let found = findings(&root, None);
            let [finding @ Finding::ClaudeHides { pointed_by, .. }] = &found[..] else {
                panic!("{pointer}: {found:?}");
            };
            assert_eq!(pointed_by, &[root.join(pointer)], "{pointer}");
            assert!(!finding.is_repairable());
            assert!(finding.describe(&root).contains(pointer));
            assert!(repair(&root, finding, None).is_err(), "{pointer}");

            let targets = Targets {
                home: None,
                ..Targets::project(&root)
            };
            let said = setup_claude::install_routing(&targets, true).unwrap();
            assert!(said.contains(pointer), "{said}");
            let text = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
            assert_eq!(text, claude_block(), "{pointer}");
            assert!(!targets.memory.exists(), "one block, not two");
        }
        // A link from the home directory counts.
        let (_dir, root) = project();
        let home = root.parent().unwrap().join("home");
        write(&root, "CLAUDE.md", &claude_block());
        write(&root, "AGENTS.md", "rules\n");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(root.join("CLAUDE.md"), home.join(".claude/CLAUDE.md")).unwrap();
        let found = findings(&root, Some(&home));
        assert!(!found[0].is_repairable(), "{found:?}");
    }

    /// A CLAUDE.md that imports the file holding Codex's block: nothing of
    /// ours is in CLAUDE.md, and nothing changes there.
    #[test]
    fn an_imported_block_is_left_to_its_file() {
        let (_dir, root) = project();
        let agents = format!("rules\n\n{}", routing_block(Flavor::Agents));
        write(&root, "AGENTS.md", &agents);
        write(&root, "CLAUDE.md", "@AGENTS.md\n\nmine\n");
        assert_eq!(findings(&root, None), []);
        let targets = Targets {
            home: None,
            ..Targets::project(&root)
        };
        setup_claude::install_routing(&targets, true).unwrap();
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
        assert_eq!(read("AGENTS.md"), agents);
        assert_eq!(read("CLAUDE.md"), "@AGENTS.md\n\nmine\n");
        assert!(targets.memory.exists());

        // An AGENTS.md of the block alone that CLAUDE.md imports stays too.
        write(&root, "AGENTS.md", &routing_block(Flavor::Agents));
        assert_eq!(findings(&root, None), []);
        let done = install_codex_routing(&root, None).unwrap();
        assert_eq!(done, CodexRouting::Done(FileChange::Unchanged));
        uninstall_codex_routing(&root, None).unwrap();
        assert_eq!(read("AGENTS.md"), "");
    }

    #[test]
    fn a_crlf_claude_md_keeps_its_line_endings() {
        let (_dir, root) = project();
        let block = claude_block().replace('\n', "\r\n");
        write(&root, "CLAUDE.md", &format!("mine\r\n\r\n{block}"));
        let found = findings(&root, None);
        assert_eq!(found.len(), 1, "{found:?}");
        repair(&root, &found[0], None).unwrap();
        let text = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert_eq!(text, "mine\r\n");

        // Alone in the file, and hiding an AGENTS.md.
        let (_dir, root) = project();
        write(&root, "CLAUDE.md", &block);
        write(&root, "AGENTS.md", "rules\r\n");
        let found = findings(&root, None);
        assert!(found[0].hides(), "{found:?}");
        repair(&root, &found[0], None).unwrap();
        assert!(!root.join("CLAUDE.md").exists());
        let text = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert_eq!(text, "rules\r\n");
    }

    /// An edited block or a read-only file is reported and left alone, and
    /// nothing else is written first.
    #[test]
    fn an_edited_or_read_only_claude_md_is_not_repaired() {
        let edited = claude_block().replace(
            "grep/rg stay right",
            "Always run the linter.\ngrep/rg stay right",
        );
        for (text, read_only) in [(edited, false), (claude_block(), true)] {
            let (_dir, root) = project();
            write(&root, "CLAUDE.md", &text);
            write(&root, "AGENTS.md", "rules\n");
            let memory = root.join("CLAUDE.md");
            let mut permissions = std::fs::metadata(&memory).unwrap().permissions();
            permissions.set_readonly(read_only);
            std::fs::set_permissions(&memory, permissions).unwrap();
            let found = findings(&root, None);
            assert!(found[0].hides(), "{found:?}");
            let error = format!("{:#}", repair(&root, &found[0], None).unwrap_err());
            assert!(error.contains("Nothing was changed"), "{error}");
            assert_eq!(std::fs::read_to_string(&memory).unwrap(), text);
            assert!(!root.join(".claude").exists());
        }
        // An AGENTS.md whose block was edited is the user's text.
        let (_dir, root) = project();
        let edited = routing_block(Flavor::Agents).replace("grep/rg stay", "grep stays");
        write(&root, "AGENTS.md", &edited);
        write(&root, "CLAUDE.md", "mine\n");
        assert_eq!(findings(&root, None), []);
        assert!(install_codex_routing(&root, None).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            edited
        );
    }
}
