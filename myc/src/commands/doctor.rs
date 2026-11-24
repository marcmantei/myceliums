//! `myc doctor`.

use crate::commands::instructions::{self, Fix, Project};
use crate::commands::repos::find_orphans;
use crate::commands::setup_claude;
use crate::repo::{data_dir, registry_path};
use crate::ui::format_bytes;
use anyhow::Result;
use myceliums_core::config::{self, ProjectConfig};
use myceliums_core::{
    check_model_cache, local_model_cached, local_model_code, Embedder, GlobalConfig,
    IndexEmbeddingMeta,
};
use myceliums_storage::RepoRegistry;
use std::path::{Path, PathBuf};

/// A project whose instruction files are checked.
struct Checked {
    project: Project,
    /// Around the working directory, as opposed to merely indexed.
    current: bool,
}

/// The projects whose instruction files are checked: the one around the
/// working directory (its git top level, and the directory itself), the other
/// worktrees of its repository, and every indexed one that still exists. Never
/// the home directory.
fn projects(
    cwd: Option<PathBuf>,
    registry: Option<&RepoRegistry>,
    home: Option<&Path>,
) -> Vec<Checked> {
    let indexed: Vec<PathBuf> = registry
        .map(|r| r.list())
        .unwrap_or_default()
        .into_iter()
        .map(|repo| PathBuf::from(&repo.path))
        .collect();
    let top = cwd.as_deref().and_then(setup_claude::git_toplevel);
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    let mut add = |root: PathBuf, current: bool| {
        // An indexed project that no longer exists is skipped here.
        if let Ok(root) = std::fs::canonicalize(root) {
            let known = roots.iter().any(|(known, _)| *known == root);
            if root.is_dir() && Some(root.as_path()) != home && !known {
                roots.push((root, current));
            }
        }
    };
    let project = |root: &Path| Project {
        root: root.to_path_buf(),
        home: home.map(Path::to_path_buf),
        others: indexed.clone(),
    };
    let worktrees = top.as_deref().map(|top| project(top).worktrees());
    for root in top.into_iter().chain(cwd) {
        add(root, true);
    }
    for root in worktrees
        .unwrap_or_default()
        .into_iter()
        .chain(indexed.clone())
    {
        add(root, false);
    }
    roots
        .into_iter()
        .map(|(root, current)| Checked {
            project: project(&root),
            current,
        })
        .collect()
}

/// The lines about instruction files that an older `myc` wrote (#200), and
/// the number of issues left. With `fix`, files that hide a project's own
/// are repaired in the current project, with `all` in every one, where that
/// is plain; everything else is only said.
fn check_instructions(projects: &[Checked], fix: bool, all: bool) -> (Vec<String>, u32) {
    let mut lines = Vec::new();
    for Checked { project, .. } in projects.iter().filter(|p| fix && (p.current || all)) {
        for finding in instructions::findings(project) {
            if finding.fix != Fix::Doctor || finding.hidden.is_empty() {
                continue;
            }
            lines.push(match instructions::repair(project, &finding) {
                Ok(done) => format!("\u{2713} Instruction files repaired: {done}"),
                Err(e) => format!("\u{2717} Not repaired: {e:#}"),
            });
        }
    }
    let mut issues = 0;
    // A file is reported once, also when several projects see it (a git top
    // level and a directory below it, a worktree and the project above).
    let mut reported: Vec<PathBuf> = Vec::new();
    for Checked { project, current } in projects {
        for finding in instructions::findings(project) {
            let id = instructions::file_id(&finding.file);
            if reported.iter().any(|p| instructions::file_id(p) == id) {
                continue;
            }
            reported.push(finding.file.clone());
            let issue = finding.is_issue();
            let mark = if issue { "\u{2717}" } else { "-" };
            issues += u32::from(issue);
            lines.push(format!("{mark} Instruction files: {}", finding.describe()));
            let command = |dir: &Path| {
                let dir = setup_claude::shell_quote(&dir.to_string_lossy());
                format!("    to fix: cd {dir} && myc doctor --fix")
            };
            match &finding.fix {
                Fix::ByHand(text) => lines.push(format!("    {text}")),
                Fix::Above(dir) => lines.push(command(dir)),
                Fix::Doctor if finding.hidden.is_empty() => {}
                Fix::Doctor if *current => lines.push("    to fix: myc doctor --fix".to_string()),
                Fix::Doctor => lines.push(command(&project.root)),
            }
        }
    }
    if lines.is_empty() {
        lines.push("\u{2713} Instruction files: none hidden by myceliums".to_string());
    }
    let current: Vec<&Project> = projects
        .iter()
        .filter(|p| p.current)
        .map(|p| &p.project)
        .collect();
    for block in instructions::block_list(&current, &reported) {
        lines.push(format!("- Routing block: {block}"));
    }
    (lines, issues)
}

pub(crate) async fn cmd_doctor(download: bool, fix: bool, all_projects: bool) -> Result<()> {
    use myceliums_core::parser::{SourceLanguage, SourceParser};
    use std::process::Command;

    println!("Myceliums Health Check");
    println!("=====================");
    println!();

    let data = data_dir();
    let mut issues = 0u32;

    // ── Rust toolchain (informational) ────────────────────────────────
    match Command::new("rustc").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("\u{2713} Rust toolchain: {}", version.trim());
        }
        _ => {
            println!("- Rust toolchain: not found (optional, only needed for development)");
        }
    }

    match Command::new("cargo").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("\u{2713} Cargo: {}", version.trim());
        }
        _ => {
            println!("- Cargo: not found (optional, only needed for development)");
        }
    }

    // ── Data directory ────────────────────────────────────────────────
    if data.exists() && data.is_dir() {
        let test_file = data.join(".write_test");
        match std::fs::write(&test_file, b"test") {
            Ok(_) => {
                // best-effort: the write test already succeeded; removing the
                // probe file is cleanup and does not affect the reported result.
                let _ = std::fs::remove_file(&test_file);
                println!(
                    "\u{2713} Data directory ({}) exists and is writable",
                    data.display()
                );
            }
            Err(_) => {
                println!(
                    "\u{2717} Data directory ({}) exists but is NOT writable",
                    data.display()
                );
                issues += 1;
            }
        }
    } else {
        println!(
            "\u{2717} Data directory ({}) does not exist — run 'myc analyze <path>' to create it",
            data.display()
        );
        issues += 1;
    }

    // ── Registry file ─────────────────────────────────────────────────
    let reg_path = registry_path();
    let registry = if reg_path.exists() {
        match RepoRegistry::load(&reg_path) {
            Ok(reg) => {
                println!("\u{2713} Registry file (repos.json) is valid");
                Some(reg)
            }
            Err(e) => {
                println!(
                    "\u{2717} Registry file (repos.json) exists but is invalid: {}",
                    e
                );
                issues += 1;
                None
            }
        }
    } else {
        println!(
            "\u{2713} Registry file (repos.json) not yet created (will be created on first analyze)"
        );
        None
    };

    // ── LanceDB repository data ───────────────────────────────────────
    let mut total_repos = 0u32;
    let mut healthy_repos = 0u32;
    if let Some(ref registry) = registry {
        let repos = registry.list();
        total_repos = repos.len() as u32;
        for repo in &repos {
            let db_path = RepoRegistry::repo_db_path(&data, &repo.id);
            if db_path.exists() && db_path.is_dir() {
                let table_count = std::fs::read_dir(&db_path)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .count()
                    })
                    .unwrap_or(0);
                println!("\u{2713} LanceDB: {} ({} tables)", repo.name, table_count);
                healthy_repos += 1;
            } else {
                println!(
                    "\u{2717} LanceDB: {} (data directory missing at {})",
                    repo.name,
                    db_path.display()
                );
                issues += 1;
            }
        }
    }

    // ── Orphaned data directories ────────────────────────────────────
    match find_orphans() {
        Ok(orphans) if orphans.is_empty() => {
            println!("\u{2713} No orphaned data directories");
        }
        Ok(orphans) => {
            let total: u64 = orphans.iter().map(|(_, _, s)| s).sum();
            println!(
                "\u{2717} {} orphaned data directories ({}) — run 'myc clean --orphans' to remove",
                orphans.len(),
                format_bytes(total)
            );
            for (name, _, size) in &orphans {
                println!("    {} ({})", name, format_bytes(*size));
            }
            issues += 1;
        }
        Err(e) => {
            println!("\u{2717} Could not check for orphaned directories: {}", e);
            issues += 1;
        }
    }

    // ── Stale analysis locks ──────────────────────────────────────────
    {
        let data_path = data.join("data");
        let mut stale_locks = 0u32;
        if data_path.exists() {
            // Locks live in data/.locks/<repo>.lock (older releases wrote
            // data/<repo>/analysis.lock; those are checked too).
            let locks = std::fs::read_dir(myceliums_core::lock::locks_dir(&data))
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path());
            let legacy = std::fs::read_dir(&data_path)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path().join("analysis.lock"));
            for lock_path in locks.chain(legacy) {
                if lock_path.is_file() {
                    if let Ok(content) = std::fs::read_to_string(&lock_path) {
                        let pid = myceliums_core::lock::parse_lock_pid(&content);
                        let alive = pid.is_some_and(myceliums_core::lock::is_process_alive);
                        if !alive {
                            stale_locks += 1;
                            println!(
                                "\u{2717} Stale lock: {} (PID {:?})",
                                lock_path.display(),
                                pid
                            );
                        }
                    }
                }
            }
        }
        if stale_locks == 0 {
            println!("\u{2713} No stale analysis locks");
        } else {
            println!("  Run 'myc clean --orphans' or manually remove the .lock files");
            issues += stale_locks;
        }
    }

    // ── Tree-sitter grammars ──────────────────────────────────────────
    let parsers_to_check = [
        ("TypeScript", SourceLanguage::TypeScript),
        ("TSX", SourceLanguage::Tsx),
        ("JavaScript", SourceLanguage::JavaScript),
        ("Python", SourceLanguage::Python),
        ("Go", SourceLanguage::Go),
        ("Rust", SourceLanguage::Rust),
        ("Java", SourceLanguage::Java),
        ("C#", SourceLanguage::CSharp),
        ("C", SourceLanguage::C),
        ("C++", SourceLanguage::Cpp),
        ("Ruby", SourceLanguage::Ruby),
        ("Kotlin", SourceLanguage::Kotlin),
        ("Swift", SourceLanguage::Swift),
        ("PHP", SourceLanguage::Php),
        ("Lua", SourceLanguage::Lua),
        ("Zig", SourceLanguage::Zig),
        ("PowerShell", SourceLanguage::PowerShell),
        ("Elixir", SourceLanguage::Elixir),
        ("Scala", SourceLanguage::Scala),
        ("Objective-C", SourceLanguage::ObjectiveC),
        ("Dart", SourceLanguage::Dart),
        ("Vue", SourceLanguage::Vue),
        ("Svelte", SourceLanguage::Svelte),
    ];
    // The grammar for each language comes from core's single table
    // (`SourceLanguage::tree_sitter_language`), via the same constructor the
    // analyzer uses.
    for (name, lang) in &parsers_to_check {
        match SourceParser::new(*lang) {
            Ok(_) => println!("\u{2713} tree-sitter grammar: {}", name),
            Err(e) => {
                println!(
                    "\u{2717} tree-sitter grammar: {} — {}",
                    name,
                    anyhow::Error::from(e).root_cause()
                );
                issues += 1;
            }
        }
    }

    // ── Embedding configuration + model cache ─────────────────────────
    // Resolve the embedding config the same way analysis would: from
    // .myceliums.toml in the current directory, falling back to defaults.
    let embedding_cfg = {
        let config_path = std::path::Path::new(config::CONFIG_FILENAME);
        if config_path.exists() {
            match ProjectConfig::load(config_path) {
                Ok(cfg) => cfg.embedding,
                Err(e) => {
                    println!("\u{2717} .myceliums.toml is invalid: {}", e);
                    issues += 1;
                    Default::default()
                }
            }
        } else {
            Default::default()
        }
    };
    match IndexEmbeddingMeta::from_config(&embedding_cfg) {
        Ok(meta) => {
            println!(
                "\u{2713} Embedding config: {} (reranker: {})",
                meta.identity(),
                embedding_cfg.reranker,
            );
            if meta.provider == "local" {
                let model_code = local_model_code(&meta.model)?;
                let cache_info = check_model_cache(&model_code);
                // The pinned revision's files, not any snapshot of the repo.
                if local_model_cached(&meta.model) {
                    println!(
                        "\u{2713} Embedding model: cached ({:.0} MB at {})",
                        cache_info.size_bytes as f64 / 1_000_000.0,
                        cache_info.cache_dir.display(),
                    );
                } else if download {
                    println!("Downloading embedding model {}...", model_code);
                    match Embedder::new(meta.clone()) {
                        Ok(_) => {
                            let updated = check_model_cache(&model_code);
                            println!(
                                "\u{2713} Embedding model: downloaded ({:.0} MB to {})",
                                updated.size_bytes as f64 / 1_000_000.0,
                                updated.cache_dir.display(),
                            );
                        }
                        Err(e) => {
                            println!("\u{2717} Embedding model: download failed — {}", e);
                            issues += 1;
                        }
                    }
                } else {
                    println!(
                        "\u{2717} Embedding model: not downloaded (run `myc doctor --download` or it will download on first use)"
                    );
                    issues += 1;
                }
            }
            println!(
                "  Available local models: {}",
                myceliums_core::EMBEDDING_MODELS
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        Err(e) => {
            println!("\u{2717} Embedding config: {}", e);
            issues += 1;
        }
    }

    // ── Content type support ────────────────────────────────────────────
    let content_types = [
        "Markdown (.md, .markdown)",
        "MDX (.mdx)",
        "Plain text (.txt)",
    ];
    for ct in &content_types {
        println!("\u{2713} Content type supported: {}", ct);
    }

    // ── Global configuration ──────────────────────────────────────────
    match GlobalConfig::load(&data) {
        Ok(_) => println!("\u{2713} Global configuration is valid"),
        Err(e) => {
            println!("\u{2717} Global configuration is invalid: {}", e);
            issues += 1;
        }
    }

    // ── Project config in CWD (if present) ────────────────────────────
    if let Ok(cwd) = std::env::current_dir() {
        let project_config_path = cwd.join(config::CONFIG_FILENAME);
        if project_config_path.exists() {
            match ProjectConfig::load(&project_config_path) {
                Ok(cfg) => {
                    let name = if cfg.project.name.is_empty() {
                        "(default)".to_string()
                    } else {
                        cfg.project.name.clone()
                    };
                    println!(
                        "\u{2713} Project config ({}) is valid — project: {}",
                        config::CONFIG_FILENAME,
                        name
                    );
                }
                Err(e) => {
                    println!(
                        "\u{2717} Project config ({}) is invalid: {}",
                        config::CONFIG_FILENAME,
                        e
                    );
                    issues += 1;
                }
            }
        }
    }

    // ── Instruction files (CLAUDE.md, AGENTS.md) ─────────────────────
    {
        let home = dirs::home_dir().map(|h| std::fs::canonicalize(&h).unwrap_or(h));
        let cwd = std::env::current_dir().ok();
        let projects = projects(cwd, registry.as_ref(), home.as_deref());
        let (lines, left) = check_instructions(&projects, fix, all_projects);
        for line in lines {
            println!("{line}");
        }
        issues += left;
    }

    // ── Summary ───────────────────────────────────────────────────────
    println!();
    if total_repos > 0 {
        let needs_reanalysis = total_repos - healthy_repos;
        if needs_reanalysis > 0 {
            println!(
                "{} repositories registered, {} healthy, {} need re-analysis",
                total_repos, healthy_repos, needs_reanalysis
            );
        } else {
            println!("{} repositories registered, all healthy", total_repos);
        }
    }

    if issues == 0 {
        println!("\u{2713} All checks passed");
    } else {
        println!("\u{2717} {} issue(s) found", issues);
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::routing::{routing_block, Flavor};

    struct Setup {
        tmp: tempfile::TempDir,
        registry: RepoRegistry,
    }

    impl Setup {
        fn new() -> Self {
            Self {
                tmp: tempfile::tempdir().unwrap(),
                registry: RepoRegistry::default(),
            }
        }

        /// A git project whose CLAUDE.md of the routing block hides AGENTS.md.
        fn hidden(&self, name: &str) -> PathBuf {
            let root = self.top().join(name);
            std::fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q"]);
            hide(&root);
            root
        }

        fn top(&self) -> PathBuf {
            self.tmp.path().canonicalize().unwrap()
        }

        fn register(&mut self, root: &Path) {
            let path = root.to_string_lossy();
            let info = serde_json::from_value(serde_json::json!({
                "id": path, "name": path, "path": path, "analyzed_at": "",
                "symbol_count": 0, "file_count": 0,
            }))
            .unwrap();
            self.registry.register(info);
        }

        fn check(&self, cwd: &Path, fix: bool, all: bool) -> (String, u32) {
            let projects = projects(Some(cwd.to_path_buf()), Some(&self.registry), None);
            let (lines, issues) = check_instructions(&projects, fix, all);
            (lines.join("\n"), issues)
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn hide(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        let block = routing_block(Flavor::Claude { subagent: true });
        std::fs::write(root.join("CLAUDE.md"), block).unwrap();
        std::fs::write(root.join("AGENTS.md"), "rules\n").unwrap();
    }

    fn is_hidden(root: &Path) -> bool {
        root.join("CLAUDE.md").exists()
    }

    #[test]
    fn fix_repairs_the_current_project_and_lists_the_indexed_ones() {
        let mut s = Setup::new();
        let (current, indexed) = (s.hidden("current"), s.hidden("other project"));
        s.register(&current);
        s.register(&indexed);
        s.register(&s.tmp.path().join("gone"));

        let (lines, issues) = s.check(&current, false, false);
        assert_eq!(issues, 2, "{lines}");
        assert!(lines.contains("to fix: myc doctor --fix"), "{lines}");
        let command = format!("to fix: cd '{}' && myc doctor --fix", indexed.display());
        assert!(lines.contains(&command), "{lines}");
        assert!(is_hidden(&current) && is_hidden(&indexed));
        assert!(!lines.contains("gone"), "{lines}");
        assert!(!s.tmp.path().join("gone").exists());

        let (lines, issues) = s.check(&current, true, false);
        assert_eq!(issues, 1, "{lines}");
        assert!(lines.contains("repaired") && lines.contains(&command));
        assert!(!is_hidden(&current) && is_hidden(&indexed));

        let (lines, issues) = s.check(&current, true, true);
        assert_eq!(issues, 0, "{lines}");
        assert!(!is_hidden(&indexed));
        assert_eq!(s.check(&current, false, false).1, 0);
        assert!(!s.tmp.path().join("gone").exists());
    }

    /// The directory the listed command changes to may be below a git top
    /// level; it is repaired there as well.
    #[test]
    fn a_project_below_a_git_top_level_is_current_in_its_directory() {
        let mut s = Setup::new();
        let (repo, sub) = (s.top().join("repo"), s.top().join("repo/sub"));
        hide(&sub);
        git(&repo, &["init", "-q"]);
        s.register(&sub);
        let repo = repo.as_path();
        assert_eq!(s.check(repo, true, false).1, 1, "indexed only");
        assert_eq!(s.check(&sub, true, false).1, 0);
        assert!(!is_hidden(&sub));
    }

    #[test]
    fn fix_leaves_an_old_block_that_hides_nothing() {
        let s = Setup::new();
        let root = s.hidden("project");
        let block = routing_block(Flavor::Claude { subagent: true });
        let text = format!("mine\n\n{block}");
        std::fs::write(root.join("CLAUDE.md"), &text).unwrap();
        for all in [false, true] {
            let (lines, issues) = s.check(&root, true, all);
            assert_eq!(issues, 0, "{lines}");
            assert!(lines.contains("nothing is hidden"), "{lines}");
            let now = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
            assert_eq!(now, text);
            assert!(!root.join(".claude").exists());
        }
    }

    /// A worktree inside the project is checked too, and the project's own
    /// CLAUDE.md is reported from within it.
    #[test]
    fn worktrees_and_the_project_above_are_reported() {
        let s = Setup::new();
        let root = s.hidden("project");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        git(&root, &["worktree", "add", "-q", ".claude/worktrees/w"]);
        let worktree = root.join(".claude/worktrees/w");
        let fix_in = |dir: &Path| format!("to fix: cd {} && myc doctor --fix", dir.display());

        let (lines, _) = s.check(&root, false, false);
        assert!(lines.contains("to fix: myc doctor --fix"), "{lines}");
        assert!(lines.contains(&fix_in(&worktree)), "{lines}");
        let (lines, _) = s.check(&worktree, true, false);
        assert!(lines.contains("repaired"), "{lines}");
        assert!(lines.contains("above this project"), "{lines}");
        assert!(lines.contains(&fix_in(&root)), "{lines}");
        assert!(!is_hidden(&worktree) && is_hidden(&root));
        let (lines, issues) = s.check(&root, true, false);
        assert_eq!(issues, 0, "{lines}");
        assert!(!is_hidden(&root));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_project_is_reported_and_does_not_stop_the_others() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut s = Setup::new();
        let (locked, other) = (s.hidden("a-locked"), s.hidden("b-other"));
        s.register(&locked);
        s.register(&other);
        let set_mode = |mode| {
            let permissions = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(&locked, permissions).unwrap();
        };
        set_mode(0o555);
        let (lines, issues) = s.check(&other, true, true);
        set_mode(0o755);
        assert_eq!(issues, 1, "{lines}");
        assert!(lines.contains("it is read-only"), "{lines}");
        assert!(is_hidden(&locked), "nothing was changed there");
        assert!(!is_hidden(&other), "{lines}");
    }

    /// Every block of the current project is listed, hidden files or not.
    #[test]
    fn blocks_are_listed_with_file_and_line() {
        let s = Setup::new();
        let root = s.hidden("project");
        std::fs::remove_file(root.join("CLAUDE.md")).unwrap();
        let block = routing_block(Flavor::Claude { subagent: true });
        std::fs::write(root.join("GEMINI.md"), format!("rules\n\n{block}")).unwrap();
        let (lines, issues) = s.check(&root, false, false);
        assert_eq!(issues, 0, "{lines}");
        assert!(lines.contains("none hidden"), "{lines}");
        assert!(lines.contains("- Routing block: GEMINI.md:3 "), "{lines}");
    }
}
