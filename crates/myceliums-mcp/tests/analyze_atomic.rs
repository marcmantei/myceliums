//! `analyze` rebuilds atomically and one at a time (#98):
//!
//! * the new index is built beside the live one and swapped in only when
//!   complete — a run that is cancelled or crashes mid-analysis leaves the
//!   previous index answering queries, and no staging directory behind;
//! * while an analysis runs, the old index stays queryable, and a second
//!   `analyze` of the same repository is an `isError` result.
//!
//! This binary owns its data directory, and `MYCELIUMS_DATA_DIR` is
//! process-wide, so the tests share one directory and take turns.

use myceliums_core::{AnalysisPhase, ProgressReporter};
use myceliums_mcp::{AnalyzeParams, DeleteParams, GetCallersParams, MyceliumsMcp, SearchParams};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

fn data_dir() -> &'static Path {
    static DATA: OnceLock<tempfile::TempDir> = OnceLock::new();
    DATA.get_or_init(|| {
        let data = tempfile::tempdir().unwrap();
        std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
        data
    })
    .path()
}

/// A server reading the test data directory (the data directory is fixed
/// when the server is created).
fn new_server() -> MyceliumsMcp {
    data_dir();
    MyceliumsMcp::new()
}

/// One test at a time: they share the registry file.
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

/// A fresh copy of the TypeScript fixture, analyzed once.
async fn analyzed_project(
    server: &MyceliumsMcp,
    name: &str,
) -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join(name);
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-ts-project");
    copy_dir(&fixture, &project);
    let project = project.canonicalize().unwrap();
    server
        .analyze_for_test(params(&project))
        .await
        .expect("first analyze");
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);
    (dir, project, repo_id)
}

fn params(project: &Path) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(true),
        skip_embeddings: Some(true),
        ..Default::default()
    }
}

/// Adds a function the current index does not know.
fn add_probe(project: &Path) {
    let utils = project.join("src/utils.ts");
    let mut source = std::fs::read_to_string(&utils).unwrap();
    source.push_str("\nexport function zebraSwapProbe(): number {\n    return 42;\n}\n");
    std::fs::write(&utils, source).unwrap();
}

async fn finds(server: &MyceliumsMcp, repo_id: &str, query: &str) -> bool {
    let out = server
        .context_search_for_test(SearchParams {
            query: query.to_string(),
            repo_id: Some(repo_id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("search must keep working: {e}"));
    !out.contains("No results")
}

/// The old index answers: the probe is unknown, the fixture's call graph
/// is intact.
async fn assert_previous_index(server: &MyceliumsMcp, repo_id: &str) {
    assert!(!finds(server, repo_id, "zebraSwapProbe").await);
    assert!(finds(server, repo_id, "formatName").await);
    let callers = server
        .get_callers_for_test(GetCallersParams {
            symbol_name: "parseBody".to_string(),
            repo_id: repo_id.to_string(),
            ..Default::default()
        })
        .await
        .expect("callers from the previous index");
    assert!(callers.contains("processRequest"), "{callers}");
}

/// What rebuilds left in `data/.staging/`.
fn leftovers() -> Vec<String> {
    std::fs::read_dir(data_dir().join("data").join(".staging"))
        .into_iter()
        .flatten()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// Calls `on` for the first report of `phase` (by name).
struct OnPhase<F: Fn() + Send + Sync> {
    phase: &'static str,
    on: F,
    fired: Mutex<bool>,
}

impl<F: Fn() + Send + Sync> ProgressReporter for OnPhase<F> {
    fn report(&self, phase: AnalysisPhase) {
        let name = format!("{phase:?}");
        if name.starts_with(self.phase)
            && !std::mem::replace(&mut *self.fired.lock().unwrap(), true)
        {
            (self.on)();
        }
    }
}

fn on_phase(
    phase: &'static str,
    on: impl Fn() + Send + Sync + 'static,
) -> Option<Arc<dyn ProgressReporter>> {
    Some(Arc::new(OnPhase {
        phase,
        on,
        fired: Mutex::new(false),
    }))
}

#[tokio::test]
async fn a_cancelled_analyze_keeps_the_previous_index() {
    let _turn = serial().await;
    let server = new_server();
    let (_dir, project, repo_id) = analyzed_project(&server, "cancel-project").await;
    add_probe(&project);

    // Cancel mid-analysis: while the analyzer resolves calls.
    let ct = CancellationToken::new();
    let cancel = ct.clone();
    let err = server
        .analyze_observed_for_test(
            params(&project),
            on_phase("BuildingRelationships", move || cancel.cancel()),
            ct,
        )
        .await
        .expect_err("a cancelled analyze fails");
    assert!(err.contains("cancelled"), "{err}");
    assert!(err.contains("previous index is unchanged"), "{err}");
    assert_previous_index(&server, &repo_id).await;
    assert_eq!(leftovers(), Vec::<String>::new(), "staging dir removed");

    // The next analyze swaps the new index in.
    server.analyze_for_test(params(&project)).await.unwrap();
    assert!(finds(&server, &repo_id, "zebraSwapProbe").await);
    assert!(leftovers().is_empty());
}

#[tokio::test]
async fn a_crash_mid_analyze_keeps_the_previous_index_and_releases_the_lock() {
    let _turn = serial().await;
    let server = new_server();
    let (_dir, project, repo_id) = analyzed_project(&server, "crash-project").await;
    add_probe(&project);

    // Injected failure: the build panics half-way through the analyzer.
    let err = server
        .analyze_observed_for_test(
            params(&project),
            on_phase("BuildingRelationships", || {
                panic!("injected failure mid-analyze")
            }),
            CancellationToken::new(),
        )
        .await
        .expect_err("a crashed analyze fails");
    assert!(err.contains("analysis task failed"), "{err}");
    assert_previous_index(&server, &repo_id).await;
    assert!(leftovers().is_empty(), "{:?}", leftovers());
    let lock = data_dir()
        .join("data")
        .join(".locks")
        .join(format!("{repo_id}.lock"));
    assert!(!lock.exists(), "the lock is released");

    server
        .analyze_for_test(params(&project))
        .await
        .expect("the repository can be analyzed again");
    assert!(finds(&server, &repo_id, "zebraSwapProbe").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_analyze_of_a_running_repo_is_refused_while_queries_keep_working() {
    let _turn = serial().await;
    let server = Arc::new(new_server());
    let (_dir, project, repo_id) = analyzed_project(&server, "busy-project").await;
    add_probe(&project);

    // Hold the first analysis mid-way until the checks below are done.
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (started_tx, release_rx) = (Mutex::new(started_tx), Mutex::new(release_rx));
    let hold = on_phase("BuildingRelationships", move || {
        started_tx.lock().unwrap().send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
    });
    let first = tokio::spawn({
        let (server, params) = (server.clone(), params(&project));
        async move {
            server
                .analyze_observed_for_test(params, hold, CancellationToken::new())
                .await
        }
    });
    tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
        .await
        .unwrap();

    let second = server
        .analyze_for_test(params(&project))
        .await
        .expect_err("a second analyze is refused");
    assert!(second.contains("Analysis already running"), "{second}");
    assert!(second.contains("Next:"), "{second}");
    let delete = server
        .delete_for_test(DeleteParams {
            repo_id: repo_id.clone(),
            confirm: true,
        })
        .await
        .expect_err("no delete during an analysis");
    assert!(delete.contains("an analysis of it is running"), "{delete}");
    // The runtime is free and the old index answers during the analysis.
    assert_previous_index(&server, &repo_id).await;

    release_tx.send(()).unwrap();
    first.await.unwrap().expect("the first analyze completes");
    assert!(finds(&server, &repo_id, "zebraSwapProbe").await);
    assert!(leftovers().is_empty());
    // And the repository is free again.
    server.analyze_for_test(params(&project)).await.unwrap();
}

/// Records the phases it sees and cancels `ct` at the first `Discovering`.
struct CancelAtStart {
    ct: CancellationToken,
    seen: Mutex<Vec<String>>,
}

impl ProgressReporter for CancelAtStart {
    fn report(&self, phase: AnalysisPhase) {
        if matches!(phase, AnalysisPhase::Discovering) {
            self.ct.cancel();
        }
        self.seen.lock().unwrap().push(format!("{phase:?}"));
    }
}

#[tokio::test]
async fn cancelling_stops_the_analyzer_early_and_recovers_dead_leftovers() {
    let _turn = serial().await;
    let server = new_server();
    let (_dir, project, repo_id) = analyzed_project(&server, "early-stop-project").await;
    add_probe(&project);

    // A rebuild killed earlier (no Drop) by a process that no longer runs.
    let dead_build = data_dir()
        .join("data")
        .join(".staging")
        .join("build-99999999-1-0");
    std::fs::create_dir_all(dead_build.join("symbols.lance")).unwrap();

    let ct = CancellationToken::new();
    let observer = Arc::new(CancelAtStart {
        ct: ct.clone(),
        seen: Mutex::new(Vec::new()),
    });
    let err = server
        .analyze_observed_for_test(params(&project), Some(observer.clone()), ct)
        .await
        .expect_err("cancelled");
    assert!(err.contains("cancelled"), "{err}");
    let seen = observer.seen.lock().unwrap().clone();
    assert!(
        !seen.iter().any(|p| p.starts_with("BuildingRelationships")),
        "the analyzer stops before resolving calls: {seen:?}"
    );
    assert_previous_index(&server, &repo_id).await;
    assert!(
        !dead_build.exists(),
        "a dead process's staging dir is removed"
    );
    assert!(leftovers().is_empty(), "{:?}", leftovers());
}

/// A project folder named like a rebuild's leftover (`.build-123`, as if
/// PID 123 had left it) is an ordinary repository: its id is sanitized, and
/// later rebuilds — which recover dead leftovers — never touch its index.
#[tokio::test]
async fn a_project_named_like_a_leftover_survives_other_rebuilds() {
    let _turn = serial().await;
    let server = new_server();
    let (_dir, _project, repo_id) = analyzed_project(&server, ".build-123").await;
    assert!(repo_id.starts_with("_build-123-"), "{repo_id}");
    // (The analyzer skips hidden folders, so this index holds no symbols;
    // what matters is that its directory stays exactly as it is.)
    let index = data_dir().join("data").join(&repo_id);
    assert!(index.is_dir());
    std::fs::write(index.join("sentinel"), "live").unwrap();

    // Other rebuilds run the leftover recovery.
    let (_other_dir, other, _) = analyzed_project(&server, "neighbour-project").await;
    server.analyze_for_test(params(&other)).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(index.join("sentinel")).unwrap(),
        "live",
        "the index is untouched"
    );
    finds(&server, &repo_id, "anything").await;
}

/// #118: a registered repository whose index directory is gone is rebuilt
/// by `analyze`, not answered from the cache — reading the cached index
/// failed, and the error said to run `analyze`.
#[tokio::test]
async fn analyze_rebuilds_a_registered_repo_whose_index_is_missing() {
    let _turn = serial().await;
    let server = new_server();
    let (_dir, project, repo_id) = analyzed_project(&server, "lost-index").await;
    let index = data_dir().join("data").join(&repo_id);
    std::fs::remove_dir_all(&index).unwrap();

    let out = server
        .analyze_for_test(AnalyzeParams {
            force: Some(false),
            ..params(&project)
        })
        .await
        .expect("analyze rebuilds the index");
    assert!(out.contains("(fresh)"), "{out}");
    assert!(index.is_dir());
    assert!(finds(&server, &repo_id, "formatName").await);
}
