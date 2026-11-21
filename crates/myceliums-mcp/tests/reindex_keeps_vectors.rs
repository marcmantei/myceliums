//! A re-index through the `analyze` tool never drops existing embeddings
//! (#151): with `skip_embeddings`, and when the embedding model fails, the
//! symbols it did not change keep their vectors from the index it replaces,
//! and the answer says how many symbols are left without one.
//!
//! No model is downloaded: the vectors of the first index are written by the
//! test, under an `openai-compatible` model whose endpoint nothing listens on.
//!
//! This binary owns its data directory (`MYCELIUMS_DATA_DIR` is
//! process-wide), so it has one test.

use myceliums_core::config::CONFIG_FILENAME;
use myceliums_core::IndexEmbeddingMeta;
use myceliums_mcp::{AnalyzeParams, MyceliumsMcp};
use myceliums_storage::{RepoRegistry, Store};
use std::collections::HashMap;
use std::path::Path;

/// Each symbol's vector by name, read from the live index.
async fn vectors(db: &Path, repo_id: &str) -> HashMap<String, Option<Vec<f32>>> {
    let store = Store::open_existing(db, repo_id).await.unwrap();
    store
        .get_symbols_with_vectors()
        .await
        .unwrap()
        .into_iter()
        .map(|(s, v)| (s.name, v))
        .collect()
}

fn analyze(project: &Path, skip_embeddings: bool) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(true),
        skip_embeddings: Some(skip_embeddings),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_reindex_that_does_not_embed_keeps_the_previous_vectors() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let server = MyceliumsMcp::new();

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("vectors-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/a.ts"),
        "export function alpha(): number {\n    return 1;\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/b.ts"),
        "export function beta(): number {\n    return 2;\n}\n",
    )
    .unwrap();
    // Nothing listens on the discard port: the model fails at once.
    let config_path = project.join(CONFIG_FILENAME);
    std::fs::write(
        &config_path,
        format!(
            "[embedding]\nprovider = \"openai-compatible\"\nmodel = \"unreachable\"\n\
             base_url = \"http://127.0.0.1:9/v1\"\ndim = {}\n",
            myceliums_storage::schema::DEFAULT_EMBEDDING_DIM
        ),
    )
    .unwrap();
    let embedding = myceliums_core::ProjectConfig::load(&config_path)
        .unwrap()
        .embedding;
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);
    let db = RepoRegistry::repo_db_path(data.path(), &repo_id);

    // The first index, given a vector per symbol as an embedding run with
    // this model would have left it.
    server
        .analyze_for_test(analyze(&project, true))
        .await
        .expect("first analyze");
    let meta = IndexEmbeddingMeta::from_config(&embedding).unwrap();
    {
        let store = Store::open_existing(&db, &repo_id).await.unwrap();
        let symbols = store.get_symbols().await.unwrap();
        let pairs = symbols
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut v = vec![0.0f32; meta.dim];
                v[i % meta.dim] = 1.0;
                (s.uid.clone(), v)
            })
            .collect();
        store.store_embeddings(pairs).await.unwrap();
        store
            .set_index_meta(
                IndexEmbeddingMeta::META_KEY,
                &serde_json::to_string(&meta).unwrap(),
            )
            .await
            .unwrap();
    }
    let before = vectors(&db, &repo_id).await;
    let total = before.len();
    assert!(before.values().all(Option::is_some), "{before:?}");

    // A new function, re-indexed with skip_embeddings: every old symbol
    // keeps its vector, the new one has none — and the answer says so.
    std::fs::write(
        project.join("src/b.ts"),
        "export function beta(): number {\n    return 2;\n}\n\
         export function gamma(): number {\n    return 3;\n}\n",
    )
    .unwrap();
    let out = server
        .analyze_for_test(analyze(&project, true))
        .await
        .expect("skip_embeddings re-index");
    assert!(
        out.contains(&format!(
            "Embeddings:  {total}/{} symbols ({total} kept from the previous index)",
            total + 1
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "⚠ 1 of {} symbols have no vector (embeddings skipped)",
            total + 1
        )),
        "{out}"
    );
    let after = vectors(&db, &repo_id).await;
    for (name, vector) in &before {
        assert_eq!(&after[name], vector, "{name}");
    }
    assert_eq!(after["gamma"], None);

    // `alpha` changes and the model fails: the others keep their vectors,
    // `alpha` and `gamma` are left to the background embedding (#157), which
    // fails — nothing else is lost.
    std::fs::write(
        project.join("src/a.ts"),
        "export function alpha(): number {\n    return 10;\n}\n",
    )
    .unwrap();
    let out = server
        .analyze_for_test(analyze(&project, false))
        .await
        .expect("the model runs after the answer");
    assert!(
        out.contains(&format!("Embeddings:  {}/{} symbols", total - 1, total + 1)),
        "{out}"
    );
    assert!(
        out.contains("Embedding:   2 symbols in the background with unreachable"),
        "{out}"
    );
    let line = server
        .wait_for_embedding_for_test(&repo_id, std::time::Duration::from_secs(60))
        .await
        .expect("a background embedding ran");
    assert!(line.contains("stopped after 0/2 symbols"), "{line}");
    let failed = vectors(&db, &repo_id).await;
    assert_eq!(failed["alpha"], None);
    assert_eq!(failed["gamma"], None);
    assert_eq!(failed["beta"], before["beta"]);
    let stats = server.server_stats_for_test().await.unwrap();
    assert!(stats.contains("Background embedding:"), "{stats}");
}
