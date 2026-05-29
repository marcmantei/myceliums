//! Embedding and reranking support via fastembed.
//!
//! Provides [`Embedder`] for generating vector embeddings of code symbols
//! (using all-MiniLM-L6-v2) and [`Reranker`] for cross-encoder reranking
//! (using BAAI/bge-reranker-base). Models are downloaded on first use and
//! cached locally.

use anyhow::{Context, Result};
use fastembed::{
    EmbeddingModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding, TextRerank,
};
use myceliums_storage::{CodeSymbol, SymbolMetadata};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::info;

/// The HuggingFace model repo used by fastembed for AllMiniLML6V2.
const MODEL_REPO: &str = "Qdrant/all-MiniLM-L6-v2-onnx";

/// The default fastembed cache directory name (relative to CWD unless overridden).
const DEFAULT_CACHE_DIR: &str = ".fastembed_cache";

/// Information about the fastembed model cache.
pub struct ModelCacheInfo {
    /// The directory where the model is cached.
    pub cache_dir: PathBuf,
    /// Whether the model files are present.
    pub is_cached: bool,
    /// Total size in bytes (0 if not cached).
    pub size_bytes: u64,
}

/// Get the fastembed cache directory, respecting `FASTEMBED_CACHE_DIR` and `HF_HOME` env vars.
pub(crate) fn get_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FASTEMBED_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("HF_HOME") {
        return PathBuf::from(dir);
    }
    PathBuf::from(DEFAULT_CACHE_DIR)
}

/// Get the model-specific subdirectory within the cache.
fn model_cache_subdir(cache_dir: &Path) -> PathBuf {
    let dir_name = format!("models--{}", MODEL_REPO.replace('/', "--"));
    cache_dir.join(dir_name)
}

/// Compute total size of a directory recursively.
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Check the status of the fastembed model cache.
pub fn check_model_cache() -> ModelCacheInfo {
    let cache_dir = get_cache_dir();
    let model_dir = model_cache_subdir(&cache_dir);
    let snapshots_dir = model_dir.join("snapshots");

    // The model is considered cached if the snapshots directory exists and contains files.
    let is_cached = snapshots_dir.exists()
        && std::fs::read_dir(&snapshots_dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);

    let size_bytes = if is_cached { dir_size(&model_dir) } else { 0 };

    ModelCacheInfo {
        cache_dir,
        is_cached,
        size_bytes,
    }
}

/// Generates vector embeddings for code symbols using all-MiniLM-L6-v2.
///
/// The underlying ONNX model (~100 MB) is downloaded on first use and
/// cached in the fastembed cache directory (set via `FASTEMBED_CACHE_DIR` env var).
pub struct Embedder {
    model: TextEmbedding,
}

/// Cross-encoder reranker using BAAI/bge-reranker-base.
///
/// Scores `(query, document)` pairs jointly, which is more accurate than
/// embedding-based similarity but slower (O(n) forward passes).
pub struct Reranker {
    model: TextRerank,
}

/// Truncate a byte-oriented slice to at most `max` bytes, ensuring the cut
/// lands on a valid UTF-8 character boundary.
fn truncate_to_char_boundary(s: &str, max: usize) -> &str {
    if max >= s.len() {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Build the embedding document text for a single [`CodeSymbol`].
///
/// The format is:
/// `{kind} {name} {decorators} {signature} {return_type} {superclasses} {content_head}`
///
/// Metadata fields are extracted from the JSON-serialised [`SymbolMetadata`]
/// stored in `CodeSymbol::metadata`. Missing or unparseable metadata is
/// gracefully skipped (empty strings).  Content is truncated to 512 bytes on
/// a character boundary to keep documents concise for the embedding model.
pub fn build_embedding_text(s: &CodeSymbol) -> String {
    let kind = s.kind.to_string();

    let meta: Option<SymbolMetadata> = s
        .metadata
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());

    let decorators = meta
        .as_ref()
        .map(|m| m.decorators.join(" "))
        .unwrap_or_default();
    let return_type = meta
        .as_ref()
        .and_then(|m| m.return_type.as_deref())
        .unwrap_or_default();
    let superclasses = meta
        .as_ref()
        .map(|m| m.superclasses.join(" "))
        .unwrap_or_default();

    let content_head = truncate_to_char_boundary(&s.content, 512);

    format!(
        "{kind} {name} {decorators} {signature} {return_type} {superclasses} {content_head}",
        name = s.name,
        signature = s.signature,
    )
}

impl Embedder {
    /// Create a new Embedder with all-MiniLM-L6-v2.
    /// This downloads the model (~100MB) on first use and prints progress to stderr.
    pub fn new() -> Result<Self> {
        let cache_info = check_model_cache();

        if !cache_info.is_cached {
            eprintln!(
                "Downloading fastembed model ({}, ~100 MB, one-time)... this may take a minute.",
                MODEL_REPO,
            );
        }

        info!("Loading embedding model (all-MiniLM-L6-v2)...");
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )
        .context("Failed to initialize fastembed model")?;

        if !cache_info.is_cached {
            let updated = check_model_cache();
            eprintln!(
                "Model downloaded to {} ({:.0} MB)",
                updated.cache_dir.display(),
                updated.size_bytes as f64 / 1_000_000.0,
            );
        }

        info!("Embedding model loaded.");
        Ok(Self { model })
    }

    /// Generate embeddings for a batch of symbols, processing in chunks to
    /// avoid OOM on large repositories.
    ///
    /// Each symbol's text is enriched with metadata (kind, decorators,
    /// return type, superclasses) and content truncated to 512 chars.
    pub fn embed_symbols(
        &self,
        symbols: &[CodeSymbol],
        batch_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        if symbols.is_empty() {
            return Ok(vec![]);
        }

        let batch_size = if batch_size == 0 { 256 } else { batch_size };
        let total_batches = symbols.len().div_ceil(batch_size);
        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(symbols.len());

        info!(
            "Generating embeddings for {} symbols in {} batches of {}...",
            symbols.len(),
            total_batches,
            batch_size,
        );

        for (i, chunk) in symbols.chunks(batch_size).enumerate() {
            info!(
                "Embedding batch {}/{} ({} symbols)...",
                i + 1,
                total_batches,
                chunk.len(),
            );

            let documents: Vec<String> = chunk.iter().map(build_embedding_text).collect();

            let embeddings = self
                .model
                .embed(documents, None)
                .context("Failed to generate embeddings")?;

            all_embeddings.extend(embeddings);
        }

        info!("Generated {} embeddings.", all_embeddings.len());
        Ok(all_embeddings)
    }

    /// Generate an embedding for a single query string.
    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let embeddings = self
            .model
            .embed(vec![query.to_string()], None)
            .context("Failed to embed query")?;

        embeddings
            .into_iter()
            .next()
            .context("No embedding returned for query")
    }

    /// Embed multiple texts at once. Returns one vector per input.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let embeddings = self
            .model
            .embed(texts.to_vec(), None)
            .context("Failed to embed batch")?;
        Ok(embeddings)
    }

    /// Build a searchable text from a [`CodeSymbol`].
    pub fn symbol_text(sym: &CodeSymbol) -> String {
        build_embedding_text(sym)
    }

    /// Compute cosine similarity between two vectors.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
        let dot: f64 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| *x as f64 * *y as f64)
            .sum();
        let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }

    /// Perform a brute-force vector search over symbols.
    /// Returns (symbol, similarity) pairs sorted by similarity descending.
    pub fn vector_search(
        &self,
        symbols: &[CodeSymbol],
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(CodeSymbol, f64)>> {
        let texts: Vec<String> = symbols.iter().map(Self::symbol_text).collect();
        let embeddings = self.embed_batch(&texts)?;

        let mut scored: Vec<(CodeSymbol, f64)> = symbols
            .iter()
            .zip(embeddings.iter())
            .map(|(sym, emb)| {
                let sim = Self::cosine_similarity(query_embedding, emb);
                (sym.clone(), sim)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }
}

/// Truncate a string to at most `max_chars` characters, splitting on a char
/// boundary so the result is always valid UTF-8.
pub fn truncate_content(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        // Fast path: byte length already within limit, so char length is too.
        return s;
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Global singleton for the embedder to avoid reloading the model.
static GLOBAL_EMBEDDER: OnceCell<Arc<Embedder>> = OnceCell::const_new();

/// Get or initialize the global embedder instance.
pub async fn get_embedder() -> Result<Arc<Embedder>> {
    GLOBAL_EMBEDDER
        .get_or_try_init(|| async {
            let embedder = Embedder::new()?;
            Ok::<_, anyhow::Error>(Arc::new(embedder))
        })
        .await
        .cloned()
}

impl Reranker {
    /// Create a new Reranker with BAAI/bge-reranker-base.
    /// This downloads the model (~140MB) on first use.
    pub fn new() -> Result<Self> {
        info!(
            "Loading reranker model (BAAI/bge-reranker-base)... This may download ~140MB on first use."
        );
        let model = TextRerank::try_new(
            RerankInitOptions::new(RerankerModel::BGERerankerBase)
                .with_show_download_progress(true),
        )
        .context("Failed to initialize fastembed reranker model")?;
        info!("Reranker model loaded.");
        Ok(Self { model })
    }

    /// Rerank a list of documents given a query.
    /// Returns pairs of (document_index, score) sorted by score descending.
    pub fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f32)>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Reranking {} documents for query: {}",
            documents.len(),
            query
        );

        // Convert documents to string references for the API
        let doc_refs: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();

        let rerank_results = self
            .model
            .rerank(query, doc_refs, true, None)
            .context("Failed to rerank documents")?;

        let mut results: Vec<(usize, f32)> =
            rerank_results.iter().map(|r| (r.index, r.score)).collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        info!("Reranked {} documents.", results.len());
        Ok(results)
    }
}

/// Global singleton for the reranker to avoid reloading the model.
static GLOBAL_RERANKER: OnceCell<Arc<Reranker>> = OnceCell::const_new();

/// Get or initialize the global reranker instance.
pub(crate) async fn get_reranker() -> Result<Arc<Reranker>> {
    GLOBAL_RERANKER
        .get_or_try_init(|| async {
            let reranker = Reranker::new()?;
            Ok::<_, anyhow::Error>(Arc::new(reranker))
        })
        .await
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    // --- truncate_to_char_boundary tests ---

    #[test]
    fn truncate_char_boundary_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn truncate_char_boundary_within_multibyte_2byte() {
        // U+00E9 (e-acute) is 2 bytes in UTF-8: 0xC3 0xA9
        let s = "caf\u{00E9}!";
        // Byte layout: c(1) a(1) f(1) \xC3\xA9(2) !(1) = 6 bytes
        // Cutting at byte 4 lands inside the e-acute; should back up to 3.
        assert_eq!(truncate_to_char_boundary(s, 4), "caf");
    }

    #[test]
    fn truncate_char_boundary_emoji_4byte() {
        // U+1F600 is 4 bytes in UTF-8
        let s = "hi\u{1F600}bye";
        // Byte layout: h(1) i(1) \xF0\x9F\x98\x80(4) b(1) y(1) e(1) = 9 bytes
        // Cutting at byte 3 lands inside the emoji; should back up to 2.
        assert_eq!(truncate_to_char_boundary(s, 3), "hi");
        // Cutting at byte 6 = right after the emoji.
        assert_eq!(truncate_to_char_boundary(s, 6), "hi\u{1F600}");
    }

    #[test]
    fn truncate_char_boundary_3byte_char() {
        // U+4E16 (Chinese character) is 3 bytes: 0xE4 0xB8 0x96
        let s = "a\u{4E16}b";
        // Byte layout: a(1) \xE4\xB8\x96(3) b(1) = 5 bytes
        // Cutting at byte 2 lands inside the 3-byte char; back up to 1.
        assert_eq!(truncate_to_char_boundary(s, 2), "a");
        // Cutting at byte 4 = right after the Chinese char.
        assert_eq!(truncate_to_char_boundary(s, 4), "a\u{4E16}");
    }

    #[test]
    fn truncate_char_boundary_exact_fit() {
        let s = "abc";
        assert_eq!(truncate_to_char_boundary(s, 3), "abc");
        assert_eq!(truncate_to_char_boundary(s, 100), "abc");
    }

    #[test]
    fn truncate_char_boundary_empty() {
        assert_eq!(truncate_to_char_boundary("", 10), "");
        assert_eq!(truncate_to_char_boundary("hello", 0), "");
    }

    // --- build_embedding_text tests ---

    fn make_symbol(
        name: &str,
        kind: SymbolKind,
        signature: &str,
        content: &str,
        metadata_json: Option<&str>,
    ) -> CodeSymbol {
        CodeSymbol {
            uid: "uid-1".to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind,
            file_path: "src/main.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: signature.to_string(),
            content: content.to_string(),
            repo_id: "repo-1".to_string(),
            metadata: metadata_json.map(|s| s.to_string()),
        }
    }

    #[test]
    fn build_text_no_metadata() {
        let sym = make_symbol(
            "foo",
            SymbolKind::Function,
            "fn foo(x: i32) -> bool",
            "{ x > 0 }",
            None,
        );
        let text = build_embedding_text(&sym);
        assert_eq!(text, "Function foo  fn foo(x: i32) -> bool   { x > 0 }");
    }

    #[test]
    fn build_text_with_full_metadata() {
        let meta = r#"{"decorators":["@decorator","@other"],"return_type":"bool","superclasses":["Base","Mixin"],"type_params":[]}"#;
        let sym = make_symbol(
            "MyClass",
            SymbolKind::Class,
            "class MyClass(Base, Mixin)",
            "pass",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert_eq!(
            text,
            "Class MyClass @decorator @other class MyClass(Base, Mixin) bool Base Mixin pass"
        );
    }

    #[test]
    fn build_text_with_partial_metadata() {
        // Only decorators, no return_type or superclasses
        let meta = r#"{"decorators":["@app.route"]}"#;
        let sym = make_symbol(
            "index",
            SymbolKind::Function,
            "def index()",
            "return render_template('index.html')",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert!(text.contains("@app.route"));
        assert!(text.contains("Function"));
        assert!(text.contains("def index()"));
        assert!(text.contains("return render_template('index.html')"));
    }

    #[test]
    fn build_text_invalid_metadata_json() {
        // Malformed JSON should be gracefully ignored
        let sym = make_symbol(
            "bar",
            SymbolKind::Method,
            "def bar(self)",
            "pass",
            Some("{not valid json"),
        );
        let text = build_embedding_text(&sym);
        // Should still produce output with empty metadata fields
        assert!(text.starts_with("Method bar"));
        assert!(text.contains("def bar(self)"));
    }

    #[test]
    fn build_text_content_truncated_at_512_bytes() {
        let long_content = "x".repeat(1000);
        let sym = make_symbol("big", SymbolKind::Function, "fn big()", &long_content, None);
        let text = build_embedding_text(&sym);
        // The content portion should be at most 512 bytes
        // Total text = "Function big  fn big()   " + content_head
        let prefix = "Function big  fn big()   ";
        let content_in_text = &text[prefix.len()..];
        assert_eq!(content_in_text.len(), 512);
    }

    #[test]
    fn build_text_content_truncated_multibyte() {
        // Build content that has multi-byte chars around the 512-byte boundary.
        // U+00E9 is 2 bytes. Fill with 'a' up to 511, then add e-acute.
        let mut content = "a".repeat(511);
        content.push('\u{00E9}'); // 2 bytes: total 513 bytes
        content.push_str("zzz");
        let sym = make_symbol("mb", SymbolKind::Function, "fn mb()", &content, None);
        let text = build_embedding_text(&sym);
        // Content should be truncated to 511 bytes (backing up from 512 to
        // avoid splitting the 2-byte char).
        let prefix = "Function mb  fn mb()   ";
        let content_in_text = &text[prefix.len()..];
        assert_eq!(content_in_text.len(), 511);
        assert_eq!(content_in_text, "a".repeat(511));
    }

    #[test]
    fn build_text_empty_metadata_fields() {
        // SymbolMetadata with all defaults (empty vecs, None)
        let meta = r#"{}"#;
        let sym = make_symbol(
            "empty_meta",
            SymbolKind::Variable,
            "let x",
            "42",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert_eq!(text, "Variable empty_meta  let x   42");
    }

    #[test]
    fn build_text_short_content_not_truncated() {
        let sym = make_symbol("short", SymbolKind::Constant, "const X: u32", "= 1;", None);
        let text = build_embedding_text(&sym);
        assert!(text.ends_with("= 1;"));
    }
}
