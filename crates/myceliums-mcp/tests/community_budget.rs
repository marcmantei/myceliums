//! Community detection can be skipped and has a budget (#172): the analyze
//! summary and the community tools say when communities are partial or
//! skipped, and the core tools work without them.
//!
//! This binary owns its data directory, and `MYCELIUMS_DATA_DIR` is
//! process-wide, so the tests share one directory and take turns.

use myceliums_mcp::{
    AnalyzeParams, GetCallersParams, GetCommunitiesParams, MyceliumsMcp, SearchParams,
};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn data_dir() -> &'static Path {
    static DATA: OnceLock<tempfile::TempDir> = OnceLock::new();
    DATA.get_or_init(|| {
        let data = tempfile::tempdir().unwrap();
        std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
        data
    })
    .path()
}

async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A fresh copy of the TypeScript fixture, with `config` as its
/// `.myceliums.toml` when given.
fn project(dir: &tempfile::TempDir, name: &str, config: Option<&str>) -> PathBuf {
    let project = dir.path().join(name);
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-ts-project");
    copy_dir(&fixture, &project);
    if let Some(config) = config {
        std::fs::write(project.join(".myceliums.toml"), config).unwrap();
    }
    project.canonicalize().unwrap()
}

fn params(project: &Path, skip_communities: bool) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(true),
        skip_embeddings: Some(true),
        skip_communities: Some(skip_communities),
        ..Default::default()
    }
}

async fn communities(server: &MyceliumsMcp, repo_id: &str) -> serde_json::Value {
    let out = server
        .get_communities_for_test(GetCommunitiesParams {
            repo_id: repo_id.to_string(),
            ..Default::default()
        })
        .await
        .expect("get_communities");
    serde_json::from_str(&out).unwrap()
}

/// The core tools answer from an index without communities.
async fn assert_core_tools_work(server: &MyceliumsMcp, repo_id: &str) {
    let callers = server
        .get_callers_for_test(GetCallersParams {
            symbol_name: "parseBody".to_string(),
            repo_id: repo_id.to_string(),
            ..Default::default()
        })
        .await
        .expect("callers");
    assert!(callers.contains("processRequest"), "{callers}");
    let found = server
        .context_search_for_test(SearchParams {
            query: "formatName".to_string(),
            repo_id: Some(repo_id.to_string()),
            ..Default::default()
        })
        .await
        .expect("search");
    assert!(!found.contains("No results"), "{found}");
}

#[tokio::test]
async fn skip_communities_skips_detection_and_says_so() {
    let _turn = serial().await;
    data_dir();
    let server = MyceliumsMcp::new();
    let dir = tempfile::tempdir().unwrap();
    let project = project(&dir, "skipped", None);
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let summary = server
        .analyze_for_test(params(&project, true))
        .await
        .expect("analyze");
    assert!(summary.contains("Communities: 0 (skipped)"), "{summary}");
    assert!(
        summary.contains("Communities were not detected (`skip_communities`)"),
        "{summary}"
    );
    assert!(summary.contains("\nPhases:      discover "), "{summary}");
    assert!(summary.contains("communities "), "{summary}");

    let listed = communities(&server, &repo_id).await;
    assert_eq!(listed["total_count"], 0);
    let notice = listed["communities_notice"].as_str().expect("notice");
    assert!(notice.contains("not detected"), "{notice}");

    assert_core_tools_work(&server, &repo_id).await;

    // Without the switch, the next analysis detects them again.
    let summary = server
        .analyze_for_test(params(&project, false))
        .await
        .expect("analyze");
    assert!(!summary.contains("(skipped)"), "{summary}");
    let listed = communities(&server, &repo_id).await;
    assert!(listed["total_count"].as_u64().unwrap() > 0, "{listed}");
    assert!(listed.get("communities_notice").is_none(), "{listed}");
}

#[tokio::test]
async fn a_disabled_config_skips_detection() {
    let _turn = serial().await;
    data_dir();
    let server = MyceliumsMcp::new();
    let dir = tempfile::tempdir().unwrap();
    let project = project(&dir, "disabled", Some("[community]\nenabled = false\n"));
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let summary = server
        .analyze_for_test(params(&project, false))
        .await
        .expect("analyze");
    assert!(summary.contains("Communities: 0 (skipped)"), "{summary}");
    assert!(
        summary.contains("`[community] enabled = false`"),
        "{summary}"
    );
    assert_core_tools_work(&server, &repo_id).await;
}

/// An exhausted budget (here: the iteration cap, which a real analysis hits
/// deterministically) keeps the partition and marks it partial everywhere.
#[tokio::test]
async fn an_exhausted_budget_is_surfaced_as_partial() {
    let _turn = serial().await;
    data_dir();
    let server = MyceliumsMcp::new();
    let dir = tempfile::tempdir().unwrap();
    let project = project(&dir, "partial", Some("[community]\nmax_iterations = 1\n"));
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let summary = server
        .analyze_for_test(params(&project, false))
        .await
        .expect("analyze");
    assert!(summary.contains(" (partial)\n"), "{summary}");
    assert!(summary.contains("⚠ Communities are partial"), "{summary}");
    assert!(summary.contains("max_iterations"), "{summary}");

    let listed = communities(&server, &repo_id).await;
    assert!(listed["total_count"].as_u64().unwrap() > 0, "{listed}");
    let notice = listed["communities_notice"].as_str().expect("notice");
    assert!(notice.starts_with("Communities are partial"), "{notice}");

    // A cached analyze reports the recorded state too.
    let cached = server
        .analyze_for_test(AnalyzeParams {
            force: Some(false),
            ..params(&project, false)
        })
        .await
        .expect("cached analyze");
    if cached.contains("(cached)") {
        assert!(cached.contains(" (partial)\n"), "{cached}");
    }
}
