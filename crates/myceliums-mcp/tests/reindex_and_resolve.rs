//! Two contract guarantees that need their own projects and index (#96):
//!
//! * When the `Index:` line says the index is stale or edited since
//!   indexing, `analyze` (without `force`) really re-indexes, and the line
//!   then says up to date — the advice is never a no-op, also for an
//!   uncommitted deletion that `git status` keeps listing.
//! * Two real top-level functions with the same name (the parser gives both
//!   `qualified_name == name`) are reported as ambiguous, and the
//!   `file_path:qualified_name` handle from the error picks one — for
//!   `symbol_context` and for `rename_symbol` alike.
//!
//! * Each analysis records a snapshot, so `detect_architecture_drift`
//!   compares the current graph with the analysis before it (#97).
//!
//! This binary owns its data directory, so it may analyze; and because
//! `MYCELIUMS_DATA_DIR` is process-wide, both scenarios run in one test.

use myceliums_mcp::{
    AnalyzeParams, DetectDriftParams, GetStatsParams, MyceliumsMcp, RenameParams,
    SymbolContextParams,
};
use std::path::Path;
use std::process::{Command, Stdio};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

fn analyze(project: &Path) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        skip_embeddings: Some(true),
        ..Default::default()
    }
}

async fn index_line(server: &MyceliumsMcp, repo_id: &str) -> String {
    let stats = server
        .get_stats_for_test(GetStatsParams {
            repo_id: repo_id.to_string(),
        })
        .await
        .expect("get_stats");
    let stats: serde_json::Value = serde_json::from_str(&stats).unwrap();
    stats["index_status"].as_str().unwrap().to_string()
}

async fn drift(server: &MyceliumsMcp, repo_id: &str) -> Result<String, String> {
    server
        .detect_architecture_drift_for_test(DetectDriftParams {
            repo_id: Some(repo_id.to_string()),
            ..Default::default()
        })
        .await
}

#[tokio::test]
async fn analyze_refreshes_what_the_index_line_calls_stale_and_same_names_are_ambiguous() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    let projects = tempfile::tempdir().unwrap();
    let server = MyceliumsMcp::new();

    // ── Freshness advice is never a no-op ────────────────────────────────
    let project = projects.path().join("fresh-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/app.ts"),
        "export function start(): number {\n    return 1;\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/extra.ts"),
        "export function extra(): number {\n    return 3;\n}\n",
    )
    .unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "init"]);
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let first = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(first.contains("(fresh)"), "{first}");
    let line = index_line(&server, &repo_id).await;
    assert!(line.ends_with("= HEAD, up to date."), "{line}");
    let none_yet = drift(&server, &repo_id).await.unwrap_err();
    assert!(none_yet.contains("nothing to compare"), "{none_yet}");

    // Nothing changed: the cached index is reused.
    let again = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(again.contains("(cached)"), "{again}");

    // An uncommitted edit: the core cache alone would still reuse the
    // index (HEAD did not move); the freshness line calls it stale ...
    std::fs::write(
        project.join("src/app.ts"),
        "export function start(): number {\n    return 2;\n}\nexport function later() {}\n",
    )
    .unwrap();
    // (result lines reuse git state for up to 3 s)
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    let line = index_line(&server, &repo_id).await;
    assert!(line.contains("1 file changed since indexing"), "{line}");
    // ... so analyze without force re-indexes, and the line clears.
    let refreshed = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(refreshed.contains("(fresh)"), "{refreshed}");
    let line = index_line(&server, &repo_id).await;
    assert!(line.ends_with("= HEAD, up to date."), "{line}");
    // The drift since the first analysis is the new function, nothing else
    // (symbol uids differ between analyses; snapshot keys do not).
    let changes = drift(&server, &repo_id).await.unwrap();
    assert!(
        changes.contains("+1 -0 symbols")
            && changes.contains("+ symbol Function  later (src/app.ts)"),
        "{changes}"
    );

    // A new commit moves HEAD: stale, and analyze re-indexes again.
    git(&project, &["commit", "-q", "-am", "edit"]);
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    let line = index_line(&server, &repo_id).await;
    assert!(line.contains("stale; run analyze"), "{line}");
    let refreshed = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(refreshed.contains("(fresh)"), "{refreshed}");
    assert!(index_line(&server, &repo_id)
        .await
        .ends_with("= HEAD, up to date."));

    // An uncommitted deletion stays in `git status` until it is committed.
    // It is a change while the index still holds the file, and no longer one
    // once analyze has re-indexed without it — the line must clear, not
    // call the index stale forever.
    std::fs::remove_file(project.join("src/extra.ts")).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    let line = index_line(&server, &repo_id).await;
    assert!(line.contains("1 file changed since indexing"), "{line}");
    let refreshed = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(refreshed.contains("(fresh)"), "{refreshed}");
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    let line = index_line(&server, &repo_id).await;
    assert!(line.ends_with("= HEAD, up to date."), "{line}");
    let again = server.analyze_for_test(analyze(&project)).await.unwrap();
    assert!(again.contains("(cached)"), "{again}");

    // ── Same-named top-level functions are ambiguous ─────────────────────
    let project = projects.path().join("twin-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/a.ts"),
        "export function parse(input: string): number {\n    return input.length;\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/b.ts"),
        "export function parse(text: string): string {\n    return text.trim();\n}\n",
    )
    .unwrap();
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);
    server.analyze_for_test(analyze(&project)).await.unwrap();

    let context = |name: &str| SymbolContextParams {
        symbol_name: name.to_string(),
        repo_id: repo_id.clone(),
    };
    let err = server
        .symbol_context_for_test(context("parse"))
        .await
        .expect_err("two top-level `parse` functions are ambiguous");
    assert!(err.contains("is ambiguous: 2 matches"), "{err}");
    assert!(
        err.contains(
            "Next: pass one of these as the symbol name: `src/a.ts:parse`, `src/b.ts:parse`"
        ),
        "{err}"
    );

    let picked = server
        .symbol_context_for_test(context("src/b.ts:parse"))
        .await
        .expect("the listed handle resolves");
    assert!(picked.contains("File:   src/b.ts:1-3"), "{picked}");
    assert!(picked.contains("text.trim()"), "{picked}");

    let rename = |name: &str| RenameParams {
        symbol_name: name.to_string(),
        new_name: "parseText".to_string(),
        repo_id: repo_id.clone(),
        ..Default::default()
    };
    let err = server
        .rename_symbol_for_test(rename("parse"))
        .await
        .expect_err("rename must not guess between the two");
    assert!(err.contains("is ambiguous"), "{err}");
    let plan = server
        .rename_symbol_for_test(rename("src/b.ts:parse"))
        .await
        .expect("rename of the picked symbol");
    assert!(plan.contains("src/b.ts:1"), "{plan}");
    assert!(!plan.contains("src/a.ts"), "only b.ts is renamed: {plan}");
}
