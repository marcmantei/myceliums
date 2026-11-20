//! Fixtures shared by the handler and router tests.

use crate::state::AppState;
use myceliums_storage::{CodeSymbol, RepoRegistry, Store, SymbolKind};
use std::path::PathBuf;
use std::sync::Arc;

/// A unique temp directory for an isolated test store.
pub(crate) fn test_data_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("myceliums_http_test_{}", name))
}

pub(crate) fn app_state(data_dir: PathBuf) -> Arc<AppState> {
    Arc::new(AppState::new(data_dir, None))
}

pub(crate) fn sample_symbol(uid: &str, repo_id: &str) -> CodeSymbol {
    CodeSymbol {
        uid: uid.to_string(),
        name: uid.to_string(),
        qualified_name: uid.to_string(),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".to_string(),
        start_line: 1,
        end_line: 2,
        signature: format!("fn {}()", uid),
        content: "body".to_string(),
        repo_id: repo_id.to_string(),
        metadata: None,
    }
}

/// Seed a real store with one embedded symbol so `graph_v1_node` reaches the
/// semantic-neighbor vector search on a healthy backend.
/// Registers `repo_id` in the registry `app_state` points at, as
/// `analyze` would; unregistered ids are rejected by the graph service.
pub(crate) fn register_repo(data_dir: &std::path::Path, repo_id: &str) {
    let mut registry = RepoRegistry::load(&data_dir.join("repos.json")).unwrap();
    registry.register(myceliums_storage::RepoInfo {
        id: repo_id.to_string(),
        name: repo_id.to_string(),
        path: format!("/nonexistent/{repo_id}"),
        analyzed_at: String::new(),
        symbol_count: 1,
        file_count: 1,
        analyzed_commit: None,
        vector_geometry_version: 0,
        classification_version: 0,
    });
    registry.save().unwrap();
}

pub(crate) async fn seed_healthy_store(data_dir: &std::path::Path, repo_id: &str, node_id: &str) {
    register_repo(data_dir, repo_id);
    let db_path = RepoRegistry::repo_db_path(data_dir, repo_id);
    std::fs::create_dir_all(&db_path).unwrap();
    let store = Store::open(&db_path, repo_id).await.unwrap();
    store
        .store_symbols(&[sample_symbol(node_id, repo_id)])
        .await
        .unwrap();
    let dim = myceliums_storage::schema::DEFAULT_EMBEDDING_DIM as usize;
    store
        .store_embeddings(vec![(node_id.to_string(), vec![0.5f32; dim])])
        .await
        .unwrap();
}

/// Seed a valid store, then overwrite its on-disk table files with garbage.
/// `Store::open` (a lazy `connect`) still succeeds, but the first table read
/// — `get_symbols`, `get_symbols_with_vectors`, or `vector_search` — then
/// returns `Err`, giving us a deterministic backend failure to assert the
/// 5xx mapping against (verified reachable: `connect` ok, reads error).
pub(crate) async fn corrupt_seeded_store(data_dir: &std::path::Path, repo_id: &str, node_id: &str) {
    seed_healthy_store(data_dir, repo_id, node_id).await;
    let db_path = RepoRegistry::repo_db_path(data_dir, repo_id);
    fn corrupt(p: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    corrupt(&path);
                } else {
                    let _ = std::fs::write(&path, b"CORRUPT");
                }
            }
        }
    }
    corrupt(&db_path);
}
