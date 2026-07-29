//! Shared harness for MCP integration tests.
//!
//! Builds a real, on-disk Myceliums index from the `sample-ts-project` fixture
//! inside a throwaway data directory, then points the MCP server at it via the
//! `MYCELIUMS_DATA_DIR` override. Analysis runs with embeddings disabled so the
//! index build is deterministic and offline (no model downloads).
//!
//! The index is built exactly once per test binary and shared across tests.
//! Initialization is guarded by a [`tokio::sync::OnceCell`], so every test —
//! even under the default multi-threaded test runner — awaits the *same*
//! initialization and observes a fully-populated data directory. The env
//! overrides are therefore written exactly once, before any handler runs.
//!
//! Not every test binary uses every helper (each `tests/*.rs` compiles this
//! module independently), so a few items look unused to any single binary.
//! Rather than blanket-silencing the whole module — which would also hide a
//! genuinely dead helper — the affected items carry a targeted
//! `#[allow(dead_code)]` individually.

use myceliums_core::Analyzer;
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::sync::OnceCell;

/// Stable repo id used by the whole suite.
#[allow(dead_code)] // used by mcp_handlers.rs, not by pipeline.rs
pub const REPO_ID: &str = "sample-ts-project";

/// Named symbols from the `sample-ts-project` fixture, referenced by name in
/// assertions across the suite. Extracted here so a fixture change updates one
/// place instead of many inline string literals.
///
/// Grouped by role for discoverability; not every binary uses every constant,
/// hence the module-level module `allow` below.
#[allow(dead_code)] // subset used per test binary
pub mod fixture {
    // Classes / interfaces / type aliases.
    pub const USER_SERVICE: &str = "UserService";
    pub const DATABASE: &str = "Database";
    pub const USER: &str = "User";
    pub const CONFIG: &str = "Config";

    // Functions / methods.
    pub const GET_USER: &str = "getUser";
    pub const FETCH_USER: &str = "fetchUser";
    pub const FORMAT_NAME: &str = "formatName";
    pub const CREATE_USER: &str = "createUser";
    pub const FIND_BY_ID: &str = "findById";

    /// File path the user service lives in, relative to the fixture root.
    pub const USER_SERVICE_FILE: &str = "src/services/user.ts";
}

/// Absolute path to the checked-in TypeScript fixture project.
#[allow(dead_code)] // used directly by pipeline.rs; reached transitively elsewhere
pub fn fixture_repo_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sample-ts-project")
        .canonicalize()
        .expect("sample-ts-project fixture must exist")
}

/// The initialized data directory; also keeps the `TempDir` alive for the
/// lifetime of the process so the on-disk index is not deleted mid-test.
#[allow(dead_code)] // driven by ensure_indexed(); unused in pipeline.rs binary
static DATA_DIR: OnceCell<TempDir> = OnceCell::const_new();

/// Build the shared index once and configure `MYCELIUMS_DATA_DIR`.
///
/// Returns the data-directory root. Safe (and cheap) to call from every test:
/// all callers await the single initialization.
///
/// ## Isolation guarantee
///
/// The index is **read-only** for the lifetime of the test binary. Every MCP
/// handler exercised through this harness (`context_search`, `detect_impact`,
/// `rename_symbol`, `cypher_query`) only *queries* the graph — `rename_symbol`
/// returns a plan without applying it, and `cypher_query` rejects write
/// clauses. No handler mutates the on-disk index or the shared `TempDir`, so
/// tests observe identical state regardless of execution order. This is why a
/// single shared index is safe here, whereas `pipeline.rs` — which needs a
/// pristine store to assert exact entity counts — deliberately builds its own
/// fresh `tempfile::tempdir()` per invocation. If a future test needs to
/// *mutate* the index, it must not use this shared harness; it should build an
/// isolated store the way `pipeline.rs` does.
#[allow(dead_code)] // used by mcp_handlers.rs, not by pipeline.rs
pub async fn ensure_indexed() -> PathBuf {
    let dir = DATA_DIR
        .get_or_init(|| async {
            let tmp = tempfile::tempdir().expect("create temp data dir");
            let data_root = tmp.path().to_path_buf();

            // Point the server at this directory.
            std::env::set_var("MYCELIUMS_DATA_DIR", &data_root);

            // Index the fixture into <data_root>/data/<repo_id> with embeddings
            // disabled — fully offline and deterministic.
            let db_path = RepoRegistry::repo_db_path(&data_root, REPO_ID);
            let store = Store::open(&db_path, REPO_ID).await.expect("open store");
            let analyzer = Analyzer::new(store, fixture_repo_path()).set_skip_embeddings(true);
            let result = analyzer.analyze().await.expect("analyze fixture");

            // Register the repo so handlers resolving by id / most-recent work.
            let registry_path = data_root.join("repos.json");
            let mut registry = RepoRegistry::load(&registry_path).expect("load registry");
            registry.register(RepoInfo {
                id: REPO_ID.to_string(),
                name: REPO_ID.to_string(),
                path: fixture_repo_path().to_string_lossy().to_string(),
                analyzed_at: "2026-01-01T00:00:00Z".to_string(),
                symbol_count: result.symbol_count as u32,
                file_count: result.file_count as u32,
                analyzed_commit: None,
                vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
            });
            registry.save().expect("save registry");

            tmp
        })
        .await;
    dir.path().to_path_buf()
}
