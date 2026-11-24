//! `myc setup-claude` and `myc setup-codex` hide no instruction file of the
//! project (#200), and `myc doctor` finds and repairs what older versions
//! left. The file combinations are unit tests (`setup_claude.rs`,
//! `instructions.rs`); what Claude Code loads is checked with
//! `evals/agent/probe_instructions.py`.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const OLD_BLOCK: &str = "<!-- BEGIN myceliums -->\n\
<!-- v3, written by `myc setup-claude`: edits inside these markers are overwritten; `myc setup-claude --uninstall` removes them -->\n\
## Code navigation: myceliums\n\
<!-- END myceliums -->\n";

/// A project in a temporary home, so no real file is read or written.
struct Project {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let root = home.join("project");
        std::fs::create_dir_all(&root).unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .expect("git runs");
        assert!(status.success());
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self {
            _tmp: tmp,
            home,
            root,
        }
    }

    fn myc(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myc"))
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("MYCELIUMS_DATA_DIR", self.home.join("data"))
            .stdin(Stdio::null())
            .output()
            .expect("run myc")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.myc(args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// The `Instruction files` lines of `myc doctor`, which exits with an
    /// error for other reasons here (no data directory, no model).
    fn doctor(&self, args: &[&str]) -> String {
        let out = self.myc(&[&["doctor"], args].concat());
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|line| line.contains("Instruction files"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }
}

#[test]
fn setup_claude_keeps_agents_md_readable_and_uninstall_removes_everything() {
    let p = Project::new(&[("AGENTS.md", "rules\n")]);
    let out = p.ok(&["setup-claude"]);
    assert!(out.contains(".claude/rules/myceliums.md"), "{out}");
    assert_eq!(p.read("CLAUDE.md"), None);
    assert_eq!(p.read("AGENTS.md").as_deref(), Some("rules\n"));
    assert!(p
        .read(".claude/rules/myceliums.md")
        .is_some_and(|t| t.contains("BEGIN myceliums")));
    assert!(p.doctor(&[]).contains("none hidden"), "{}", p.doctor(&[]));

    p.ok(&["setup-claude", "--uninstall"]);
    assert_eq!(p.read("AGENTS.md").as_deref(), Some("rules\n"));
    assert!(!p.root.join(".claude").exists());
    assert!(!p.root.join(".mcp.json").exists());
}

#[test]
fn doctor_finds_and_fixes_an_old_installation() {
    let p = Project::new(&[
        ("AGENTS.md", "rules\n"),
        ("packages/app/AGENTS.md", "app rules\n"),
        ("CLAUDE.md", OLD_BLOCK),
    ]);
    let found = p.doctor(&[]);
    assert!(
        found.contains("makes Claude Code skip AGENTS.md, packages/app/AGENTS.md"),
        "{found}"
    );
    assert_eq!(p.read("CLAUDE.md").as_deref(), Some(OLD_BLOCK), "no change");

    let fixed = p.doctor(&["--fix"]);
    assert!(fixed.contains("repaired"), "{fixed}");
    assert_eq!(p.read("CLAUDE.md"), None);
    assert!(p.read(".claude/rules/myceliums.md").is_some());
    assert_eq!(p.read("AGENTS.md").as_deref(), Some("rules\n"));
    assert!(p.doctor(&[]).contains("none hidden"));
}

#[test]
fn a_repeated_setup_repairs_an_old_installation() {
    let p = Project::new(&[("AGENTS.md", "rules\n"), ("CLAUDE.md", OLD_BLOCK)]);
    let out = p.ok(&["setup-claude"]);
    assert!(out.contains("moved out of CLAUDE.md"), "{out}");
    assert_eq!(p.read("CLAUDE.md"), None);
    assert!(p.doctor(&[]).contains("none hidden"));
}

#[test]
fn setup_codex_creates_no_agents_md_beside_a_claude_md() {
    let p = Project::new(&[("CLAUDE.md", "rules\n")]);
    let out = p.ok(&["setup-codex"]);
    assert!(out.contains("would hide CLAUDE.md"), "{out}");
    assert_eq!(p.read("AGENTS.md"), None);

    // With text of its own, AGENTS.md gets the block and uninstall takes it
    // out again.
    std::fs::write(p.root.join("AGENTS.md"), "agents rules\n").unwrap();
    p.ok(&["setup-codex"]);
    assert!(p.read("AGENTS.md").unwrap().contains("BEGIN myceliums"));
    p.ok(&["setup-codex", "--uninstall"]);
    assert_eq!(p.read("AGENTS.md").as_deref(), Some("agents rules\n"));
    assert_eq!(p.read("CLAUDE.md").as_deref(), Some("rules\n"));
}
