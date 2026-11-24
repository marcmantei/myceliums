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
//! both; [`findings`] detects what they left. The other editors' setups write
//! MCP configuration only, no instruction file.
//!
//! What older versions left is repaired in the plain case only
//! ([`repair_obstacle`]): a regular file of the project with one name, which
//! no other instruction file links to or imports, holding one block exactly
//! as `myc` wrote it. In every other case nothing is changed and the message
//! gives file and what to do ([`by_hand`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};

use super::routing::{self, FileChange, Flavor, Owner};
use super::setup_claude::{self, Targets};

/// Files of which one makes Claude Code skip AGENTS.md files.
const CLAUDE_FILES: &[&str] = &["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The files Claude Code then skips, per directory.
const AGENTS_FILES: &[&str] = &["AGENTS.md", ".claude/AGENTS.md"];
/// Files that Zed and OpenCode (`CLAUDE.md`) or Zed alone (`GEMINI.md`) do not
/// read beside an AGENTS.md.
const BEHIND_AGENTS_MD: &[&str] = &["CLAUDE.md", "GEMINI.md"];
/// Every name under which some agent reads a directory's instructions.
const ALL_FILES: &[&str] = &[
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    ".claude/AGENTS.md",
    "AGENTS.override.md",
    "AGENT.md",
    "GEMINI.md",
    ".github/copilot-instructions.md",
    ".rules",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
];
/// Directories whose files are instructions.
const RULE_DIRS: &[&str] = &[".claude/rules", ".cursor/rules"];
/// Instruction files in the home directory.
const HOME_FILES: &[&str] = &[".claude/CLAUDE.md", ".codex/AGENTS.md"];
/// A project outside git with more entries than this is not searched.
const MAX_WALKED: usize = 20_000;

/// A project, and where else its instruction files may be linked from.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub home: Option<PathBuf>,
    /// The roots of other indexed projects.
    pub others: Vec<PathBuf>,
}

impl Project {
    /// The project at `root`, with this user's home and index registry.
    pub fn new(root: &Path) -> Self {
        let others = myceliums_storage::RepoRegistry::load(&crate::repo::registry_path())
            .map(|r| {
                r.list()
                    .iter()
                    .map(|repo| repo.path.clone().into())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            root: root.to_path_buf(),
            home: dirs::home_dir().map(|h| std::fs::canonicalize(&h).unwrap_or(h)),
            others,
        }
    }

    fn show(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// `paths` relative to the project where possible, the first three by
    /// name.
    pub fn names(&self, paths: &[PathBuf]) -> String {
        const SHOWN: usize = 3;
        let shown: Vec<String> = paths.iter().take(SHOWN).map(|p| self.show(p)).collect();
        let mut out = shown.join(", ");
        if paths.len() > SHOWN {
            out.push_str(&format!(" and {} more", paths.len() - SHOWN));
        }
        out
    }

    /// Whether the project is in a git checkout (whether or not git works).
    fn in_git(&self) -> bool {
        self.root.ancestors().any(|dir| dir.join(".git").exists())
    }

    fn git(&self, args: &[&str]) -> Result<Vec<PathBuf>> {
        let out = myceliums_core::git_read::git_read(&self.root, args, Duration::from_secs(5))?;
        Ok(out
            .split('\0')
            .filter(|rel| !rel.is_empty())
            .map(|rel| self.root.join(rel))
            .collect())
    }

    /// The instruction files in and below the root: through git, else by
    /// walking the directory; an error when neither is possible. In git,
    /// `ignored` adds the ignored ones, but not those in a directory that is
    /// ignored as a whole (`node_modules`), nor in nested repositories.
    fn nested(&self, ignored: bool) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        if self.in_git() {
            let mut specs: Vec<String> = ALL_FILES.iter().map(|n| format!("*{n}")).collect();
            specs.extend(RULE_DIRS.iter().map(|dir| format!("*{dir}/*")));
            let mut queries = vec![vec!["--cached", "--others", "--exclude-standard"]];
            if ignored {
                queries.push(vec![
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                ]);
            }
            for query in queries {
                let mut args = vec!["ls-files", "-z"];
                args.extend(query);
                args.push("--");
                args.extend(specs.iter().map(String::as_str));
                out.extend(self.git(&args)?);
            }
        } else {
            for (i, entry) in walkdir::WalkDir::new(&self.root).into_iter().enumerate() {
                if i >= MAX_WALKED {
                    bail!("the project is not in git and has more than {MAX_WALKED} files");
                }
                out.push(entry?.into_path());
            }
        }
        out.retain(|path| is_instruction_file(path) && !path.is_dir());
        Ok(out)
    }

    /// Whether git ignores `path`; false outside git.
    fn ignores(&self, path: &Path) -> Result<bool> {
        if !self.in_git() {
            return Ok(false);
        }
        let rel = self.show(path);
        let args = [
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
            &rel,
        ];
        Ok(!self.git(&args)?.is_empty())
    }

    /// The other worktrees of the project's repository.
    pub fn worktrees(&self) -> Vec<PathBuf> {
        let args = ["worktree", "list", "--porcelain"];
        myceliums_core::git_read::git_read(&self.root, &args, Duration::from_secs(5))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
            .filter(|path| path != &self.root)
            .collect()
    }
}

fn is_instruction_file(path: &Path) -> bool {
    ALL_FILES.iter().any(|name| path.ends_with(name))
        || path.parent().is_some_and(|dir| {
            RULE_DIRS.iter().any(|rules| dir.ends_with(rules))
                && !path.ends_with(setup_claude::RULES_FILE)
        })
}

// ── What stands in the way ───────────────────────────────────────────

/// Why the instruction file at `path` is not one to write to: it is (behind)
/// a link, outside the project, or [`routing::obstacle`].
pub fn file_obstacle(root: &Path, path: &Path) -> Option<String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(path).unwrap_or_default();
            return Some(format!("it is a symbolic link to {}", target.display()));
        }
        if !meta.is_file() {
            return Some("it is not a regular file".to_string());
        }
    }
    // The directory, or the closest one that exists.
    let dir = path
        .ancestors()
        .skip(1)
        .find_map(|dir| dir.canonicalize().ok());
    match (dir, root.canonicalize()) {
        (Some(dir), Ok(root)) if dir.starts_with(&root) => routing::obstacle(path),
        (Some(dir), Ok(_)) => Some(format!("it lies outside the project, in {}", dir.display())),
        _ => Some("its directory cannot be read".to_string()),
    }
}

/// Why a block in the file at `path` is left to the user: [`file_obstacle`],
/// git ignores the file, or another instruction file links to it or imports
/// it, or that cannot be found out.
pub fn repair_obstacle(project: &Project, path: &Path) -> Option<String> {
    if let Some(reason) = file_obstacle(&project.root, path) {
        return Some(reason);
    }
    match project.ignores(path) {
        Ok(false) => {}
        Ok(true) => return Some("git ignores it".to_string()),
        Err(e) => return Some(format!("git could not be asked about it ({e:#})")),
    }
    match pointers_to(project, path) {
        Err(e) => Some(format!("the files that may link to it are unknown ({e:#})")),
        Ok(pointers) => pointers.describe(project),
    }
}

/// The instruction files that point to a file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Pointers {
    /// Links: the same file under another name.
    links: Vec<PathBuf>,
    /// Files that import it (`@CLAUDE.md`).
    imports: Vec<PathBuf>,
}

impl Pointers {
    fn describe(&self, project: &Project) -> Option<String> {
        if !self.links.is_empty() {
            let names = project.names(&self.links);
            return Some(format!("it is the same file as {names}"));
        }
        (!self.imports.is_empty()).then(|| format!("{} imports it", project.names(&self.imports)))
    }
}

/// The instruction files that point to `path`: in the project, above it, in
/// the home directory, in the repository's other worktrees and in other
/// indexed projects. An error when the project cannot be searched, or is
/// outside git and has another instruction file.
fn pointers_to(project: &Project, path: &Path) -> Result<Pointers> {
    let nested = project.nested(true)?;
    if !project.in_git() {
        if let Some(other) = nested.iter().find(|p| *p != path) {
            bail!(
                "the project is not in git and also has {}",
                project.show(other)
            );
        }
    }
    let mut candidates = nested;
    let dirs = project.root.ancestors().map(Path::to_path_buf);
    for dir in dirs
        .chain(project.worktrees())
        .chain(project.others.clone())
    {
        candidates.extend(ALL_FILES.iter().map(|name| dir.join(name)));
    }
    let home = project.home.as_deref();
    let fallbacks = codex_fallback_names(home);
    candidates.extend(fallbacks.iter().map(|name| project.root.join(name)));
    if let Some(home) = home {
        candidates.extend(HOME_FILES.iter().map(|name| home.join(name)));
    }
    let mut out = Pointers::default();
    for candidate in candidates {
        let known = out.links.contains(&candidate) || out.imports.contains(&candidate);
        if known || same_entry(&candidate, path) {
            continue;
        }
        if same_file(&candidate, path) {
            out.links.push(candidate);
        } else if imports(&candidate, path) {
            out.imports.push(candidate);
        }
    }
    Ok(out)
}

/// Whether both paths are one name of a file, spelled differently.
fn same_entry(a: &Path, b: &Path) -> bool {
    let dir = |path: &Path| path.parent().and_then(|dir| dir.canonicalize().ok());
    a == b || (a.file_name() == b.file_name() && dir(a).is_some() && dir(a) == dir(b))
}

/// Whether both paths lead to one file. Hard links are told by the link count
/// ([`routing::obstacle`]).
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Whether `file` imports `path` (`@relative/path`). A mention in a code span
/// counts too: this only ever keeps a file from being changed.
fn imports(file: &Path, path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    text.split_whitespace()
        .filter_map(|word| word.split_once('@'))
        .map(|(_, target)| target.trim_end_matches(|c: char| "`.,;:)\"'".contains(c)))
        .any(|target| !target.is_empty() && same_file(&dir.join(target), path))
}

/// What to do by hand about the block in the file at `path`, which was left
/// as it is because of `reason`.
pub fn by_hand(path: &Path, reason: &str) -> String {
    let p = path.display();
    let text = std::fs::read_to_string(path).unwrap_or_default();
    match routing::blocks(&text) {
        Ok(blocks) if blocks.is_empty() => {
            format!("{p} was not changed, because {reason}.")
        }
        Ok(blocks) => format!(
            "{p}:{} holds a myceliums routing block. It was not changed, because {reason}. By hand: {}. Delete the file only if nothing is left in it and no link or import names it.",
            look_at(&blocks),
            routing::manual_step(&blocks)
        ),
        Err(e) => format!(
            "{p} holds an incomplete myceliums routing block ({e:#}). It was not changed. By hand: {}.",
            routing::COMPARE_STEP
        ),
    }
}

/// The lines at which `blocks` start, as a hint where to look.
fn look_at(blocks: &[routing::Block]) -> String {
    let lines: Vec<String> = blocks.iter().map(|b| b.line.to_string()).collect();
    lines.join(",")
}

// ── What older versions left ─────────────────────────────────────────

/// Something an older `myc` left in a project's instruction files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The file with the block.
    pub file: PathBuf,
    /// What it hides from some agent; empty: the block is only in an old
    /// place, or `unknown`.
    pub hidden: Vec<PathBuf>,
    /// Why it could not be determined what else it hides.
    pub unknown: Option<String>,
    /// How to undo it.
    pub fix: Fix,
    what: String,
}

/// How to undo a [`Finding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// `myc doctor --fix` in the project.
    Doctor,
    /// `myc doctor --fix` in the directory above that holds the file.
    Above(PathBuf),
    /// By hand, as the text says.
    ByHand(String),
}

impl Finding {
    /// Whether instructions are hidden, or may be.
    pub fn is_issue(&self) -> bool {
        !self.hidden.is_empty() || self.unknown.is_some()
    }

    /// One line for `myc doctor`: what is wrong, without the remedy.
    pub fn describe(&self) -> &str {
        &self.what
    }
}

/// What `myc` left in the instruction files of `project`, and in a
/// `CLAUDE.md` above it that hides them.
pub fn findings(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    let root = &project.root;
    for dir in root.ancestors() {
        let file = dir.join("CLAUDE.md");
        let Some(only_block) = claude_block_alone(&file) else {
            continue;
        };
        let (hidden, unknown) = if only_block && !claude_file_besides(project, &file) {
            hidden_from_claude(project, &file)
        } else {
            (Vec::new(), None)
        };
        let above = if dir == root {
            ""
        } else {
            ", above this project,"
        };
        let mut what = format!(
            "{}{above} holds only the myceliums routing block",
            file.display()
        );
        if hidden.is_empty() && unknown.is_none() {
            if dir != root {
                continue;
            }
            what = format!(
                "{}: the myceliums routing block belongs in .claude/rules/myceliums.md now (nothing is hidden)",
                file.display()
            );
        }
        if !hidden.is_empty() {
            what.push_str(&format!(
                " and makes Claude Code skip {}",
                project.names(&hidden)
            ));
        }
        if let Some(reason) = &unknown {
            what.push_str(&format!(
                "; whether it hides AGENTS.md files in subdirectories could not be determined ({reason})"
            ));
        }
        let fix = if dir == root {
            fix_for(project, &file)
        } else {
            Fix::Above(dir.to_path_buf())
        };
        if fix == Fix::Doctor && hidden.is_empty() && unknown.is_none() {
            what.push_str("; 'myc setup-claude' moves it");
        }
        out.push(Finding {
            file,
            hidden,
            unknown,
            fix,
            what,
        });
    }
    let agents = root.join("AGENTS.md");
    if is_only_block(&agents) {
        let hidden = hidden_by_agents_md(project);
        if !hidden.is_empty() {
            let what = format!(
                "{} holds only the myceliums routing block and makes agents that read one instruction file per directory skip {}",
                agents.display(),
                project.names(&hidden)
            );
            let fix = fix_for(project, &agents);
            out.push(Finding {
                file: agents,
                hidden,
                unknown: None,
                fix,
                what,
            });
        }
    }
    out
}

fn fix_for(project: &Project, file: &Path) -> Fix {
    match repair_obstacle(project, file) {
        None => Fix::Doctor,
        Some(reason) => Fix::ByHand(by_hand(file, &reason)),
    }
}

/// Undoes `finding`, one with [`Fix::Doctor`]; returns what changed.
pub fn repair(project: &Project, finding: &Finding) -> Result<String> {
    if let Fix::ByHand(text) = &finding.fix {
        bail!("{text}");
    }
    if finding.file == project.root.join("CLAUDE.md") {
        let mut targets = Targets::project(&project.root);
        targets.project = Some(project.clone());
        let done = setup_claude::install_routing(&targets, targets.agent.exists())?;
        if let Some(note) = done.note {
            bail!("{}", note.text);
        }
        return Ok(done.line);
    }
    if finding.file != project.root.join("AGENTS.md") {
        bail!("{} is not in this project", finding.file.display());
    }
    routing::uninstall_block(&finding.file, Owner::Any, true)?;
    Ok(format!("removed {}", finding.file.display()))
}

/// Whether `path` holds nothing but a block of `setup-claude`; `None` without
/// one. Markers that do not match count as a block with other text.
fn claude_block_alone(path: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    match routing::remove_block(&text, Owner::Claude) {
        Ok(rest) => rest.map(|rest| rest.trim().is_empty()),
        Err(_) => Some(false),
    }
}

/// Whether `path` holds a routing block and nothing else.
fn is_only_block(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| routing::remove_block(&text, Owner::Any).ok().flatten())
        .is_some_and(|rest| rest.trim().is_empty())
}

/// Whether Claude Code skips the project's AGENTS.md files for a reason other
/// than `memory`. `~/.claude/CLAUDE.md` is none, and neither is another file
/// of the routing block alone, which is a finding of its own.
fn claude_file_besides(project: &Project, memory: &Path) -> bool {
    let user_memory = project
        .home
        .as_ref()
        .map(|h| h.join(".claude").join("CLAUDE.md"));
    project
        .root
        .ancestors()
        .flat_map(|dir| CLAUDE_FILES.iter().map(move |name| dir.join(name)))
        .any(|p| {
            p != memory
                && Some(&p) != user_memory.as_ref()
                && p.is_file()
                && claude_block_alone(&p) != Some(true)
        })
}

/// The AGENTS.md files Claude Code would read in the project without
/// `memory`: in the root, above it, and in subdirectories that have no Claude
/// file of their own; and why the subdirectories could not be searched. None
/// when `memory` is a link, or another name of an
/// AGENTS.md: then the project made its instructions a CLAUDE.md.
fn hidden_from_claude(project: &Project, memory: &Path) -> (Vec<PathBuf>, Option<String>) {
    let root = &project.root;
    if setup_claude::is_symlink(memory) {
        return (Vec::new(), None);
    }
    let mut out: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|dir| AGENTS_FILES.iter().map(move |name| dir.join(name)))
        .filter(|p| p.is_file())
        .collect();
    let (nested, unknown) = match project.nested(false) {
        Ok(nested) => (nested, None),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    };
    for path in nested.into_iter().filter(|p| p.ends_with("AGENTS.md")) {
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
    if out.iter().any(|p| same_file(p, memory)) {
        return (Vec::new(), None);
    }
    (out, unknown)
}

/// The instruction files in the root that some agent reads only while there
/// is no AGENTS.md beside them: [`BEHIND_AGENTS_MD`] and Codex's configured
/// fallbacks. Empty files, files made of a routing block alone and links to
/// AGENTS.md hold nothing to hide.
pub fn hidden_by_agents_md(project: &Project) -> Vec<PathBuf> {
    let agents = project.root.join("AGENTS.md");
    let mut names: Vec<String> = BEHIND_AGENTS_MD.iter().map(|n| n.to_string()).collect();
    for name in codex_fallback_names(project.home.as_deref()) {
        if !names.contains(&name) && name != "AGENTS.md" {
            names.push(name);
        }
    }
    names
        .iter()
        .map(|name| project.root.join(name))
        .filter(|path| {
            std::fs::read_to_string(path).is_ok_and(|text| !text.trim().is_empty())
                && !same_file(path, &agents)
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

/// Every file with routing blocks among the instruction files of `projects`
/// and the user's `~/.claude/CLAUDE.md`, and what removes them: for `myc
/// doctor`, and after an uninstall for what is left. A file is listed once,
/// with all its names. One in `explained`, whose remedy was already printed,
/// refers to that.
pub fn block_list(projects: &[&Project], explained: &[PathBuf]) -> Vec<String> {
    // Each file with the project it was first found in and its names.
    let mut files: Vec<(FileId, &Project, Vec<PathBuf>)> = Vec::new();
    for project in projects {
        let root = &project.root;
        let mut paths: Vec<PathBuf> = ALL_FILES.iter().map(|name| root.join(name)).collect();
        paths.push(rules_file(root));
        paths.extend(project.nested(true).unwrap_or_default());
        paths.extend(user_memory(project));
        for path in paths {
            let Some(id) = file_id(&path) else {
                continue;
            };
            match files.iter_mut().find(|(known, ..)| *known == id) {
                Some((.., names)) if names.iter().any(|name| same_entry(name, &path)) => {}
                Some((.., names)) => names.push(path),
                None => files.push((id, project, vec![path])),
            }
        }
    }
    let explained: Vec<FileId> = explained.iter().filter_map(|p| file_id(p)).collect();
    let mut out = Vec::new();
    for (id, project, names) in files {
        let Ok(text) = std::fs::read_to_string(&names[0]) else {
            continue;
        };
        let name = project.show(&names[0]);
        let shared = match &names[1..] {
            [] => String::new(),
            others => format!("the same file as {}; ", project.names(others)),
        };
        let (line, remedy) = match routing::blocks(&text) {
            Ok(blocks) if blocks.is_empty() => continue,
            Ok(blocks) => {
                let remedy = remedy(project, &names, &blocks);
                (format!(":{}", look_at(&blocks)), remedy)
            }
            Err(e) => {
                let step = routing::COMPARE_STEP;
                (String::new(), format!("incomplete, {e:#}; by hand: {step}"))
            }
        };
        let remedy = if explained.contains(&id) {
            "see above".to_string()
        } else {
            remedy
        };
        out.push(format!("{name}{line} ({shared}{remedy})"));
    }
    out
}

/// What removes `blocks` from the file with `names`.
fn remedy(project: &Project, names: &[PathBuf], blocks: &[routing::Block]) -> String {
    let root = &project.root;
    let file = &names[0];
    let alone = names.len() == 1 && blocks.len() == 1 && blocks[0].shipped;
    if alone && Some(file) == user_memory(project).as_ref() {
        "`myc setup-claude --scope user --uninstall` removes it".to_string()
    } else if alone && *file == rules_file(root) && file_obstacle(root, file).is_none() {
        "`myc setup-claude --uninstall` removes it".to_string()
    } else if alone
        && *file == root.join("CLAUDE.md")
        && blocks[0].owner == Owner::Claude
        && repair_obstacle(project, file).is_none()
    {
        "`myc setup-claude` moves it to .claude/rules/myceliums.md".to_string()
    } else if alone && *file == root.join("AGENTS.md") && codex_obstacle(project, file).is_none() {
        "`myc setup-codex --uninstall` removes it".to_string()
    } else {
        format!("by hand: {}", routing::manual_step(blocks))
    }
}

fn rules_file(root: &Path) -> PathBuf {
    root.join(RULE_DIRS[0]).join(setup_claude::RULES_FILE)
}

fn user_memory(project: &Project) -> Option<PathBuf> {
    Some(project.home.as_ref()?.join(".claude/CLAUDE.md"))
}

/// What tells a file under all its names, links followed.
#[cfg(unix)]
pub type FileId = (u64, u64);
#[cfg(not(unix))]
pub type FileId = PathBuf;

#[cfg(unix)]
pub fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Hard links are not known on this platform.
#[cfg(not(unix))]
pub fn file_id(path: &Path) -> Option<FileId> {
    path.canonicalize().ok()
}

// ── setup-codex ──────────────────────────────────────────────────────

/// What `myc setup-codex` did about the routing block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRouting {
    /// The block is in AGENTS.md, or was removed from it.
    Done(FileChange),
    /// No block: an AGENTS.md of the block alone would hide `hidden`.
    /// `removed`: one that an older version had created is gone.
    WouldHide { hidden: Vec<PathBuf>, removed: bool },
    /// AGENTS.md was left as it is, for the reason in the text.
    Left(String),
}

/// Why `setup-codex` leaves the AGENTS.md at `path` alone: [`file_obstacle`]
/// or another name of the file. An import of it is none: the importing file
/// wants what AGENTS.md says.
fn codex_obstacle(project: &Project, path: &Path) -> Option<String> {
    if let Some(reason) = file_obstacle(&project.root, path) {
        return Some(reason);
    }
    if !path.exists() {
        return None;
    }
    match pointers_to(project, path) {
        Err(e) => Some(format!("the files that may link to it are unknown ({e:#})")),
        Ok(Pointers { links, .. }) if !links.is_empty() => {
            Some(format!("it is the same file as {}", project.names(&links)))
        }
        Ok(_) => None,
    }
}

/// Writes the routing block into the project's AGENTS.md, unless that file
/// would consist of the block alone and hide another instruction file, or is
/// not one to write to.
pub fn install_codex_routing(project: &Project) -> Result<CodexRouting> {
    let agents = project.root.join("AGENTS.md");
    let block = routing::routing_block(Flavor::Agents);
    if let Some(reason) = codex_obstacle(project, &agents) {
        return Ok(CodexRouting::Left(format!(
            "{}\nThe block for Codex, to add by hand if it is missing:\n\n{block}",
            by_hand(&agents, &reason)
        )));
    }
    let own_text = std::fs::read_to_string(&agents)
        .is_ok_and(|text| !text.trim().is_empty() && !is_only_block(&agents));
    let hidden = hidden_by_agents_md(project);
    if !own_text && !hidden.is_empty() {
        if !is_only_block(&agents) {
            let removed = false;
            return Ok(CodexRouting::WouldHide { hidden, removed });
        }
        return match repair_obstacle(project, &agents) {
            Some(reason) => Ok(CodexRouting::Left(by_hand(&agents, &reason))),
            None => {
                routing::uninstall_block(&agents, Owner::Any, true)?;
                let removed = true;
                Ok(CodexRouting::WouldHide { hidden, removed })
            }
        };
    }
    Ok(CodexRouting::Done(routing::install_block(&agents, &block)?))
}

/// Removes the routing block from the project's AGENTS.md, and the file when
/// nothing is left.
pub fn uninstall_codex_routing(project: &Project) -> Result<CodexRouting> {
    let agents = project.root.join("AGENTS.md");
    let has_block = std::fs::read_to_string(&agents)
        .is_ok_and(|text| routing::blocks(&text).map_or(true, |blocks| !blocks.is_empty()));
    if !has_block {
        return Ok(CodexRouting::Done(FileChange::Absent));
    }
    let obstacle = if is_only_block(&agents) {
        // The file would be deleted, which an import of it forbids too.
        codex_obstacle(project, &agents).or_else(|| match pointers_to(project, &agents) {
            Ok(pointers) => pointers.describe(project),
            Err(e) => Some(format!("{e:#}")),
        })
    } else {
        codex_obstacle(project, &agents)
    };
    if let Some(reason) = obstacle {
        return Ok(CodexRouting::Left(by_hand(&agents, &reason)));
    }
    let change = routing::uninstall_block(&agents, Owner::Any, true)?;
    Ok(CodexRouting::Done(change))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::routing::{routing_block, BEGIN, END};

    fn claude_block() -> String {
        routing_block(Flavor::Claude { subagent: true })
    }

    /// A project below a directory of its own, so nothing above it counts,
    /// and a home directory beside it.
    struct Case {
        _dir: tempfile::TempDir,
        p: Project,
    }

    impl Case {
        fn outside_git() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let top = dir.path().canonicalize().unwrap();
            let root = top.join("project");
            std::fs::create_dir_all(&root).unwrap();
            let p = Project {
                root,
                home: Some(top.join("home")),
                others: vec![top.join("other")],
            };
            Self { _dir: dir, p }
        }

        fn new() -> Self {
            let case = Self::outside_git();
            git(&case.p.root, &["init", "-q"]);
            case
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.p.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn read(&self, rel: &str) -> Option<String> {
            std::fs::read_to_string(self.p.root.join(rel)).ok()
        }

        #[cfg(unix)]
        fn link(&self, target: &str, rel: &str) {
            let path = self.p.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(self.p.root.join(target), path).unwrap();
        }

        fn targets(&self) -> Targets {
            Targets {
                project: Some(self.p.clone()),
                ..Targets::project(&self.p.root)
            }
        }

        fn setup_claude(&self) -> setup_claude::Routed {
            setup_claude::install_routing(&self.targets(), true).unwrap()
        }

        /// The only finding.
        fn finding(&self) -> Finding {
            let mut found = findings(&self.p);
            assert_eq!(found.len(), 1, "{found:?}");
            found.remove(0)
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    #[test]
    fn a_project_without_a_block_has_no_findings() {
        let case = Case::new();
        assert_eq!(findings(&case.p), []);
        case.write("AGENTS.md", "rules\n");
        case.write("CLAUDE.md", "mine\n");
        assert_eq!(findings(&case.p), []);
        assert!(block_list(&[&case.p], &[]).is_empty());
    }

    #[test]
    fn a_block_only_claude_md_hides_every_agents_md_around_it() {
        let case = Case::new();
        case.write("CLAUDE.md", &claude_block());
        assert!(case.finding().hidden.is_empty(), "nothing to hide yet");
        for rel in [
            "AGENTS.md",
            ".claude/AGENTS.md",
            "packages/app/AGENTS.md",
            "packages/web/.claude/AGENTS.md",
            "../AGENTS.md",
        ] {
            case.write(rel, "rules\n");
        }
        // Read by Claude Code in neither case: its directory has a CLAUDE.md.
        case.write("packages/own/AGENTS.md", "x\n");
        case.write("packages/own/CLAUDE.md", "x\n");
        let finding = case.finding();
        assert_eq!(finding.hidden.len(), 5, "{finding:?}");
        assert_eq!(finding.fix, Fix::Doctor);
        let what = finding.describe();
        assert!(
            what.contains("skip AGENTS.md") && what.contains("and 2 more"),
            "{what}"
        );

        // Repairing moves the block and shows the files again.
        repair(&case.p, &finding).unwrap();
        assert_eq!(case.read("CLAUDE.md"), None);
        assert!(case
            .read(".claude/rules/myceliums.md")
            .unwrap()
            .contains(BEGIN));
        assert_eq!(findings(&case.p), []);
    }

    /// Another Claude file hides AGENTS.md anyway, by the project's choice;
    /// the user's own memory file does not.
    #[test]
    fn only_a_claude_md_that_is_the_sole_reason_is_blamed() {
        for other in ["CLAUDE.local.md", ".claude/CLAUDE.md", "../CLAUDE.md"] {
            let case = Case::new();
            case.write("CLAUDE.md", &claude_block());
            case.write("AGENTS.md", "rules\n");
            assert!(!case.finding().hidden.is_empty());
            case.write(other, "mine\n");
            assert!(case.finding().hidden.is_empty(), "{other}");
        }
        let mut case = Case::new();
        let home = case.p.root.parent().unwrap().to_path_buf();
        case.write("CLAUDE.md", &claude_block());
        case.write("AGENTS.md", "rules\n");
        case.write("../.claude/CLAUDE.md", "personal\n");
        case.p.home = Some(home);
        assert!(!case.finding().hidden.is_empty());
    }

    /// A worktree inside a project, both with the block of an older version:
    /// each hides the AGENTS.md from a session in the worktree, and each is
    /// reported.
    #[test]
    fn a_claude_md_above_the_project_is_reported_too() {
        let case = Case::new();
        case.write("CLAUDE.md", &claude_block());
        case.write("../CLAUDE.md", &claude_block());
        case.write("AGENTS.md", "rules\n");
        let found = findings(&case.p);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].fix, Fix::Doctor);
        let above = case.p.root.parent().unwrap().to_path_buf();
        assert_eq!(found[1].fix, Fix::Above(above));
        assert!(found.iter().all(|f| !f.hidden.is_empty()), "{found:?}");
        repair(&case.p, &found[0]).unwrap();
        assert_eq!(case.finding(), found[1]);
        assert!(repair(&case.p, &found[1]).is_err(), "not from here");
        assert!(case.read("../CLAUDE.md").is_some());
    }

    #[test]
    fn a_claude_md_with_own_text_keeps_it() {
        for nl in ["\n", "\r\n"] {
            let case = Case::new();
            let text = format!("mine\n\n{}", claude_block()).replace('\n', nl);
            case.write("CLAUDE.md", &text);
            case.write("AGENTS.md", "rules\n");
            assert!(case.finding().hidden.is_empty());
            assert!(case.setup_claude().note.is_none());
            assert_eq!(case.read("CLAUDE.md"), Some(format!("mine{nl}")));
            assert_eq!(findings(&case.p), []);
        }
    }

    /// Outside git, the only instruction file of a project is plain too.
    #[test]
    fn outside_git_only_a_lone_claude_md_is_plain() {
        let case = Case::outside_git();
        case.write("CLAUDE.md", &claude_block());
        assert!(case.setup_claude().note.is_none());
        assert_eq!(case.read("CLAUDE.md"), None);
    }

    /// Everything but the plain case: nothing is changed, and the message
    /// gives file, lines and reason.
    #[cfg(unix)]
    #[test]
    fn in_every_other_case_nothing_is_changed() {
        type Make = fn(&Case);
        let cases: &[(&str, Make)] = &[
            ("symbolic link to", |c| {
                std::fs::remove_file(c.p.root.join("CLAUDE.md")).unwrap();
                c.write("AGENTS.md", &format!("rules\n\n{}", claude_block()));
                c.link("AGENTS.md", "CLAUDE.md");
            }),
            ("the same file as AGENTS.md", |c| {
                std::fs::remove_file(c.p.root.join("AGENTS.md")).unwrap();
                c.link("CLAUDE.md", "AGENTS.md");
            }),
            ("2 hard links", |c| {
                std::fs::remove_file(c.p.root.join("AGENTS.md")).unwrap();
                let name = |rel| c.p.root.join(rel);
                std::fs::hard_link(name("CLAUDE.md"), name("AGENTS.md")).unwrap();
            }),
            ("the same file as GEMINI.md", |c| {
                c.link("CLAUDE.md", "GEMINI.md")
            }),
            ("the same file as .claude/CLAUDE.md", |c| {
                c.link("CLAUDE.md", ".claude/CLAUDE.md")
            }),
            ("the same file as packages/app/CLAUDE.md", |c| {
                c.link("CLAUDE.md", "packages/app/CLAUDE.md")
            }),
            ("the same file as packages/app/AGENTS.md", |c| {
                c.write(".gitignore", "packages/app/AGENTS.md\n");
                c.link("CLAUDE.md", "packages/app/AGENTS.md");
            }),
            ("other/CLAUDE.md", |c| {
                c.link("CLAUDE.md", "../other/CLAUDE.md")
            }),
            ("home/.codex/AGENTS.md", |c| {
                c.link("CLAUDE.md", "../home/.codex/AGENTS.md")
            }),
            ("GEMINI.md imports it", |c| {
                c.write("GEMINI.md", "See @CLAUDE.md.\n")
            }),
            ("mine.md imports it", |c| {
                c.write(".claude/rules/mine.md", "@../../CLAUDE.md\n")
            }),
            ("was edited", |c| {
                let text = claude_block().replace(END, "- mine\n<!-- END myceliums -->");
                c.write("CLAUDE.md", &text);
            }),
            ("reformatted", |c| {
                c.write("CLAUDE.md", &claude_block().replacen('\n', "\r\n", 2))
            }),
            ("reformatted", |c| {
                c.write("CLAUDE.md", claude_block().trim_end())
            }),
            ("2 myceliums blocks", |c| {
                c.write("CLAUDE.md", &claude_block().repeat(2))
            }),
            ("without a matching", |c| {
                c.write("CLAUDE.md", &format!("{BEGIN}\nx\n"))
            }),
            ("read-only", |c| {
                let path = c.p.root.join("CLAUDE.md");
                let mut permissions = std::fs::metadata(&path).unwrap().permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(&path, permissions).unwrap();
            }),
            ("git ignores it", |c| c.write(".gitignore", "CLAUDE.md\n")),
            ("git could not be asked", |c| {
                std::fs::remove_dir_all(c.p.root.join(".git")).unwrap();
                c.write(".git", "no git directory\n");
            }),
            ("not in git and also has AGENTS.md", |c| {
                std::fs::remove_dir_all(c.p.root.join(".git")).unwrap();
            }),
        ];
        for (reason, make) in cases {
            let case = Case::new();
            case.write("CLAUDE.md", &claude_block());
            case.write("AGENTS.md", "rules\n");
            make(&case);
            let before = (case.read("CLAUDE.md"), case.read("AGENTS.md"));
            let unchanged = || {
                assert_eq!((case.read("CLAUDE.md"), case.read("AGENTS.md")), before);
                assert!(case.p.root.join("CLAUDE.md").exists());
                assert_eq!(case.read(".claude/rules/myceliums.md"), None, "{reason}");
            };
            let found = findings(&case.p);
            let finding = found
                .iter()
                .find(|f| f.file.ends_with("project/CLAUDE.md"))
                .unwrap_or_else(|| panic!("{reason}: {found:?}"));
            let Fix::ByHand(text) = &finding.fix else {
                panic!("{reason}: {finding:?}");
            };
            assert!(text.contains(reason), "{reason}: {text}");
            assert!(text.contains("CLAUDE.md"), "{text}");
            let step = text.split("By hand: ").nth(1).expect("a step");
            // By the markers where the text is ours, else without a range;
            // never by line numbers.
            let ours = !["was edited", "reformatted", "without a matching"].contains(reason);
            let range = step.contains(&format!("from the line `{BEGIN}` to the next line `{END}`"));
            assert_eq!(range, ours, "{reason}: {step}");
            assert_eq!(step.contains("compare it"), !ours, "{reason}: {step}");
            assert!(!step.contains("lines "), "{reason}: {step}");
            assert!(repair(&case.p, finding).is_err());
            unchanged();
            let routed = case.setup_claude();
            let note = routed
                .note
                .unwrap_or_else(|| panic!("{reason}: a note"))
                .text;
            assert!(note.contains(reason), "{reason}: {note}");
            unchanged();
            // `myc doctor` and the uninstalls list the block.
            let listed = block_list(&[&case.p], &[]).join("\n");
            assert!(listed.contains("CLAUDE.md"), "{reason}: {listed}");
        }
    }

    /// `text` after doing what `step` says, or as it is for a step without a
    /// range.
    fn follow(step: &str, text: &str) -> String {
        if !step.contains(&format!("from the line `{BEGIN}` to the next line `{END}`")) {
            return text.to_string();
        }
        let mut inside = false;
        let mut out = String::new();
        for line in text.split_inclusive('\n') {
            inside |= line.trim() == BEGIN;
            if !inside {
                out.push_str(line);
            }
            inside &= line.trim() != END;
        }
        out
    }

    /// The advice works: after it, setup goes on. Following it twice, under
    /// either name, or for several blocks, takes none of the user's lines.
    #[cfg(unix)]
    #[test]
    fn following_the_advice_twice_is_safe_and_lets_setup_go_on() {
        let block = claude_block();
        let codex = routing_block(Flavor::Agents);
        for (text, user) in [
            (format!("a\n\n{block}b\nc\n"), "a\n\nb\nc\n"),
            (format!("a\n{block}b\n{codex}c\n{block}d\n"), "a\nb\nc\nd\n"),
        ] {
            let case = Case::new();
            case.write("AGENTS.md", &text);
            case.link("AGENTS.md", "CLAUDE.md");
            let note = case.setup_claude().note.unwrap().text;
            let listed = block_list(&[&case.p], &[]);
            assert_eq!(listed.len(), 1, "one entry per file: {listed:?}");
            let steps = [&note, &listed[0], &note, &listed[0]];
            for (step, name) in steps.iter().zip(["CLAUDE.md", "AGENTS.md"].iter().cycle()) {
                let now = follow(step, &case.read(name).unwrap());
                case.write(name, &now);
                assert_eq!(now, user, "{step}");
            }
            assert!(case.setup_claude().note.is_none());
            assert!(case.read(".claude/rules/myceliums.md").is_some());
            assert_eq!(findings(&case.p), []);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_rules_directory_outside_the_project_is_not_written_to() {
        let case = Case::new();
        std::fs::create_dir_all(case.p.root.join("../shared")).unwrap();
        case.link("../shared", ".claude/rules");
        let note = case.setup_claude().note.unwrap().text;
        assert!(note.contains("outside the project"), "{note}");
        assert_eq!(case.read("../shared/myceliums.md"), None);
    }

    #[test]
    fn blocks_are_listed_with_what_removes_them() {
        let case = Case::new();
        case.write(
            "AGENTS.md",
            &format!("rules\n\n{}", routing_block(Flavor::Agents)),
        );
        case.write("packages/app/CLAUDE.md", &claude_block());
        case.setup_claude();
        let listed = block_list(&[&case.p], &[]);
        let step = routing::manual_step(&[]);
        let expected = [
            "AGENTS.md:3 (`myc setup-codex --uninstall` removes it)".to_string(),
            ".claude/rules/myceliums.md:1 (`myc setup-claude --uninstall` removes it)".to_string(),
            format!("packages/app/CLAUDE.md:1 (by hand: {step})"),
        ];
        assert_eq!(listed, expected);
    }

    /// One file with several names is one block to delete, not several.
    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_listed_once() {
        let block = claude_block();
        for kind in [
            "CLAUDE.md -> AGENTS.md",
            "AGENTS.md -> CLAUDE.md",
            "hard",
            "home",
        ] {
            let case = Case::new();
            let (file, link) = match kind {
                "CLAUDE.md -> AGENTS.md" => ("AGENTS.md", "CLAUDE.md"),
                "home" => ("CLAUDE.md", "../home/.claude/CLAUDE.md"),
                _ => ("CLAUDE.md", "AGENTS.md"),
            };
            case.write(file, &format!("mine\n\n{block}mine too\n"));
            if kind == "hard" {
                let name = |rel| case.p.root.join(rel);
                std::fs::hard_link(name(file), name(link)).unwrap();
            } else {
                case.link(file, link);
            }
            let listed = block_list(&[&case.p], &[]);
            assert_eq!(listed.len(), 1, "{kind}: {listed:?}");
            let start = "CLAUDE.md:3 (the same file as ";
            assert!(listed[0].starts_with(start), "{kind}: {listed:?}");
            assert!(
                listed[0].contains("; by hand: delete everything from"),
                "{listed:?}"
            );
            // Once its remedy was printed, the list only refers to it.
            for explained in [file, link] {
                let listed = block_list(&[&case.p], &[case.p.root.join(explained)]);
                assert!(listed[0].ends_with("see above)"), "{kind}: {listed:?}");
                assert!(!listed[0].contains("by hand"), "{kind}: {listed:?}");
            }
        }
    }

    /// Without an answer from git, "nothing is hidden" would be a guess.
    #[test]
    fn what_could_not_be_determined_is_said() {
        let case = Case::new();
        std::fs::remove_dir_all(case.p.root.join(".git")).unwrap();
        case.write(".git", "no git directory\n");
        case.write("CLAUDE.md", &claude_block());
        case.write("packages/app/AGENTS.md", "rules\n");
        let finding = case.finding();
        assert!(finding.hidden.is_empty());
        assert!(finding.is_issue(), "{finding:?}");
        let what = finding.describe();
        assert!(
            what.contains("could not be determined (git ls-files"),
            "{what}"
        );
        assert!(!what.contains("nothing is hidden"), "{what}");
        assert!(matches!(finding.fix, Fix::ByHand(_)), "{finding:?}");
        // What is known is said as well.
        case.write("AGENTS.md", "rules\n");
        let what = case.finding().describe().to_string();
        assert!(what.contains("skip AGENTS.md; whether"), "{what}");
        // Text of the project's own makes CLAUDE.md the project's choice.
        case.write("CLAUDE.md", &format!("mine\n\n{}", claude_block()));
        assert!(!case.finding().is_issue());
    }

    #[test]
    fn setup_codex_writes_into_an_agents_md_and_creates_one_that_hides_nothing() {
        // Neither: created. Only AGENTS.md: the block is appended.
        for (before, after) in [
            (None, FileChange::Created),
            (Some("rules\n"), FileChange::Updated),
        ] {
            let case = Case::new();
            if let Some(text) = before {
                case.write("AGENTS.md", text);
            }
            let install = || install_codex_routing(&case.p).unwrap();
            assert_eq!(install(), CodexRouting::Done(after));
            assert_eq!(install(), CodexRouting::Done(FileChange::Unchanged));
            assert_eq!(findings(&case.p), []);
            let removed = uninstall_codex_routing(&case.p).unwrap();
            assert!(matches!(removed, CodexRouting::Done(_)), "{removed:?}");
            assert_eq!(case.read("AGENTS.md").as_deref(), before);
        }
        // Both: AGENTS.md has text of its own, so agents that skip CLAUDE.md
        // did so before. An import of it does not matter.
        for claude in ["mine\n", "@AGENTS.md\n"] {
            let case = Case::new();
            case.write("AGENTS.md", "rules\n");
            case.write("CLAUDE.md", claude);
            let done = install_codex_routing(&case.p).unwrap();
            assert_eq!(done, CodexRouting::Done(FileChange::Updated));
            let removed = uninstall_codex_routing(&case.p).unwrap();
            assert_eq!(removed, CodexRouting::Done(FileChange::Removed));
            assert_eq!(case.read("AGENTS.md").as_deref(), Some("rules\n"));
        }
    }

    #[test]
    fn setup_codex_creates_no_agents_md_that_hides_another_file() {
        for name in ["CLAUDE.md", "GEMINI.md"] {
            let case = Case::new();
            case.write(name, "mine\n");
            let would_hide = |removed| CodexRouting::WouldHide {
                hidden: vec![case.p.root.join(name)],
                removed,
            };
            assert_eq!(install_codex_routing(&case.p).unwrap(), would_hide(false));
            assert_eq!(case.read("AGENTS.md"), None);

            // One that an older version created is found and removed.
            case.write("AGENTS.md", &routing_block(Flavor::Agents));
            let finding = case.finding();
            assert_eq!(finding.hidden, [case.p.root.join(name)]);
            assert_eq!(finding.fix, Fix::Doctor);
            assert_eq!(install_codex_routing(&case.p).unwrap(), would_hide(true));
            assert_eq!(case.read("AGENTS.md"), None);
            case.write("AGENTS.md", &routing_block(Flavor::Agents));
            repair(&case.p, &finding).unwrap();
            assert_eq!(case.read("AGENTS.md"), None);
            assert_eq!(case.read(name).as_deref(), Some("mine\n"));
        }
        // A CLAUDE.md of our block alone, or an empty one, is nothing to hide.
        for text in ["", "\n", &claude_block()] {
            let case = Case::new();
            case.write("CLAUDE.md", text);
            let done = install_codex_routing(&case.p).unwrap();
            assert_eq!(done, CodexRouting::Done(FileChange::Created));
        }
    }

    #[test]
    fn codex_fallback_file_names_count() {
        let mut case = Case::new();
        case.write(
            "../home/.codex/config.toml",
            "project_doc_fallback_filenames = [\"TEAM_GUIDE.md\"]\n",
        );
        case.write("TEAM_GUIDE.md", "team rules\n");
        assert!(matches!(
            install_codex_routing(&case.p).unwrap(),
            CodexRouting::WouldHide { .. }
        ));
        // Without that setting the file is no instruction file.
        case.p.home = None;
        assert_eq!(
            install_codex_routing(&case.p).unwrap(),
            CodexRouting::Done(FileChange::Created)
        );
    }

    /// An AGENTS.md that is a link (dangling ones included), has another name,
    /// or whose block was edited is left alone by `setup-codex` and its
    /// uninstall; so is one that an import would be left without.
    #[cfg(unix)]
    #[test]
    fn setup_codex_leaves_files_it_shares_alone() {
        let block = routing_block(Flavor::Agents);
        for case_name in [
            "link",
            "dangling",
            "linked",
            "hard link",
            "edited",
            "imported",
        ] {
            let case = Case::new();
            let root = &case.p.root;
            match case_name {
                "link" => {
                    case.write("CLAUDE.md", &format!("rules\n\n{block}"));
                    case.link("CLAUDE.md", "AGENTS.md");
                }
                "dangling" => case.link("missing.md", "AGENTS.md"),
                "linked" => {
                    case.write("AGENTS.md", &format!("rules\n\n{block}"));
                    case.link("AGENTS.md", "CLAUDE.md");
                }
                "hard link" => {
                    case.write("AGENTS.md", &format!("rules\n\n{block}"));
                    std::fs::hard_link(root.join("AGENTS.md"), root.join("CLAUDE.md")).unwrap();
                }
                "edited" => {
                    let edited = block.replace("grep/rg stay", "grep stays");
                    case.write("AGENTS.md", &format!("rules\n\n{edited}"));
                }
                _ => {
                    case.write("AGENTS.md", &block);
                    case.write("CLAUDE.md", "@AGENTS.md\nmine\n");
                }
            }
            let before = (case.read("AGENTS.md"), case.read("CLAUDE.md"));
            let installed = install_codex_routing(&case.p).unwrap();
            assert!(
                matches!(installed, CodexRouting::Left(_)),
                "{case_name}: {installed:?}"
            );
            let removed = uninstall_codex_routing(&case.p).unwrap();
            match removed {
                CodexRouting::Left(text) => assert!(text.contains("AGENTS.md:"), "{text}"),
                other => {
                    assert_eq!(case_name, "dangling", "{other:?}");
                    assert_eq!(other, CodexRouting::Done(FileChange::Absent));
                }
            }
            assert_eq!((case.read("AGENTS.md"), case.read("CLAUDE.md")), before);
            assert!(!root.join("missing.md").exists());
        }
    }
}
