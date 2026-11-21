//! Fixtures shared by the handler and router tests.

use crate::routes::build_router;
use crate::state::AppState;
use axum::http::{HeaderMap, Request, StatusCode};
use myceliums_storage::{CodeSymbol, RepoRegistry, Store, SymbolKind};
use serde_json::Value;
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

/// Registers `repo_id` in the registry `app_state` points at, as
/// `analyze` would; unregistered ids are rejected by the graph service.
pub(crate) fn register_repo(data_dir: &std::path::Path, repo_id: &str) {
    register_repo_with(
        data_dir,
        repo_id,
        myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
    );
}

/// [`register_repo`] with the given test-code classification version (0:
/// an index from before #146).
pub(crate) fn register_repo_with(
    data_dir: &std::path::Path,
    repo_id: &str,
    classification_version: u32,
) {
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
        classification_version,
    });
    registry.save().unwrap();
}

/// Seed a real store with one embedded symbol so `graph_v1_node` reaches the
/// semantic-neighbor vector search on a healthy backend.
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
    // Embedding metadata, so semantic search gets past its "no embeddings"
    // check to a read that fails.
    Store::open(&db_path, repo_id)
        .await
        .unwrap()
        .set_index_meta(myceliums_core::IndexEmbeddingMeta::META_KEY, "{}")
        .await
        .unwrap();
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

/// A symbol the index classified as test code (#146).
pub(crate) fn test_symbol(uid: &str, repo_id: &str) -> CodeSymbol {
    CodeSymbol {
        file_path: "tests/lib_test.rs".to_string(),
        metadata: Some(r#"{"test":true}"#.to_string()),
        ..sample_symbol(uid, repo_id)
    }
}

/// Registers `repo_id` and stores `symbols` with the given vectors (`None`:
/// no embedding) and `relationships`.
pub(crate) async fn seed_store(
    data_dir: &std::path::Path,
    repo_id: &str,
    symbols: &[(CodeSymbol, Option<Vec<f32>>)],
    relationships: &[myceliums_storage::Relationship],
) {
    register_repo(data_dir, repo_id);
    let db_path = RepoRegistry::repo_db_path(data_dir, repo_id);
    std::fs::create_dir_all(&db_path).unwrap();
    let store = Store::open(&db_path, repo_id).await.unwrap();
    let plain: Vec<CodeSymbol> = symbols.iter().map(|(s, _)| s.clone()).collect();
    store.store_symbols(&plain).await.unwrap();
    let vectors: Vec<(String, Vec<f32>)> = symbols
        .iter()
        .filter_map(|(s, v)| v.clone().map(|v| (s.uid.clone(), v)))
        .collect();
    if !vectors.is_empty() {
        store.store_embeddings(vectors).await.unwrap();
    }
    if !relationships.is_empty() {
        store.store_relationships(relationships).await.unwrap();
    }
}

/// A unit-length-ish vector of the default dimension: `1` on axis 0 and
/// `tilt` on axis 1, so a larger `tilt` is further from axis 0.
pub(crate) fn tilted_vector(tilt: f32) -> Vec<f32> {
    let dim = myceliums_storage::schema::DEFAULT_EMBEDDING_DIM as usize;
    let mut v = vec![0.0f32; dim];
    v[0] = 1.0;
    v[1] = tilt;
    v
}

/// Sends one request through the full router (with its loopback guard) and
/// returns the status, headers and JSON body (`Value::Null` when the body
/// is not JSON).
pub(crate) async fn request(
    state: Arc<AppState>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    use tower::ServiceExt;
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8888");
    let body = match body {
        Some(json) => {
            req = req.header("content-type", "application/json");
            axum::body::Body::from(json.to_string())
        }
        None => axum::body::Body::empty(),
    };
    let resp = build_router(state)
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Asserts `body` is exactly the error contract: `{"error": ..., "next": ...}`
/// with two non-empty strings. Returns them.
pub(crate) fn error_body(body: &Value) -> (String, String) {
    let obj = body
        .as_object()
        .unwrap_or_else(|| panic!("not a JSON object: {body}"));
    assert_eq!(obj.len(), 2, "only error and next: {body}");
    let field = |k: &str| {
        let v = obj[k]
            .as_str()
            .unwrap_or_else(|| panic!("{k} is not a string: {body}"));
        assert!(!v.is_empty(), "{k} is empty: {body}");
        v.to_string()
    };
    (field("error"), field("next"))
}
