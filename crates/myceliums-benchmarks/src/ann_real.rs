//! Real embeddings of a real repository for the ANN recall measurement.
//!
//! The vectors come from the production pipeline: every source file under the
//! root is parsed with the shipped [`SourceParser`], turned into symbols with
//! [`to_code_symbols`], and embedded with [`Embedder::embed_symbols`] under the
//! shipped default embedding configuration — the same text template, model
//! and normalisation `myc analyze` uses. Content files (Markdown, notebooks,
//! mail) are skipped; they are documents, not code symbols.
//!
//! Queries are what a user types: for a seeded sample of held-out symbols,
//! the qualified name split into lower-case words (`QuerySet.get_or_create` →
//! `query set get or create`), embedded with [`Embedder::embed_query`] — the
//! query prefix and all. Held-out symbols are kept out of the table.
//!
//! Embedding tens of thousands of symbols takes a long time on a CPU, so the
//! vectors are cached in a file keyed by a digest of every embedding text,
//! every query and the model fingerprint — any change to the tree, the
//! template or the model re-embeds — and written in chunks, so an interrupted
//! run resumes.

use anyhow::{bail, Context, Result};
use myceliums_core::embeddings::build_embedding_text;
use myceliums_core::parser::{to_code_symbols, SourceLanguage, SourceParser};
use myceliums_core::{get_embedder_for, IndexEmbeddingMeta};
use myceliums_storage::CodeSymbol;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use walkdir::WalkDir;

use crate::ann::{Rng, Vectors};

/// Directory names never descended into: VCS metadata, dependencies, build output.
const SKIPPED_DIRS: &[&str] = &["node_modules", "vendor", "target", "dist", "build"];

/// Embedding batch size. `myc analyze` defaults to 256; a smaller batch keeps
/// peak memory down on a shared machine and does not change what is embedded
/// (`analysis.embedding_batch_size` is the same memory knob for users).
const EMBEDDING_BATCH: usize = 64;

const CACHE_MAGIC: &[u8; 8] = b"MYCANN02";

/// Table vectors embedded between two cache writes.
const CACHE_CHUNK: usize = 2_048;

/// The embedded repository: symbol vectors (the table pool) and query vectors.
#[derive(Debug, Clone)]
pub struct RealEmbeddings {
    /// Repository name for reports.
    pub label: String,
    /// One vector per non-held-out symbol, in a seeded random order.
    pub pool: Vec<Vec<f32>>,
    /// One vector per held-out query.
    pub queries: Vec<Vec<f32>>,
    /// Symbols parsed in total (pool + held out).
    pub symbols: usize,
}

impl RealEmbeddings {
    /// The first `rows` pool vectors and every query. Smaller tables are
    /// prefixes of larger ones, so sizes are nested.
    pub fn table(&self, rows: usize) -> Result<Vectors> {
        if rows > self.pool.len() {
            bail!(
                "{} has {} embeddable symbols outside the query set; {rows} were asked for",
                self.label,
                self.pool.len()
            );
        }
        Ok(Vectors {
            label: format!("real:{}", self.label),
            table: self.pool[..rows].to_vec(),
            queries: self.queries.clone(),
        })
    }
}

/// Parse `root`, hold out `queries` symbols as queries, keep the first
/// `pool_size` of the rest (seeded order), and load their vectors from `cache`
/// or embed them there.
///
/// The cache is written as it goes — the queries first, then the pool in
/// chunks of [`CACHE_CHUNK`] — so an interrupted run resumes where it stopped
/// instead of re-embedding everything.
pub async fn load_or_embed(
    root: &Path,
    cache: &Path,
    queries: usize,
    pool_size: usize,
    seed: u64,
    meta: &IndexEmbeddingMeta,
) -> Result<RealEmbeddings> {
    let label = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".to_string());
    let mut symbols = parse_tree(root)?;
    let parsed = symbols.len();
    if parsed < queries + pool_size {
        bail!(
            "{} yields {parsed} symbols — too few for {queries} queries plus a \
             {pool_size}-row table",
            root.display()
        );
    }
    let mut rng = Rng::new(seed);
    rng.shuffle(&mut symbols);
    let held_out = symbols.split_off(symbols.len() - queries);
    symbols.truncate(pool_size);
    let query_texts: Vec<String> = held_out
        .iter()
        .map(|s| identifier_words(&s.qualified_name))
        .collect();

    let mut digest = Sha256::new();
    digest.update(meta.fingerprint().as_bytes());
    let documents = symbols.iter().map(build_embedding_text);
    for text in documents.chain(query_texts.iter().cloned()) {
        digest.update((text.len() as u64).to_le_bytes());
        digest.update(text.as_bytes());
    }
    let key: [u8; 32] = digest.finalize().into();
    let dim = meta.dim;

    let (mut query_vectors, mut pool) = read_cache(cache, &key, dim, queries)?.unwrap_or_default();
    eprintln!(
        "{label}: {parsed} symbols parsed; {} of {pool_size} table vectors cached in {}",
        pool.len(),
        cache.display()
    );
    if query_vectors.len() == queries && pool.len() >= pool_size {
        pool.truncate(pool_size);
        return Ok(RealEmbeddings {
            label,
            symbols: parsed,
            pool,
            queries: query_vectors,
        });
    }

    let embedder = get_embedder_for(meta.clone())
        .await
        .context("loading the embedding model")?;
    if query_vectors.len() != queries {
        query_vectors.clear();
        for text in &query_texts {
            query_vectors.push(embedder.embed_query(text).await?);
        }
        start_cache(cache, &key, dim, &query_vectors)?;
    }
    while pool.len() < pool_size {
        let end = (pool.len() + CACHE_CHUNK).min(pool_size);
        let chunk = embedder
            .embed_symbols(&symbols[pool.len()..end], EMBEDDING_BATCH)
            .await
            .context("embedding the repository")?;
        append_cache(cache, &chunk)?;
        pool.extend(chunk);
        eprintln!("{label}: embedded {}/{pool_size}", pool.len());
    }
    Ok(RealEmbeddings {
        label,
        symbols: parsed,
        pool,
        queries: query_vectors,
    })
}

/// Every code symbol under `root`, files in sorted order.
fn parse_tree(root: &Path) -> Result<Vec<CodeSymbol>> {
    let mut files: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            entry.depth() == 0
                || !(entry.file_type().is_dir()
                    && (name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref())))
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect();
    files.sort();

    let mut symbols = Vec::new();
    for file in files {
        let Some(language) = file
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(SourceLanguage::from_extension)
        else {
            continue;
        };
        if language.is_content() {
            continue;
        }
        // Generated or binary-ish files that are not UTF-8 are not code.
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let relative = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let mut parser = SourceParser::new(language)
            .with_context(|| format!("creating a parser for {relative}"))?;
        // A file the grammar rejects is skipped, as analysis would.
        let Ok(parsed) = parser.parse(&source) else {
            continue;
        };
        symbols.extend(to_code_symbols(&parsed.symbols, &relative, "ann-real"));
    }
    Ok(symbols)
}

/// `QuerySet.get_or_create` → `query set get or create`.
pub fn identifier_words(identifier: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for c in identifier.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }
        let boundary =
            c.is_uppercase() && previous.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit());
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
        previous = Some(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.join(" ")
}

/// Query vectors and the table vectors embedded so far.
type Cached = (Vec<Vec<f32>>, Vec<Vec<f32>>);

const CACHE_HEADER: usize = 8 + 32 + 8;

/// Read a cache written for `key`: `queries` query vectors, then however many
/// complete table vectors were appended. `None` when absent or for another key.
fn read_cache(path: &Path, key: &[u8; 32], dim: usize, queries: usize) -> Result<Option<Cached>> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() < CACHE_HEADER || &bytes[..8] != CACHE_MAGIC || &bytes[8..40] != key {
        return Ok(None);
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    if word(40) != dim || word(44) != queries {
        return Ok(None);
    }
    let row = dim * 4;
    let mut vectors: Vec<Vec<f32>> = bytes[CACHE_HEADER..]
        .chunks_exact(row)
        .map(|r| {
            r.chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect()
        })
        .collect();
    if vectors.len() < queries {
        return Ok(None);
    }
    let pool = vectors.split_off(queries);
    // A trailing partial row from an interrupted write is dropped: it will be
    // re-embedded, and the file is rewritten from the last complete row.
    let complete = CACHE_HEADER + (queries + pool.len()) * row;
    if bytes.len() != complete {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(complete as u64)?;
    }
    Ok(Some((vectors, pool)))
}

/// Create the cache with its header and the query vectors.
fn start_cache(path: &Path, key: &[u8; 32], dim: usize, queries: &[Vec<f32>]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(path)
            .with_context(|| format!("creating the vector cache {}", path.display()))?,
    );
    out.write_all(CACHE_MAGIC)?;
    out.write_all(key)?;
    out.write_all(&(dim as u32).to_le_bytes())?;
    out.write_all(&(queries.len() as u32).to_le_bytes())?;
    for value in queries.iter().flatten() {
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}

/// Append table vectors to a started cache.
fn append_cache(path: &Path, vectors: &[Vec<f32>]) -> Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::OpenOptions::new().append(true).open(path)?);
    for value in vectors.iter().flatten() {
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_become_query_words() {
        assert_eq!(
            identifier_words("QuerySet.get_or_create"),
            "query set get or create"
        );
        assert_eq!(identifier_words("parseHTTPRequest"), "parse httprequest");
        assert_eq!(identifier_words("Vec2d::new"), "vec2d new");
        assert_eq!(identifier_words("__init__"), "init");
    }

    #[test]
    fn the_vector_cache_resumes_and_rejects_another_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.bin");
        let queries = vec![vec![0.8, 0.6]];
        start_cache(&path, &[7; 32], 2, &queries).unwrap();
        append_cache(&path, &[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        append_cache(&path, &[vec![0.6, 0.8]]).unwrap();
        let (read_queries, read_pool) = read_cache(&path, &[7; 32], 2, 1).unwrap().unwrap();
        assert_eq!(read_queries, queries);
        assert_eq!(
            read_pool,
            vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.6, 0.8]]
        );

        // A torn trailing row is dropped, and appending continues after it.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0, 0])
            .unwrap();
        let (_, read_pool) = read_cache(&path, &[7; 32], 2, 1).unwrap().unwrap();
        assert_eq!(read_pool.len(), 3);
        append_cache(&path, &[vec![0.0, -1.0]]).unwrap();
        let (_, read_pool) = read_cache(&path, &[7; 32], 2, 1).unwrap().unwrap();
        assert_eq!(read_pool.last(), Some(&vec![0.0, -1.0]));

        assert!(read_cache(&path, &[8; 32], 2, 1).unwrap().is_none());
        assert!(read_cache(&path, &[7; 32], 3, 1).unwrap().is_none());
        assert!(read_cache(&dir.path().join("missing"), &[7; 32], 2, 1)
            .unwrap()
            .is_none());
    }

    #[test]
    fn the_parser_walk_finds_code_and_skips_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "def alpha():\n    return 1\n").unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        std::fs::write(
            dir.path().join("node_modules").join("b.py"),
            "def beta():\n    return 2\n",
        )
        .unwrap();
        let names: Vec<String> = parse_tree(dir.path())
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(names.contains(&"alpha".to_string()), "{names:?}");
        assert!(!names.contains(&"beta".to_string()), "{names:?}");
    }
}
