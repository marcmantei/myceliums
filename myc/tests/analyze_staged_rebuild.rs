//! #175: `myc analyze` rebuilds an index next to the live one and swaps it
//! in, as the MCP `analyze` tool does. It used to empty the live index and
//! rebuild it in place, so a reader saw an empty or half-built index — and,
//! once the index was a link to a generation, rewrote the generation that
//! readers trust.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use myceliums_storage::{RepoRegistry, Store};

fn myc(data: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_myc"));
    cmd.args(args)
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: some code paths derive paths from HOME.
        .env("HOME", data)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

const ANALYZE: [&str; 5] = [
    "analyze",
    ".",
    "--force",
    "--skip-embeddings",
    "--no-git-check",
];

fn check(out: &Output) {
    assert!(
        out.status.success(),
        "myc analyze failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn names(store: &Store) -> myceliums_storage::Result<Vec<String>> {
    let mut names: Vec<String> = store
        .get_symbols()
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    names.sort();
    Ok(names)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_cli_rebuild_is_staged_and_a_reader_keeps_the_old_index() {
    let data = tempfile::tempdir().unwrap();
    // Not a dot-directory: discovery skips hidden directories.
    let project = tempfile::Builder::new().prefix("proj").tempdir().unwrap();
    std::fs::write(
        project.path().join("lib.py"),
        "def greet(name):\n    return 'hi ' + name\n",
    )
    .unwrap();
    check(&myc(data.path(), project.path(), &ANALYZE).output().unwrap());
    let registry = RepoRegistry::load(&data.path().join("repos.json")).unwrap();
    let id = registry.list()[0].id.clone();
    let live = RepoRegistry::repo_db_path(data.path(), &id);
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(&live)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the CLI builds a generation and links it"
    );

    // A reader opens the index, then the CLI rebuilds it with other code.
    let reader = Store::open_existing(&live, &id).await.unwrap();
    let before = names(&reader).await.unwrap();
    assert_eq!(before, ["greet"]);
    std::fs::write(
        project.path().join("lib.py"),
        "def farewell(name):\n    return 'bye ' + name\n\ndef wave():\n    pass\n",
    )
    .unwrap();
    let mut child = myc(data.path(), project.path(), &ANALYZE).spawn().unwrap();

    // While the CLI runs, the reader keeps the previous index, whole; once
    // it is swapped out and deleted, reads fail — they never show an empty,
    // half-built or new index.
    let mut whole = 0;
    while child.try_wait().unwrap().is_none() {
        match names(&reader).await {
            Ok(names) => {
                assert_eq!(names, before, "a reader saw the index change under it");
                whole += 1;
            }
            Err(_) => break,
        }
    }
    check(&child.wait_with_output().unwrap());
    assert!(whole > 0, "no read ran while the CLI rebuilt");
    if let Ok(names) = names(&reader).await {
        assert_eq!(names, before);
    }

    let fresh = Store::open_existing(&live, &id).await.unwrap();
    assert_eq!(names(&fresh).await.unwrap(), ["farewell", "wave"]);
    assert_ne!(fresh.path(), reader.path(), "a new generation");
    #[cfg(unix)]
    {
        let generations = data.path().join("data").join(".generations");
        assert_eq!(
            std::fs::read_dir(&generations).unwrap().count(),
            1,
            "the previous generation is deleted"
        );
    }
    let staging = data.path().join("data").join(".staging");
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
}
