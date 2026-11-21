//! Embedding the symbols an index holds without a vector, in the background
//! and resumably (#157).
//!
//! A full analysis with [`crate::Analyzer::defer_embeddings`] builds the graph
//! and leaves every symbol it could not take a vector over for to this step.
//! The MCP server runs it in the background after `analyze` answered, so a
//! first index — hundreds of megabytes of model download and minutes of
//! inference with the default model — never runs inside one tool call.
//!
//! It writes the live index while tools read it, so it touches no table the
//! graph is read from until it is done. Every few slices, the vectors so far
//! are added to the `chunks` table — a symbol's own vector staged as chunk 0,
//! next to its body chunks — where vector search finds them, and the
//! accounting is updated; neither table counts towards the index's graph
//! generation ([`crate::service::UNCACHED_TABLES`]), so cached graphs stay and
//! running reads do not fail. At the end the staged vectors move onto their
//! symbols' rows in one write, and the tables are compacted. A run that stops
//! — the process ends, the model fails — keeps what it staged; the next run
//! moves that first and embeds only the rest.

use crate::embedding_stats::EmbeddingStats;
use crate::embeddings::{embedding_stopped, get_embedder_for, EmbedOptions, IndexEmbeddingMeta};
use crate::error::{MyceliumError, Result};
use myceliums_storage::{ChunkVector, CodeSymbol, Store};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Symbols embedded per slice: how often a stop request and the progress are
/// looked at between model calls.
pub const PENDING_SLICE_SYMBOLS: usize = 256;

/// Staged vectors are written once this many symbols are embedded...
pub const PERSIST_SYMBOLS: usize = 1024;

/// ...or once this long has passed since the last write: what a stopped run
/// loses at most.
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(20);

/// Called with `(done, total)` pending symbols as [`embed_pending`] proceeds.
pub type PendingProgress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);

/// What a run of [`embed_pending`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingOutcome {
    /// Symbols the index holds.
    pub symbols_total: usize,
    /// Symbols that had no vector when the run started.
    pub pending: usize,
    /// Of those, the ones this run embedded and stored.
    pub embedded: usize,
    /// Whether the run stopped early because `stop` (or
    /// [`crate::embeddings::stop_embedding`]) asked it to.
    pub stopped: bool,
}

/// The pending symbols of the index in `store`: those without a vector.
pub async fn pending_symbols(store: &Store) -> Result<(Vec<CodeSymbol>, usize)> {
    let symbols = store.get_symbols_with_vectors().await?;
    let total = symbols.len();
    let pending = symbols
        .into_iter()
        .filter_map(|(symbol, vector)| vector.is_none().then_some(symbol))
        .collect();
    Ok((pending, total))
}

/// Embed every symbol of the index in `store` that has no vector, with the
/// embedder the index records, [`PENDING_SLICE_SYMBOLS`] at a time, staging
/// the vectors and moving them onto the symbols' rows at the end (see the
/// module documentation). `stop` is asked before every slice and between
/// two model batches; a stopped run keeps what it staged.
///
/// Fails when the index records no embedder, or when the model cannot be
/// loaded or embed; the accounting then counts the symbols still without a
/// vector as failures, and what was staged stays.
pub async fn embed_pending(
    store: &Store,
    options: EmbedOptions,
    progress: Option<PendingProgress<'_>>,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<PendingOutcome> {
    let meta: IndexEmbeddingMeta = match store.get_index_meta(IndexEmbeddingMeta::META_KEY).await? {
        Some(json) => serde_json::from_str(&json)?,
        None => {
            return Err(MyceliumError::Embedding(
                "the index records no embedding model; run a full analysis".to_string(),
            ))
        }
    };
    store.set_embedding_dim(meta.dim as i32);
    let stopped = || stop() || embedding_stopped();
    // What an earlier run staged and did not move.
    let leftovers = store.promote_staged_vectors().await?;
    if leftovers > 0 {
        info!("Moved {leftovers} vectors a stopped background embedding had staged");
    }
    let (pending, symbols_total) = pending_symbols(store).await?;
    let mut outcome = PendingOutcome {
        symbols_total,
        pending: pending.len(),
        embedded: 0,
        stopped: false,
    };
    let embedded_before = symbols_total - pending.len();
    let record = |embedded: usize, failures: usize| EmbeddingStats {
        symbols_total,
        symbols_embedded: embedded_before + embedded,
        embedding_failures: failures,
    };
    if pending.is_empty() {
        record(0, 0).record(store).await?;
        if leftovers > 0 {
            compact(store).await;
        }
        return Ok(outcome);
    }
    info!(
        "Embedding {} of {} symbols in the background ({})",
        pending.len(),
        symbols_total,
        meta.identity()
    );
    // Rows a run of an earlier build stored before the symbol's own vector:
    // the symbol is embedded again, chunks included.
    let uids: Vec<String> = pending.iter().map(|s| s.uid.clone()).collect();
    store.delete_chunk_vectors_of(&uids).await?;
    store.ensure_chunks_table().await?;
    let report = |done: usize| {
        if let Some(progress) = progress {
            progress(done, pending.len());
        }
    };
    report(0);
    let embedder = match get_embedder_for(meta.clone()).await {
        Ok(embedder) => embedder,
        Err(e) if e.is_cancelled() || stopped() => {
            outcome.stopped = true;
            return Ok(outcome);
        }
        Err(e) => {
            record(0, pending.len()).record(store).await?;
            return Err(e);
        }
    };
    let mut staged: Vec<ChunkVector> = Vec::new();
    let mut staged_symbols = 0;
    let mut last_write = Instant::now();
    let mut done = 0;
    for slice in pending.chunks(PENDING_SLICE_SYMBOLS) {
        if stopped() {
            outcome.stopped = true;
            break;
        }
        let refs: Vec<&CodeSymbol> = slice.iter().collect();
        let within = |texts_done: usize, texts_total: usize| {
            report(done + texts_done * slice.len() / texts_total.max(1));
        };
        let vectors = match embedder
            .embed_index_until(&refs, options, None, Some(&within), &stopped)
            .await
        {
            Ok(vectors) => vectors,
            Err(e) if e.is_cancelled() => {
                outcome.stopped = true;
                break;
            }
            Err(e) => {
                outcome.embedded += write_staged(store, &mut staged, &mut staged_symbols).await?;
                record(outcome.embedded, pending.len() - outcome.embedded)
                    .record(store)
                    .await?;
                return Err(e);
            }
        };
        let chunks = vectors.chunk_rows(&refs);
        staged.extend(
            refs.iter()
                .zip(vectors.primary)
                .map(|(s, vector)| ChunkVector {
                    symbol_uid: s.uid.clone(),
                    file_path: s.file_path.clone(),
                    chunk_index: 0,
                    text_hash: String::new(),
                    vector,
                }),
        );
        staged.extend(chunks);
        staged_symbols += slice.len();
        done += slice.len();
        if staged_symbols >= PERSIST_SYMBOLS || last_write.elapsed() >= PERSIST_INTERVAL {
            outcome.embedded += write_staged(store, &mut staged, &mut staged_symbols).await?;
            record(outcome.embedded, 0).record(store).await?;
            last_write = Instant::now();
        }
        report(done);
    }
    outcome.embedded += write_staged(store, &mut staged, &mut staged_symbols).await?;
    record(outcome.embedded, 0).record(store).await?;
    if !outcome.stopped {
        // Every symbol has its vector: onto the symbols' rows, in one write.
        let promoted = store.promote_staged_vectors().await?;
        if promoted < outcome.embedded {
            warn!(
                "{} of {} embedded symbols were no longer in the index",
                outcome.embedded - promoted,
                outcome.embedded
            );
        }
        compact(store).await;
    }
    info!(
        "Background embedding: {} of {} pending symbols embedded{}",
        outcome.embedded,
        outcome.pending,
        if outcome.stopped { " (stopped)" } else { "" }
    );
    Ok(outcome)
}

/// Adds the staged rows to the `chunks` table in one write; returns how many
/// symbols they cover.
async fn write_staged(
    store: &Store,
    staged: &mut Vec<ChunkVector>,
    staged_symbols: &mut usize,
) -> Result<usize> {
    if staged.is_empty() {
        return Ok(0);
    }
    store.store_chunk_vectors(std::mem::take(staged)).await?;
    Ok(std::mem::take(staged_symbols))
}

/// Compacts the tables the embedding wrote in many small commits. Best
/// effort: an uncompacted table is only slower to scan.
async fn compact(store: &Store) {
    if let Err(e) = store.compact_vector_tables().await {
        warn!("Could not compact the vector tables: {e}");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::ProjectConfig;
    use crate::embeddings::PreviousVectors;
    use crate::Analyzer;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Dimension of the fake model's vectors.
    pub(crate) const FAKE_DIM: usize = 8;

    /// A deterministic, non-zero unit-free vector for `text`.
    fn fake_vector(text: &str) -> Vec<f32> {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(text.as_bytes());
        (0..FAKE_DIM).map(|i| 1.0 + digest[i] as f32).collect()
    }

    /// An OpenAI-compatible embeddings endpoint on localhost: the base URL
    /// and how many texts it has embedded.
    pub(crate) async fn fake_embedding_server() -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let texts = Arc::new(AtomicUsize::new(0));
        let counter = texts.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let counter = counter.clone();
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
                    let request: serde_json::Value =
                        serde_json::from_slice(&buf[body_at..body_at + length]).unwrap();
                    let inputs = request["input"].as_array().cloned().unwrap_or_default();
                    counter.fetch_add(inputs.len(), Ordering::SeqCst);
                    let data: Vec<serde_json::Value> = inputs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            serde_json::json!({
                                "index": i,
                                "embedding": fake_vector(t.as_str().unwrap_or_default()),
                            })
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
        (format!("http://{addr}/v1"), texts)
    }

    fn config(base_url: &str) -> ProjectConfig {
        let mut config = ProjectConfig::default();
        config.embedding.provider = "openai-compatible".to_string();
        config.embedding.model = "fake".to_string();
        config.embedding.base_url = Some(base_url.to_string());
        config.embedding.dim = Some(FAKE_DIM);
        config
    }

    /// A project of `n` TypeScript functions in two files.
    fn project(root: &Path, n: usize, changed: bool) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        let functions = |from: usize, to: usize| {
            (from..to)
                .map(|i| format!("export function f{i}(): number {{\n    return {i};\n}}\n"))
                .collect::<String>()
        };
        std::fs::write(root.join("src/a.ts"), functions(0, n / 2)).unwrap();
        let mut b = functions(n / 2, n);
        if changed {
            b.push_str("export function added(): number {\n    return -1;\n}\n");
        }
        std::fs::write(root.join("src/b.ts"), b).unwrap();
    }

    async fn deferred(root: &Path, db: &Path, base_url: &str) -> crate::analyzer::AnalysisResult {
        let store = Store::open(db, "pending-test").await.unwrap();
        Analyzer::with_config(store, root.to_path_buf(), config(base_url))
            .defer_embeddings(true)
            .analyze()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_deferred_analysis_leaves_the_embedding_to_embed_pending() {
        let (base_url, texts) = fake_embedding_server().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        project(&root, 6, false);
        let db = dir.path().join("db");

        let result = deferred(&root, &db, &base_url).await;
        assert_eq!(
            texts.load(Ordering::SeqCst),
            0,
            "no model call in the analysis"
        );
        assert!(result.symbols_total >= 6);
        assert_eq!(result.symbols_embedded, 0);
        assert_eq!(result.embedding_failures, 0, "pending is no failure");
        assert_eq!(result.embeddings_pending, result.symbols_total);

        let store = Store::open(&db, "pending-test").await.unwrap();
        let stats = EmbeddingStats::load(&store).await.unwrap().unwrap();
        assert_eq!((stats.symbols_embedded, stats.embedding_failures), (0, 0));
        assert!(
            store
                .get_index_meta(IndexEmbeddingMeta::META_KEY)
                .await
                .unwrap()
                .is_some(),
            "the embedder is recorded for the pending run"
        );

        let seen = std::sync::Mutex::new(Vec::new());
        let progress = |done: usize, total: usize| seen.lock().unwrap().push((done, total));
        let outcome = embed_pending(&store, EmbedOptions::default(), Some(&progress), &|| false)
            .await
            .unwrap();
        assert_eq!(outcome.pending, result.symbols_total);
        assert_eq!(outcome.embedded, result.symbols_total);
        assert!(!outcome.stopped);
        assert_eq!(texts.load(Ordering::SeqCst), result.symbols_total);
        let (left, _) = pending_symbols(&store).await.unwrap();
        assert!(left.is_empty());
        let stats = EmbeddingStats::load(&store).await.unwrap().unwrap();
        assert!(!stats.is_partial(), "{stats:?}");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, result.symbols_total)));
        assert_eq!(
            seen.last(),
            Some(&(result.symbols_total, result.symbols_total))
        );
    }

    #[tokio::test]
    async fn an_interrupted_run_resumes_where_it_stopped() {
        let (base_url, texts) = fake_embedding_server().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let n = PENDING_SLICE_SYMBOLS + 40;
        project(&root, n, false);
        let db = dir.path().join("db");
        let result = deferred(&root, &db, &base_url).await;
        let store = Store::open(&db, "pending-test").await.unwrap();

        // Stopped after the first slice, as a server shutting down would be.
        let done = AtomicUsize::new(0);
        let progress = |d: usize, _: usize| done.store(d, Ordering::SeqCst);
        let stop = || done.load(Ordering::SeqCst) >= PENDING_SLICE_SYMBOLS;
        let first = embed_pending(&store, EmbedOptions::default(), Some(&progress), &stop)
            .await
            .unwrap();
        assert!(first.stopped);
        assert_eq!(first.embedded, PENDING_SLICE_SYMBOLS);
        let stats = EmbeddingStats::load(&store).await.unwrap().unwrap();
        assert_eq!(stats.symbols_embedded, PENDING_SLICE_SYMBOLS);
        // Staged, not yet on the symbols' rows: the graph tables are as the
        // analysis left them.
        let (left, _) = pending_symbols(&store).await.unwrap();
        assert_eq!(left.len(), result.symbols_total);
        let staged = store.get_chunk_vectors().await.unwrap();
        assert_eq!(
            staged.iter().filter(|c| c.chunk_index == 0).count(),
            PENDING_SLICE_SYMBOLS
        );

        let second = embed_pending(&store, EmbedOptions::default(), None, &|| false)
            .await
            .unwrap();
        assert_eq!(second.pending, result.symbols_total - PENDING_SLICE_SYMBOLS);
        assert_eq!(second.embedded, second.pending);
        assert_eq!(
            texts.load(Ordering::SeqCst),
            result.symbols_total,
            "nothing is embedded twice"
        );
        assert!(!EmbeddingStats::load(&store)
            .await
            .unwrap()
            .unwrap()
            .is_partial());
    }

    #[tokio::test]
    async fn a_deferred_reanalysis_keeps_the_vectors_of_unchanged_symbols() {
        let (base_url, texts) = fake_embedding_server().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        project(&root, 6, false);
        let first_db = dir.path().join("first");
        deferred(&root, &first_db, &base_url).await;
        let first = Store::open(&first_db, "pending-test").await.unwrap();
        embed_pending(&first, EmbedOptions::default(), None, &|| false)
            .await
            .unwrap();
        let embedded_once = texts.load(Ordering::SeqCst);

        project(&root, 6, true);
        let previous =
            PreviousVectors::load(&first, myceliums_storage::schema::VECTOR_GEOMETRY_VERSION)
                .await
                .unwrap();
        let second_db = dir.path().join("second");
        let store = Store::open(&second_db, "pending-test").await.unwrap();
        let result = Analyzer::with_config(store, root.clone(), config(&base_url))
            .with_previous_vectors(previous)
            .defer_embeddings(true)
            .analyze()
            .await
            .unwrap();
        assert_eq!(result.embeddings_pending, 1, "only the added function");
        assert_eq!(result.embeddings_reused, result.symbols_total - 1);

        let store = Store::open(&second_db, "pending-test").await.unwrap();
        let outcome = embed_pending(&store, EmbedOptions::default(), None, &|| false)
            .await
            .unwrap();
        assert_eq!(outcome.embedded, 1);
        assert_eq!(texts.load(Ordering::SeqCst), embedded_once + 1);
    }

    /// While vectors are staged, no table the graph is read from changes:
    /// the index's graph generation stays, cached graphs and running reads
    /// survive the background embedding, and vector search already finds the
    /// staged symbols — and never one without a vector. Moving the vectors in
    /// at the end is one new generation.
    #[tokio::test]
    async fn staging_keeps_the_graph_generation_and_search_finds_the_staged_symbols() {
        use crate::service::IndexFingerprint;
        let (base_url, _) = fake_embedding_server().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let n = PENDING_SLICE_SYMBOLS + 40;
        project(&root, n, false);
        let db = dir.path().join("db");
        let result = deferred(&root, &db, &base_url).await;
        let store = Store::open(&db, "pending-test").await.unwrap();
        let before = IndexFingerprint::read(&db);

        let done = AtomicUsize::new(0);
        let progress = |d: usize, _: usize| done.store(d, Ordering::SeqCst);
        let stop = || done.load(Ordering::SeqCst) >= PENDING_SLICE_SYMBOLS;
        embed_pending(&store, EmbedOptions::default(), Some(&progress), &stop)
            .await
            .unwrap();
        assert_eq!(
            IndexFingerprint::read(&db),
            before,
            "staging and the accounting are no new graph generation"
        );
        let staged: std::collections::HashSet<String> = store
            .get_chunk_vectors()
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.chunk_index == 0)
            .map(|c| c.symbol_uid)
            .collect();
        let query = fake_vector("export function f3(): number {");
        let mut query = query;
        crate::embeddings::l2_normalize(&mut query);
        let hits = store.vector_search(&query, 50).await.unwrap();
        assert_eq!(hits.len(), 50, "staged symbols are found");
        assert!(
            hits.iter().all(|(s, _)| staged.contains(&s.uid)),
            "only symbols with a vector are found"
        );

        embed_pending(&store, EmbedOptions::default(), None, &|| false)
            .await
            .unwrap();
        assert_ne!(
            IndexFingerprint::read(&db),
            before,
            "moving the vectors in is"
        );
        assert!(store
            .get_chunk_vectors()
            .await
            .unwrap()
            .iter()
            .all(|c| c.chunk_index > 0));
        let (left, _) = pending_symbols(&store).await.unwrap();
        assert!(left.is_empty());
        let hits = store.vector_search(&query, 10).await.unwrap();
        assert_eq!(hits.len(), 10);
        assert!(result.symbols_total > 50);
    }

    #[tokio::test]
    async fn an_index_without_an_embedder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "pending-test").await.unwrap();
        let err = embed_pending(&store, EmbedOptions::default(), None, &|| false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no embedding model"), "{err}");
    }
}
