//! Search and impact analysis keep working while the background embedding
//! writes the live index (#157): the job stages its vectors in tables the
//! graph is not read from, so no call fails with "the index was replaced",
//! the cached graph is not thrown away on every write, and semantic search
//! only ever returns symbols that have a vector.
//!
//! The model is a fake OpenAI-compatible endpoint on localhost that answers
//! each request after a short pause. This binary owns its data directory
//! (`MYCELIUMS_DATA_DIR` is process-wide), so it has one test.

use myceliums_core::config::CONFIG_FILENAME;
use myceliums_mcp::{
    AnalyzeParams, ImpactOfChangeParams, MyceliumsMcp, SearchCodeParams, SearchMode,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DIM: usize = 8;

/// The fake model; how many texts it embedded.
async fn slow_model(pause: Duration) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let texts = Arc::new(AtomicUsize::new(0));
    let count = texts.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let count = count.clone();
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
                tokio::time::sleep(pause).await;
                let request: serde_json::Value =
                    serde_json::from_slice(&buf[body_at..body_at + length]).unwrap();
                let inputs = request["input"].as_array().cloned().unwrap_or_default();
                count.fetch_add(inputs.len(), Ordering::SeqCst);
                let data: Vec<serde_json::Value> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let mut v = vec![1.0f32; DIM];
                        for (j, b) in t.as_str().unwrap_or_default().bytes().enumerate() {
                            v[j % DIM] += b as f32;
                        }
                        serde_json::json!({ "index": i, "embedding": v })
                    })
                    .collect();
                let body = serde_json::to_vec(&serde_json::json!({ "data": data })).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    (base_url, texts)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_and_impact_work_while_the_index_is_embedded() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let server = MyceliumsMcp::new();
    let (base_url, texts) = slow_model(Duration::from_millis(15)).await;

    // Enough symbols for staged writes in the middle of the job (one every
    // 1,024 symbols), in requests of 16 texts.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("concurrency-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    for file in 0..2 {
        let functions: String = (0..550)
            .map(|i| {
                let n = file * 550 + i;
                format!(
                    "export function f{n}(x: number): number {{\n    return g{n}(x) + {n};\n}}\n\
                     function g{n}(x: number): number {{\n    return x * {n};\n}}\n"
                )
            })
            .collect();
        std::fs::write(project.join(format!("src/m{file}.ts")), functions).unwrap();
    }
    std::fs::write(
        project.join(CONFIG_FILENAME),
        format!(
            "[embedding]\nprovider = \"openai-compatible\"\nmodel = \"slow\"\n\
             base_url = \"{base_url}\"\ndim = {DIM}\n\n[analysis]\nembedding_batch_size = 16\n"
        ),
    )
    .unwrap();
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let out = server
        .analyze_for_test(AnalyzeParams {
            path: project.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(out.contains("in the background with slow"), "{out}");

    let search = |mode: SearchMode| SearchCodeParams {
        query: "multiply the input".to_string(),
        repo_id: Some(repo_id.clone()),
        mode: Some(mode),
        limit: Some(20),
        ..Default::default()
    };
    let mut calls = 0;
    let mut loads_seen = Vec::new();
    let mut while_running = 0;
    loop {
        let running = server
            .server_stats_for_test()
            .await
            .unwrap()
            .contains("building in the background");
        for mode in [SearchMode::Auto, SearchMode::Hybrid] {
            let result = server.search_code_for_test(search(mode)).await;
            match result {
                Ok(text) => assert!(!text.contains("replaced while"), "{text}"),
                // No symbol has a vector yet: semantic modes say so.
                Err(e) if e.contains("no symbol has a vector yet") => {}
                Err(e) => panic!("search ({mode:?}) failed during the embedding: {e}"),
            }
            calls += 1;
        }
        // Semantic search returns only symbols with a vector.
        match server
            .search_code_for_test(search(SearchMode::Semantic))
            .await
        {
            Ok(text) => assert!(!text.contains("replaced while"), "{text}"),
            Err(e) if e.contains("no symbol has a vector yet") => {}
            Err(e) => panic!("semantic search failed during the embedding: {e}"),
        }
        let impact = server
            .impact_of_change_for_test(ImpactOfChangeParams {
                symbol_name: Some("g7".to_string()),
                repo_id: Some(repo_id.clone()),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("impact failed during the embedding: {e}"));
        assert!(impact.contains("f7"), "{impact}");
        calls += 1;
        loads_seen.push(server.graph_table_loads_for_test());
        if running {
            while_running += 1;
        } else {
            break;
        }
    }
    assert!(
        while_running >= 3,
        "the calls ran while the job ran ({while_running} rounds)"
    );
    assert!(calls > 0);
    assert!(
        texts.load(Ordering::SeqCst) >= 2200,
        "the job embedded every symbol"
    );
    // While the job staged its vectors, the cached graph stayed: tables were
    // read once at the start and once more after the vectors moved in.
    let reloads = loads_seen.last().unwrap() - loads_seen.first().unwrap();
    assert!(
        reloads <= 10,
        "{reloads} table loads during the job: {loads_seen:?}"
    );
    let line = server
        .wait_for_embedding_for_test(&repo_id, Duration::from_secs(60))
        .await
        .unwrap();
    assert!(line.contains("built in the background"), "{line}");
}
