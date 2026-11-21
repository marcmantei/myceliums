//! `analyze` answers once the graph is built and embeds in the background
//! (#157): the call does not wait for the model, an interrupted embedding is
//! resumed by the next `analyze`, and a changed embedder re-analyzes a fresh
//! index instead of reporting it as cached.
//!
//! The model is a fake OpenAI-compatible endpoint on localhost, so nothing is
//! downloaded. This binary owns its data directory (`MYCELIUMS_DATA_DIR` is
//! process-wide), so it has one test.

use myceliums_core::config::CONFIG_FILENAME;
use myceliums_mcp::{AnalyzeParams, MyceliumsMcp};
use myceliums_storage::{RepoRegistry, Store};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DIM: usize = 8;
const WAIT: Duration = Duration::from_secs(120);

/// The fake model: how many texts it embedded, and a switch that makes it
/// answer every request with a server error.
struct FakeModel {
    base_url: String,
    texts: Arc<AtomicUsize>,
    failing: Arc<AtomicBool>,
}

fn vector_for(text: &str) -> Vec<f32> {
    let mut v = vec![1.0f32; DIM];
    for (i, b) in text.bytes().enumerate() {
        v[i % DIM] += b as f32;
    }
    v
}

async fn fake_model() -> FakeModel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let texts = Arc::new(AtomicUsize::new(0));
    let failing = Arc::new(AtomicBool::new(false));
    let (count, fail) = (texts.clone(), failing.clone());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (count, fail) = (count.clone(), fail.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (body_at, length) = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                        let length: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while buf.len() < body_at + length {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let (status, body) = if fail.load(Ordering::SeqCst) {
                    ("500 Internal Server Error", b"{}".to_vec())
                } else {
                    let request: serde_json::Value =
                        serde_json::from_slice(&buf[body_at..body_at + length]).unwrap();
                    let inputs = request["input"].as_array().cloned().unwrap_or_default();
                    count.fetch_add(inputs.len(), Ordering::SeqCst);
                    let data: Vec<serde_json::Value> = inputs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            serde_json::json!({
                                "index": i,
                                "embedding": vector_for(t.as_str().unwrap_or_default()),
                            })
                        })
                        .collect();
                    (
                        "200 OK",
                        serde_json::to_vec(&serde_json::json!({ "data": data })).unwrap(),
                    )
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    FakeModel {
        base_url,
        texts,
        failing,
    }
}

fn configure(project: &Path, base_url: &str, model: &str) {
    std::fs::write(
        project.join(CONFIG_FILENAME),
        format!(
            "[embedding]\nprovider = \"openai-compatible\"\nmodel = \"{model}\"\n\
             base_url = \"{base_url}\"\ndim = {DIM}\n"
        ),
    )
    .unwrap();
}

fn analyze(project: &Path, force: bool) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(force),
        ..Default::default()
    }
}

async fn missing_vectors(db: &Path, repo_id: &str) -> usize {
    let store = Store::open_existing(db, repo_id).await.unwrap();
    store
        .get_symbols_with_vectors()
        .await
        .unwrap()
        .into_iter()
        .filter(|(_, v)| v.is_none())
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_answers_before_the_model_and_the_embedding_resumes() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let server = MyceliumsMcp::new();
    let model = fake_model().await;

    // More symbols than one background slice.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("background-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    let functions: String = (0..300)
        .map(|i| format!("export function f{i}(): number {{\n    return {i};\n}}\n"))
        .collect();
    std::fs::write(project.join("src/a.ts"), functions).unwrap();
    configure(&project, &model.base_url, "fake-a");
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);
    let db = RepoRegistry::repo_db_path(data.path(), &repo_id);

    // The first index: the answer comes back before the model ran; the
    // background embedding then embeds every symbol.
    let out = server
        .analyze_for_test(analyze(&project, false))
        .await
        .unwrap();
    assert!(out.contains("Embeddings:  0/"), "{out}");
    assert!(out.contains("in the background with fake-a"), "{out}");
    let line = server
        .wait_for_embedding_for_test(&repo_id, WAIT)
        .await
        .expect("a job ran");
    assert!(
        line.contains("built in the background with fake-a"),
        "{line}"
    );
    assert_eq!(missing_vectors(&db, &repo_id).await, 0);
    let symbols = model.texts.load(Ordering::SeqCst);
    assert!(symbols >= 300, "{symbols} texts embedded");

    // Fresh and complete: cached, nothing embedded again.
    let out = server
        .analyze_for_test(analyze(&project, false))
        .await
        .unwrap();
    assert!(out.contains("(cached)"), "{out}");
    assert!(
        out.contains(&format!("Embeddings:  {symbols}/{symbols} symbols")),
        "{out}"
    );
    assert!(!out.contains("in the background"), "{out}");

    // Another embedder: a fresh index is re-analyzed, not reported as cached,
    // and re-embedded in the background; the model fails.
    configure(&project, &model.base_url, "fake-b");
    model.failing.store(true, Ordering::SeqCst);
    let out = server
        .analyze_for_test(analyze(&project, false))
        .await
        .unwrap();
    assert!(out.contains("(fresh)"), "{out}");
    assert!(out.contains("in the background with fake-b"), "{out}");
    let line = server
        .wait_for_embedding_for_test(&repo_id, WAIT)
        .await
        .expect("a job ran");
    assert!(line.contains("stopped after 0/"), "{line}");
    assert_eq!(missing_vectors(&db, &repo_id).await, symbols);

    // The model is back: the next analyze resumes the embedding of the
    // fresh index (no re-analysis), and the tools answer meanwhile.
    model.failing.store(false, Ordering::SeqCst);
    let before = model.texts.load(Ordering::SeqCst);
    let out = server
        .analyze_for_test(analyze(&project, false))
        .await
        .unwrap();
    assert!(out.contains("(cached)"), "{out}");
    assert!(out.contains("in the background with fake-b"), "{out}");
    server.wait_for_embedding_for_test(&repo_id, WAIT).await;
    assert_eq!(missing_vectors(&db, &repo_id).await, 0);
    assert_eq!(
        model.texts.load(Ordering::SeqCst) - before,
        symbols,
        "every symbol embedded once"
    );
    let stats = server.server_stats_for_test().await.unwrap();
    assert!(stats.contains("Background embedding:"), "{stats}");
}
