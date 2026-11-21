//! Local embedding inference: the model files of a pinned HuggingFace revision,
//! the model's own tokenizer, and an ONNX Runtime session (#157).
//!
//! fastembed ran the local models until #157. It gave no control over the
//! things that decide indexing speed and memory — the session's thread count,
//! how texts are grouped into batches, the token window — and it could not
//! load a model whose weights live in an external-data file, nor pin which
//! revision of a repository it downloads. This module does those parts itself
//! with the crates fastembed is built on (`ort`, `tokenizers`, `hf-hub`, at the
//! versions fastembed locks), and mirrors fastembed's numerics: the same
//! optimisation level, the first graph output unless `last_hidden_state`
//! exists, mean or CLS pooling over the attention mask, unit length.
//!
//! Batching sorts the texts by token count and fills each batch up to a token
//! budget, so a batch pads to a length its texts share instead of to the
//! longest text in a random group of 256 (#95 lost a 16 GB runner to that).

use super::{get_cache_dir, EmbeddingModelSpec, Pooling, WeightsFile};
use anyhow::{anyhow, bail, Context, Result};
use ort::session::{builder::GraphOptimizationLevel, Session, SessionInputValue};
use ort::value::Tensor;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tokenizers::{AddedToken, Tokenizer, TruncationParams};
use tracing::info;

/// Environment variable that sets the ONNX Runtime intra-op thread count of
/// every embedding session in the process. Unset or `0`: all logical cores.
pub const THREADS_ENV: &str = "MYCELIUMS_EMBEDDING_THREADS";

/// The intra-op thread count for new sessions: [`THREADS_ENV`] when it holds a
/// positive integer, otherwise every logical core.
pub fn session_threads() -> usize {
    parse_threads(std::env::var(THREADS_ENV).ok().as_deref())
}

fn parse_threads(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
}

/// How texts are grouped into ONNX batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    /// Most texts in one batch.
    pub max_texts: usize,
    /// Most tokens in one batch, counted as `texts × padded length` — what
    /// the session actually allocates activations for.
    pub max_tokens: usize,
}

impl BatchLimits {
    /// Token budget per batch when nothing else is configured: 32 texts at a
    /// full 512-token window, many more when the texts are short. Measured
    /// safe on the 16 GB CI runner for every registry model (#157).
    pub const DEFAULT_MAX_TOKENS: usize = 16_384;

    /// Texts per batch when nothing else is configured (an upper bound; the
    /// token budget usually binds first).
    pub const DEFAULT_MAX_TEXTS: usize = 256;

    /// Limits from the configured values, `0` meaning the default.
    pub fn new(max_texts: usize, max_tokens: usize) -> Self {
        Self {
            max_texts: if max_texts == 0 {
                Self::DEFAULT_MAX_TEXTS
            } else {
                max_texts
            },
            max_tokens: if max_tokens == 0 {
                Self::DEFAULT_MAX_TOKENS
            } else {
                max_tokens
            },
        }
    }
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

/// Token ids of one text, ready to be padded into a batch.
#[derive(Debug, Clone)]
pub(crate) struct Encoded {
    pub ids: Vec<u32>,
    pub type_ids: Vec<u32>,
}

impl Encoded {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
}

/// What the session wants as input, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Ids,
    Mask,
    TypeIds,
    Positions,
}

/// A loaded local embedding model.
pub(crate) struct LocalModel {
    spec: &'static EmbeddingModelSpec,
    session: Session,
    /// Adds special tokens and truncates at the window.
    tokenizer: Tokenizer,
    /// Neither: token offsets for chunking.
    splitter: Tokenizer,
    inputs: Vec<(String, InputKind)>,
    output: String,
    pad_id: u32,
    window: usize,
    /// Special tokens the tokenizer adds to every single text.
    special_tokens: usize,
}

impl LocalModel {
    /// Load `spec` from the cache, downloading its pinned files on a miss.
    pub fn load(spec: &'static EmbeddingModelSpec, window: usize) -> Result<Self> {
        let cache = get_cache_dir();
        let files = ModelFiles::resolve(spec, &cache, true)?;
        let threads = session_threads();
        info!(
            "Loading embedding model {} ({}@{}, {} threads)",
            spec.id,
            spec.repo,
            &spec.revision[..spec.revision.len().min(10)],
            threads
        );
        let session = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(threads))
            .and_then(|b| b.commit_from_file(&files.onnx))
            .map_err(|e| anyhow!("loading the ONNX graph of '{}': {e}", spec.id))?;

        let mut inputs = Vec::new();
        for input in &session.inputs {
            let kind = match input.name.as_str() {
                "input_ids" => InputKind::Ids,
                "attention_mask" => InputKind::Mask,
                "token_type_ids" => InputKind::TypeIds,
                "position_ids" => InputKind::Positions,
                other => bail!(
                    "embedding model '{}' expects an input named '{other}', which this \
                     version cannot provide",
                    spec.id
                ),
            };
            inputs.push((input.name.clone(), kind));
        }
        if !inputs.iter().any(|(_, k)| *k == InputKind::Ids) {
            bail!("embedding model '{}' has no input_ids input", spec.id);
        }
        // fastembed's choice: the token embeddings when the graph names them,
        // otherwise its first output (pooled already when it is 2-D).
        let output = session
            .outputs
            .iter()
            .find(|o| o.name == "last_hidden_state")
            .or_else(|| session.outputs.first())
            .map(|o| o.name.clone())
            .ok_or_else(|| anyhow!("embedding model '{}' has no outputs", spec.id))?;

        let window = window.min(spec.max_input_tokens).max(8);
        let (tokenizer, splitter, pad_id) = load_tokenizers(&files, window)
            .with_context(|| format!("loading the tokenizer of '{}'", spec.id))?;
        let special_tokens = tokenizer
            .encode_fast("", true)
            .map(|e| e.len())
            .map_err(|e| anyhow!("{e}"))?;
        let model = Self {
            spec,
            session,
            tokenizer,
            splitter,
            inputs,
            output,
            pad_id,
            window,
            special_tokens,
        };
        info!("Embedding model {}: {}", spec.id, model.signature());
        Ok(model)
    }

    /// The graph's inputs and the output pooled, for diagnostics.
    pub fn signature(&self) -> String {
        format!(
            "inputs {:?}, output '{}' of {:?}, pooling {:?}, window {}",
            self.inputs
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            self.output,
            self.session
                .outputs
                .iter()
                .find(|o| o.name == self.output)
                .map(|o| &o.output_type),
            self.spec.pooling,
            self.window
        )
    }

    /// Tokens per embedded text, special tokens included.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Special tokens added to every text.
    pub fn special_tokens(&self) -> usize {
        self.special_tokens
    }

    /// The tokenizer without truncation or special tokens, for chunking.
    pub fn splitter(&self) -> &Tokenizer {
        &self.splitter
    }

    /// Token count of `text` without special tokens or truncation.
    pub fn count_tokens(&self, text: &str) -> Result<usize> {
        Ok(self
            .splitter
            .encode_fast(text, false)
            .map_err(|e| anyhow!("tokenizing: {e}"))?
            .len())
    }

    /// Tokenize `texts` for the model: special tokens added, cut at the window.
    pub fn encode(&self, texts: &[String]) -> Result<Vec<Encoded>> {
        let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encodings = self
            .tokenizer
            .encode_batch_fast(inputs, true)
            .map_err(|e| anyhow!("tokenizing for '{}': {e}", self.spec.id))?;
        Ok(encodings
            .into_iter()
            .map(|e| Encoded {
                ids: e.get_ids().to_vec(),
                type_ids: e.get_type_ids().to_vec(),
            })
            .collect())
    }

    /// Embed already tokenized texts in token-budgeted batches, longest first;
    /// unit-length vectors in input order. `progress` is told how many texts
    /// are done after every batch.
    pub fn embed_encoded(
        &self,
        encoded: &[Encoded],
        limits: BatchLimits,
        progress: &mut dyn FnMut(usize),
    ) -> Result<Vec<Vec<f32>>> {
        let mut out: Vec<Option<Vec<f32>>> = vec![None; encoded.len()];
        let mut done = 0;
        for batch in plan_batches(encoded, limits) {
            let rows: Vec<&Encoded> = batch.iter().map(|&i| &encoded[i]).collect();
            let vectors = self.run(&rows)?;
            for (&i, vector) in batch.iter().zip(vectors) {
                out[i] = Some(vector);
            }
            done += batch.len();
            progress(done);
        }
        out.into_iter()
            .map(|v| v.ok_or_else(|| anyhow!("a text was left out of every batch")))
            .collect()
    }

    /// One forward pass over `rows`, padded to the longest of them.
    fn run(&self, rows: &[&Encoded]) -> Result<Vec<Vec<f32>>> {
        let batch = rows.len();
        let len = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
        let mut ids = vec![self.pad_id as i64; batch * len];
        let mut mask = vec![0i64; batch * len];
        let mut type_ids = vec![0i64; batch * len];
        for (b, row) in rows.iter().enumerate() {
            let base = b * len;
            for (t, (&id, &ty)) in row.ids.iter().zip(&row.type_ids).enumerate() {
                ids[base + t] = id as i64;
                mask[base + t] = 1;
                type_ids[base + t] = ty as i64;
            }
        }
        let shape = [batch, len];
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = Vec::new();
        for (name, kind) in &self.inputs {
            let data = match kind {
                InputKind::Ids => ids.clone(),
                InputKind::Mask => mask.clone(),
                InputKind::TypeIds => type_ids.clone(),
                InputKind::Positions => (0..batch)
                    .flat_map(|_| (0..len as i64).collect::<Vec<_>>())
                    .collect(),
            };
            let tensor = Tensor::from_array((shape, data))
                .map_err(|e| anyhow!("building the {name} tensor: {e}"))?;
            inputs.push((Cow::from(name.clone()), tensor.into_dyn().into()));
        }
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| anyhow!("running '{}': {e}", self.spec.id))?;
        let value = outputs
            .get(self.output.as_str())
            .ok_or_else(|| anyhow!("output '{}' missing", self.output))?;
        let (dims, data) = value
            .try_extract_raw_tensor::<f32>()
            .map_err(|e| anyhow!("reading the output of '{}': {e}", self.spec.id))?;
        let dims: Vec<usize> = dims.iter().map(|&d| d as usize).collect();
        pool(&dims, data, &mask, len, self.spec.pooling)
    }
}

/// Batches of indices into `encoded`: longest texts first, each batch as
/// large as both limits allow at the padded length of its first (longest)
/// text. A text longer than the token budget still gets a batch of its own.
pub(crate) fn plan_batches(encoded: &[Encoded], limits: BatchLimits) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..encoded.len()).collect();
    // Stable: equal lengths keep input order, so batches are deterministic.
    order.sort_by(|&a, &b| encoded[b].len().cmp(&encoded[a].len()));
    let mut batches = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut padded = 0;
    for i in order {
        if current.is_empty() {
            padded = encoded[i].len().max(1);
            current.push(i);
            continue;
        }
        let fits_tokens = (current.len() + 1) * padded <= limits.max_tokens;
        if current.len() < limits.max_texts.max(1) && fits_tokens {
            current.push(i);
        } else {
            batches.push(std::mem::take(&mut current));
            padded = encoded[i].len().max(1);
            current.push(i);
        }
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Pool a `[batch, len, dim]` (or already pooled `[batch, dim]`) output into
/// one unit-length vector per row.
fn pool(
    dims: &[usize],
    data: &[f32],
    mask: &[i64],
    len: usize,
    pooling: Pooling,
) -> Result<Vec<Vec<f32>>> {
    let mut vectors = match dims {
        [batch, dim] => (0..*batch)
            .map(|b| data[b * dim..(b + 1) * dim].to_vec())
            .collect::<Vec<_>>(),
        [batch, seq, dim] => {
            if *seq != len {
                bail!("model output has {seq} positions for a batch padded to {len}");
            }
            (0..*batch)
                .map(|b| {
                    let row = &data[b * seq * dim..(b + 1) * seq * dim];
                    match pooling {
                        Pooling::Cls => row[..*dim].to_vec(),
                        Pooling::Mean => {
                            let mut sum = vec![0f32; *dim];
                            let mut count = 0f32;
                            for t in 0..*seq {
                                if mask[b * len + t] == 0 {
                                    continue;
                                }
                                count += 1.0;
                                for (s, x) in sum.iter_mut().zip(&row[t * dim..(t + 1) * dim]) {
                                    *s += x;
                                }
                            }
                            let count = count.max(1.0);
                            sum.iter_mut().for_each(|s| *s /= count);
                            sum
                        }
                    }
                })
                .collect()
        }
        other => bail!("unexpected embedding output shape {other:?}"),
    };
    for v in &mut vectors {
        super::l2_normalize(v);
    }
    Ok(vectors)
}

/// The files of a model's pinned revision in the local cache.
pub(crate) struct ModelFiles {
    pub onnx: PathBuf,
    pub tokenizer: PathBuf,
    pub tokenizer_config: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub special_tokens_map: Option<PathBuf>,
}

impl ModelFiles {
    /// Every file `spec` needs, from the cache or — with `download` — from
    /// HuggingFace. Downloaded weights are checked against the pinned SHA-256;
    /// cached ones against the pinned size.
    pub fn resolve(spec: &EmbeddingModelSpec, cache: &Path, download: bool) -> Result<Self> {
        let fetch = |file: &str| fetch_file(spec, cache, file, download);
        let mut onnx = None;
        for weights in spec.weights() {
            let path = fetch(weights.path)?;
            verify_weights(spec, weights, &path)?;
            onnx.get_or_insert(path);
        }
        let onnx = onnx.expect("every model has an ONNX graph");
        let tokenizer = fetch("tokenizer.json")?;
        let optional = |file: &str| fetch(file).ok();
        Ok(Self {
            onnx,
            tokenizer,
            tokenizer_config: optional("tokenizer_config.json"),
            config: optional("config.json"),
            special_tokens_map: optional("special_tokens_map.json"),
        })
    }
}

/// Where a file of `spec`'s pinned revision lives in the HuggingFace cache
/// layout fastembed and `hf-hub` share (`models--org--name/snapshots/<rev>/`).
pub(crate) fn snapshot_path(spec: &EmbeddingModelSpec, cache: &Path, file: &str) -> PathBuf {
    cache
        .join(format!("models--{}", spec.repo.replace('/', "--")))
        .join("snapshots")
        .join(spec.revision)
        .join(file)
}

/// Whether every file `spec` needs to load is in `cache`.
pub(crate) fn is_cached(spec: &EmbeddingModelSpec, cache: &Path) -> bool {
    spec.weights()
        .map(|w| w.path)
        .chain(std::iter::once("tokenizer.json"))
        .all(|f| snapshot_path(spec, cache, f).is_file())
}

fn fetch_file(
    spec: &EmbeddingModelSpec,
    cache: &Path,
    file: &str,
    download: bool,
) -> Result<PathBuf> {
    let local = snapshot_path(spec, cache, file);
    if local.is_file() {
        return Ok(local);
    }
    if !download {
        bail!("{} is not in the model cache {}", file, cache.display());
    }
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_cache_dir(cache.to_path_buf())
        .with_endpoint(endpoint)
        .with_progress(true)
        .build()
        .context("setting up the HuggingFace client")?;
    let repo = api.repo(hf_hub::Repo::with_revision(
        spec.repo.to_string(),
        hf_hub::RepoType::Model,
        spec.revision.to_string(),
    ));
    let path = repo
        .get(file)
        .with_context(|| format!("downloading {file} of {}@{}", spec.repo, spec.revision))?;
    // hf-hub files a pinned commit under that commit's snapshot.
    Ok(if local.is_file() { local } else { path })
}

/// A weights file must have its pinned size; a freshly downloaded one (no
/// `.verified` marker yet) also its pinned SHA-256. A mismatch removes the
/// file so the next run downloads it again instead of loading a corrupt or
/// substituted graph.
fn verify_weights(spec: &EmbeddingModelSpec, weights: &WeightsFile, path: &Path) -> Result<()> {
    let (file, sha256, bytes) = (weights.path, weights.sha256, weights.bytes);
    let size = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let marker = path.with_file_name(format!(
        "{}.verified",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("weights")
    ));
    let bad = |why: String| -> Result<()> {
        remove_cached(path);
        bail!(
            "{file} of embedding model '{}' ({}@{}) {why}; the file was removed — run again to \
             download it",
            spec.id,
            spec.repo,
            spec.revision
        )
    };
    if size != bytes {
        return bad(format!("has {size} bytes, {bytes} expected"));
    }
    if marker.is_file() {
        return Ok(());
    }
    let mut hasher = Sha256::new();
    let mut reader = std::fs::File::open(path)?;
    std::io::copy(&mut reader, &mut hasher)?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != sha256 {
        return bad(format!("has SHA-256 {actual}, {sha256} expected"));
    }
    // Best effort: without the marker the next load just hashes again.
    let _ = std::fs::write(&marker, sha256);
    Ok(())
}

/// Remove a cached file and, for a snapshot symlink, the blob it points to.
fn remove_cached(path: &Path) {
    if let Ok(target) = std::fs::read_link(path) {
        let blob = path.parent().map(|p| p.join(&target)).unwrap_or(target);
        let _ = std::fs::remove_file(blob);
    }
    let _ = std::fs::remove_file(path);
}

/// The model's tokenizer twice: truncating at `window` with special tokens,
/// and plain for offsets; plus the padding id. Mirrors fastembed's loader:
/// the special tokens of `special_tokens_map.json` are registered, the pad
/// token comes from `tokenizer_config.json` (else `config.json`, else 0).
fn load_tokenizers(files: &ModelFiles, window: usize) -> Result<(Tokenizer, Tokenizer, u32)> {
    let mut base = Tokenizer::from_file(&files.tokenizer).map_err(|e| anyhow!("{e}"))?;
    base.with_padding(None);
    base.with_truncation(None).map_err(|e| anyhow!("{e}"))?;
    let read = |path: &Option<PathBuf>| -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(path.as_ref()?).ok()?;
        serde_json::from_str(&text).ok()
    };
    if let Some(serde_json::Value::Object(map)) = read(&files.special_tokens_map) {
        let tokens: Vec<AddedToken> = map
            .values()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(AddedToken::from(s.clone(), true)),
                serde_json::Value::Object(o) => o
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| AddedToken::from(c.to_string(), true)),
                _ => None,
            })
            .collect();
        base.add_special_tokens(&tokens);
    }
    let token_name = |v: &serde_json::Value| -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o.get("content")?.as_str().map(str::to_string),
            _ => None,
        }
    };
    let pad_id = read(&files.tokenizer_config)
        .and_then(|c| c.get("pad_token").and_then(token_name))
        .and_then(|t| base.token_to_id(&t))
        .or_else(|| {
            read(&files.config)
                .and_then(|c| c.get("pad_token_id")?.as_u64())
                .map(|id| id as u32)
        })
        .unwrap_or(0);
    let splitter = base.clone();
    let mut tokenizer = base;
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: window,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("{e}"))?;
    Ok((tokenizer, splitter, pad_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(len: usize) -> Encoded {
        Encoded {
            ids: vec![1; len],
            type_ids: vec![0; len],
        }
    }

    #[test]
    fn batches_sort_longest_first_and_respect_both_limits() {
        let encoded: Vec<Encoded> = [10, 500, 20, 500, 30, 5].iter().map(|&l| enc(l)).collect();
        let batches = plan_batches(
            &encoded,
            BatchLimits {
                max_texts: 3,
                max_tokens: 1000,
            },
        );
        // Two 500-token texts fill the 1000-token budget; the short ones
        // share a batch up to the text limit.
        assert_eq!(batches, vec![vec![1, 3], vec![4, 2, 0], vec![5]]);
        let all: usize = batches.iter().map(Vec::len).sum();
        assert_eq!(all, encoded.len());
    }

    #[test]
    fn an_oversized_text_gets_a_batch_of_its_own() {
        let encoded = vec![enc(4000), enc(10)];
        let batches = plan_batches(&encoded, BatchLimits::new(256, 1000));
        assert_eq!(batches, vec![vec![0], vec![1]]);
    }

    #[test]
    fn equal_lengths_keep_input_order() {
        let encoded: Vec<Encoded> = (0..5).map(|_| enc(7)).collect();
        let batches = plan_batches(&encoded, BatchLimits::new(2, 1000));
        assert_eq!(batches, vec![vec![0, 1], vec![2, 3], vec![4]]);
    }

    #[test]
    fn mean_pooling_ignores_padding_and_cls_takes_the_first_token() {
        // batch 2, len 3, dim 2; row 1 has one padding position.
        let data = [
            1.0, 0.0, 3.0, 0.0, 5.0, 0.0, // row 0: mean (3, 0)
            0.0, 2.0, 0.0, 4.0, 9.0, 9.0, // row 1: mean of first two = (0, 3)
        ];
        let mask = [1, 1, 1, 1, 1, 0];
        let mean = pool(&[2, 3, 2], &data, &mask, 3, Pooling::Mean).unwrap();
        assert_eq!(mean[0], vec![1.0, 0.0]);
        assert_eq!(mean[1], vec![0.0, 1.0]);
        let cls = pool(&[2, 3, 2], &data, &mask, 3, Pooling::Cls).unwrap();
        assert_eq!(cls[0], vec![1.0, 0.0]);
        assert_eq!(cls[1], vec![0.0, 1.0]);
        // An already pooled output is only normalized.
        let pooled = pool(&[1, 2], &[3.0, 4.0], &[1], 1, Pooling::Mean).unwrap();
        assert!((pooled[0][0] - 0.6).abs() < 1e-6 && (pooled[0][1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn thread_count_comes_from_the_environment_value() {
        assert_eq!(parse_threads(Some("3")), 3);
        let all = parse_threads(None);
        assert!(all >= 1);
        assert_eq!(parse_threads(Some("0")), all);
        assert_eq!(parse_threads(Some("many")), all);
    }
}
