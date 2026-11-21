//! `myc analyze` rebuilds a registered repository whose index directory is
//! gone instead of answering "Using cached analysis" for an index that no
//! longer exists (#118: the branch in `commands/analyze.rs` was untested).

use std::path::Path;
use std::process::{Command, Output, Stdio};

use myceliums_storage::RepoRegistry;

fn myc(data: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(args)
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: some code paths derive paths from HOME.
        .env("HOME", data)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run myc")
}

fn analyze(data: &Path, project: &Path) -> String {
    let out = myc(
        data,
        project,
        &["analyze", ".", "--skip-embeddings", "--no-git-check"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "myc analyze failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn analyze_rebuilds_a_registered_repository_whose_index_is_missing() {
    let data = tempfile::tempdir().unwrap();
    // Not a dot-directory: discovery skips hidden directories.
    let project = tempfile::Builder::new().prefix("proj").tempdir().unwrap();
    std::fs::write(
        project.path().join("lib.py"),
        "def greet(name):\n    return 'hi ' + name\n",
    )
    .unwrap();

    analyze(data.path(), project.path());
    let registry = RepoRegistry::load(&data.path().join("repos.json")).unwrap();
    let repos = registry.list();
    assert_eq!(repos.len(), 1, "one repository registered");
    let db = RepoRegistry::repo_db_path(data.path(), &repos[0].id);
    assert!(db.is_dir(), "the first analysis builds the index");

    std::fs::remove_dir_all(&db).unwrap();
    let rebuilt = analyze(data.path(), project.path());
    assert!(
        rebuilt.contains("Re-analysis needed: the index") && rebuilt.contains("is missing"),
        "{rebuilt}"
    );
    assert!(!rebuilt.contains("Using cached analysis"), "{rebuilt}");
    assert!(db.is_dir(), "the index is built again");

    // The rebuilt index is a real one: reads find the symbol.
    let out = myc(data.path(), project.path(), &["search", "greet"]);
    let found = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && found.contains("greet"), "{found}");
}
