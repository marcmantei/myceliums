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

use super::{get_cache_dir, EmbeddingModelSpec, PinnedFile, Pooling};
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

/// Token ids of one text, ready to be padded into a batch: nothing else of the
/// tokenizer's output (token strings, offsets, overflow) is kept.
#[derive(Debug, Clone)]
pub(crate) struct Encoded {
    pub ids: Vec<u32>,
    pub type_ids: Vec<u32>,
    /// Whether the window cut the text: its tail is in no vector.
    pub truncated: bool,
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

    /// Tokenize `texts` for the model: special tokens added, cut at the
    /// window (see [`encode_texts`]).
    pub fn encode(&self, texts: &[String]) -> Result<Vec<Encoded>> {
        let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
        encode_texts(&self.tokenizer, &self.splitter, &inputs, self.window)
            .map_err(|e| anyhow!("tokenizing for '{}': {e}", self.spec.id))
    }

    /// Embed already tokenized texts in token-budgeted batches, longest first;
    /// unit-length vectors in input order. `progress` is told how many texts
    /// are done after every batch; `stop` is asked before every batch, and a
    /// `true` ends the call with [`crate::AnalysisCancelled`].
    pub fn embed_encoded(
        &self,
        encoded: &[Encoded],
        limits: BatchLimits,
        progress: &mut dyn FnMut(usize),
        stop: &dyn Fn() -> bool,
    ) -> Result<Vec<Vec<f32>>> {
        let mut out: Vec<Option<Vec<f32>>> = vec![None; encoded.len()];
        let mut done = 0;
        for batch in plan_batches(encoded, limits) {
            if stop() {
                return Err(crate::AnalysisCancelled.into());
            }
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

/// Texts the tokenizer sees at once. Each group's tokenizer output is
/// reduced to [`Encoded`] before the next group is tokenized, so the memory
/// tokenization holds does not grow with the number of texts.
pub(crate) const ENCODE_GROUP: usize = 256;

/// Tokens past the window a long text keeps when it is cut before
/// tokenization: room for the tokenizer to truncate at exactly the token it
/// would have truncated the whole text at.
const CUT_MARGIN_TOKENS: usize = 16;

/// How far past the margin the cut looks for the end of a word.
const CUT_WORD_SEARCH_TOKENS: usize = 256;

/// Tokenize `texts` with `tokenizer` (special tokens, truncation at
/// `window`), keeping only the ids, the type ids and whether the window cut
/// the text.
///
/// A text that may be longer than the window is first cut with `splitter`
/// (the same tokenizer without special tokens or truncation) at the end of
/// the first word that ends [`CUT_MARGIN_TOKENS`] past the window. The
/// tokenizer then sees at most a word and a margin more than it keeps, so it
/// produces almost no overflow, instead of carrying the tokens of a whole
/// header chunk (up to 8 KB of text) as overflow. Words are tokenized
/// independently, so the kept tokens are those of the uncut text.
pub(crate) fn encode_texts(
    tokenizer: &Tokenizer,
    splitter: &Tokenizer,
    texts: &[&str],
    window: usize,
) -> Result<Vec<Encoded>> {
    let mut out = Vec::with_capacity(texts.len());
    for group in texts.chunks(ENCODE_GROUP) {
        let inputs = cut_for_window(splitter, group, window)?;
        let encodings = tokenizer
            .encode_batch_fast(inputs, true)
            .map_err(|e| anyhow!("{e}"))?;
        out.extend(encodings.into_iter().map(|e| Encoded {
            ids: e.get_ids().to_vec(),
            type_ids: e.get_type_ids().to_vec(),
            truncated: !e.get_overflowing().is_empty(),
        }));
    }
    Ok(out)
}

/// `group`, each text longer than `window` tokens cut after the first whole
/// word past `window` + [`CUT_MARGIN_TOKENS`] tokens.
fn cut_for_window<'a>(
    splitter: &Tokenizer,
    group: &[&'a str],
    window: usize,
) -> Result<Vec<&'a str>> {
    let mut cut = group.to_vec();
    // No tokenizer spends more than a token on a byte, so a text of at most
    // `window` bytes fits as it is.
    let long: Vec<usize> = (0..group.len())
        .filter(|&i| group[i].len() > window)
        .collect();
    if long.is_empty() {
        return Ok(cut);
    }
    let inputs: Vec<&str> = long.iter().map(|&i| group[i]).collect();
    let encodings = splitter
        .encode_batch(inputs, false)
        .map_err(|e| anyhow!("{e}"))?;
    for (&i, encoding) in long.iter().zip(&encodings) {
        if let Some(end) = window_cut(encoding, window + CUT_MARGIN_TOKENS) {
            cut[i] = prefix_on_char_boundary(group[i], end);
        }
    }
    Ok(cut)
}

/// The byte offset to cut a text at so that it keeps its first `keep`
/// tokens and ends with a whole word: the end of the last token before the
/// first word that starts at token `keep` or later. `None` when the text has
/// at most `keep` tokens. A word longer than [`CUT_WORD_SEARCH_TOKENS`] is
/// cut inside, well past the margin.
fn window_cut(encoding: &tokenizers::Encoding, keep: usize) -> Option<usize> {
    let offsets = encoding.get_offsets();
    if offsets.len() <= keep {
        return None;
    }
    let words = encoding.get_word_ids();
    let last = (keep + CUT_WORD_SEARCH_TOKENS).min(offsets.len());
    let boundary = (keep..last)
        .find(|&j| words[j].is_none() || words[j] != words[j - 1])
        .unwrap_or(last);
    Some(offsets[boundary - 1].1)
}

/// `text[..end]`, with `end` moved forward to a character boundary (a
/// byte-level token may end inside a multi-byte character).
fn prefix_on_char_boundary(text: &str, end: usize) -> &str {
    let mut end = end.min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    &text[..end]
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
    /// Every file `spec` pins, from the cache or (with `download`) from
    /// HuggingFace over HTTPS, each checked against its pinned size and
    /// SHA-256 ([`verify_file`]). A tokenizer file the registry pins as absent
    /// is not looked for: the loader's default applies, the same on every
    /// load, and nothing is fetched for it.
    pub fn resolve(spec: &EmbeddingModelSpec, cache: &Path, download: bool) -> Result<Self> {
        let fetch = |file: &PinnedFile| -> Result<PathBuf> {
            let path = fetch_file(spec, cache, file, download)?;
            verify_file(spec, file, &path)?;
            Ok(path)
        };
        let mut onnx = None;
        for weights in spec.weights() {
            let path = fetch(weights)?;
            onnx.get_or_insert(path);
        }
        let onnx = onnx.expect("every model has an ONNX graph");
        let pinned = &spec.tokenizer;
        let optional = |file: &Option<PinnedFile>| file.as_ref().map(fetch).transpose();
        Ok(Self {
            onnx,
            tokenizer: fetch(&pinned.tokenizer)?,
            tokenizer_config: optional(&pinned.tokenizer_config)?,
            config: optional(&pinned.config)?,
            special_tokens_map: optional(&pinned.special_tokens_map)?,
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

/// Whether every file `spec` pins is in `cache`.
pub(crate) fn is_cached(spec: &EmbeddingModelSpec, cache: &Path) -> bool {
    spec.files()
        .all(|f| snapshot_path(spec, cache, f.path).is_file())
}

/// The HuggingFace endpoint files are downloaded from: `HF_ENDPOINT` when set,
/// which must be an `https://` URL (the pinned hashes catch altered content,
/// but nothing should travel in the clear), else `https://huggingface.co`.
fn hf_endpoint(value: Option<String>) -> Result<String> {
    match value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => Ok("https://huggingface.co".to_string()),
        Some(url) if url.starts_with("https://") => Ok(url.trim_end_matches('/').to_string()),
        Some(url) => bail!(
            "HF_ENDPOINT={url} is not an https:// URL; model files are only downloaded over \
             HTTPS — set an https:// mirror or unset HF_ENDPOINT"
        ),
    }
}

fn fetch_file(
    spec: &EmbeddingModelSpec,
    cache: &Path,
    file: &PinnedFile,
    download: bool,
) -> Result<PathBuf> {
    let local = snapshot_path(spec, cache, file.path);
    if local.is_file() {
        return Ok(local);
    }
    if !download {
        bail!(
            "{} is not in the model cache {}",
            file.path,
            cache.display()
        );
    }
    let endpoint = hf_endpoint(std::env::var("HF_ENDPOINT").ok())?;
    let url = format!(
        "{endpoint}/{}/resolve/{}/{}",
        spec.repo, spec.revision, file.path
    );
    let status = |done: u64, total: Option<u64>| super::DownloadStatus {
        model: spec.id.to_string(),
        file: file.path.to_string(),
        done,
        total,
    };
    let downloaded = Downloader::new().and_then(|downloader| {
        downloader.fetch(
            &url,
            &local,
            file.bytes,
            &super::embedding_stopped,
            &mut |done, total| super::report_download(Some(status(done, total))),
        )
    });
    super::report_download(None);
    downloaded.with_context(|| {
        format!(
            "downloading {} of {}@{}",
            file.path, spec.repo, spec.revision
        )
    })?;
    Ok(local)
}

/// How a model file is downloaded: over HTTPS only — redirects included,
/// so a redirect to plain HTTP fails instead of being followed — through
/// the proxy of the environment, with the system's TLS (native-tls, as
/// hf-hub used).
pub(crate) struct Downloader {
    agent: ureq::Agent,
}

/// What one request of a download brought.
enum Attempt {
    Done,
    /// Start over from the first byte (the partial file was dropped).
    Restart,
}

impl Downloader {
    /// The downloader with the system's trust store.
    pub(crate) fn new() -> Result<Self> {
        Ok(Self::with_tls(native_tls::TlsConnector::new()?))
    }

    fn with_tls(tls: native_tls::TlsConnector) -> Self {
        let agent = ureq::builder()
            .https_only(true)
            .try_proxy_from_env(true)
            .tls_connector(std::sync::Arc::new(tls))
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(60))
            .build();
        Self { agent }
    }

    /// Download `url` to `target`, which must come out exactly `expected`
    /// bytes long (the pinned size; the pinned hash is checked by the
    /// caller).
    ///
    /// The bytes go to `<target>.part`, are synced and then renamed to
    /// `target`. An existing `.part` is resumed with a range request when
    /// the server answers with exactly that range, otherwise the download
    /// starts over. More bytes than `expected` abort it and remove the
    /// `.part`. `<target>.part.lock` (a PID lock) keeps two processes from
    /// writing one `.part`: a second one waits for the first, then uses what
    /// it downloaded. `stop` is asked between two reads (and while waiting):
    /// `true` leaves the `.part` for the next run and fails with
    /// [`crate::AnalysisCancelled`]. `progress` hears the bytes on disk and
    /// the size.
    pub(crate) fn fetch(
        &self,
        url: &str,
        target: &Path,
        expected: u64,
        stop: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        let part = target.with_file_name(format!("{name}.part"));
        let lock = target.with_file_name(format!("{name}.part.lock"));
        let _lock = loop {
            if stop() {
                return Err(crate::AnalysisCancelled.into());
            }
            match crate::AnalysisLock::acquire_file(lock.clone())? {
                crate::LockOutcome::Acquired(held) => break held,
                crate::LockOutcome::AlreadyRunning { .. } => {
                    std::thread::sleep(std::time::Duration::from_millis(200))
                }
            }
        };
        if target.is_file() {
            // Another process finished the download while this one waited.
            return Ok(());
        }
        for _ in 0..3 {
            match self.attempt(url, &part, expected, stop, progress)? {
                Attempt::Done => {
                    std::fs::rename(&part, target)
                        .with_context(|| format!("moving {} into place", part.display()))?;
                    return Ok(());
                }
                Attempt::Restart => continue,
            }
        }
        bail!("the server kept answering with other ranges than the one asked for")
    }

    /// One request: resume `part` or start it, and stream into it.
    fn attempt(
        &self,
        url: &str,
        part: &Path,
        expected: u64,
        stop: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Attempt> {
        use std::io::{Read, Write};
        let mut start = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if start > expected {
            std::fs::remove_file(part)?;
            start = 0;
        }
        let mut request = self
            .agent
            .get(url)
            .set(
                "User-Agent",
                concat!("myceliums/", env!("CARGO_PKG_VERSION")),
            )
            // Byte ranges and sizes of the file itself, not of a compressed
            // transfer.
            .set("Accept-Encoding", "identity");
        if start > 0 {
            request = request.set("Range", &format!("bytes={start}-"));
        }
        let response = match request.call() {
            // The part is complete already, or not of this file.
            Err(ureq::Error::Status(416, _)) if start > 0 => {
                std::fs::remove_file(part)?;
                return Ok(Attempt::Restart);
            }
            other => other.map_err(|e| anyhow!("{e}"))?,
        };
        let resumed = start > 0 && response.status() == 206;
        if resumed && content_range_start(response.header("Content-Range")) != Some(start) {
            // Not the range asked for: the part cannot be extended with it.
            std::fs::remove_file(part)?;
            return Ok(Attempt::Restart);
        }
        if !resumed {
            start = 0;
        }
        let length = response
            .header("Content-Length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        if let Some(length) = length.filter(|&l| start + l != expected) {
            bail!(
                "the server announced {} bytes, the pinned size is {expected}",
                start + length
            );
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(part)
            .with_context(|| format!("writing {}", part.display()))?;
        let mut reader = response.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = start;
        progress(done, Some(expected));
        loop {
            if stop() {
                out.flush()?;
                return Err(crate::AnalysisCancelled.into());
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if done + n as u64 > expected {
                drop(out);
                std::fs::remove_file(part)?;
                bail!("the server sent more than the pinned {expected} bytes");
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, Some(expected));
        }
        if done != expected {
            // Kept: the next run resumes it.
            out.flush()?;
            bail!("the download ended after {done} of {expected} bytes");
        }
        out.sync_all()
            .with_context(|| format!("syncing {}", part.display()))?;
        Ok(Attempt::Done)
    }
}

/// The first byte of a `Content-Range: bytes <first>-<last>/<size>` header.
fn content_range_start(header: Option<&str>) -> Option<u64> {
    let range = header?.trim().strip_prefix("bytes ")?;
    range.split('-').next()?.trim().parse().ok()
}

/// A pinned file must have its pinned size, checked on every load, and its
/// pinned SHA-256, checked whenever the file is new or changed since it was
/// last verified (a `.verified` marker records the hash and the file's
/// modification time). A mismatch removes the file, so the next run
/// downloads it again instead of loading a corrupt or substituted graph or
/// tokenizer.
fn verify_file(spec: &EmbeddingModelSpec, pinned: &PinnedFile, path: &Path) -> Result<()> {
    let (file, sha256, bytes) = (pinned.path, pinned.sha256, pinned.bytes);
    let metadata =
        std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    let size = metadata.len();
    let marker = path.with_file_name(format!(
        "{}.verified",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let bad = |why: String| -> Result<()> {
        remove_cached(path);
        let _ = std::fs::remove_file(&marker);
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
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stamp = format!("{sha256} {modified}");
    if std::fs::read_to_string(&marker).is_ok_and(|m| m == stamp) {
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
    let _ = std::fs::write(&marker, stamp);
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
            truncated: false,
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

    use std::collections::HashMap;
    use tokenizers::models::bpe::BPE;
    use tokenizers::models::wordlevel::WordLevel;
    use tokenizers::pre_tokenizers::whitespace::Whitespace;
    use tokenizers::processors::template::TemplateProcessing;

    /// `[CLS] … [SEP]` around every text, like the registry models.
    fn with_specials(mut base: Tokenizer, window: usize) -> (Tokenizer, Tokenizer) {
        base.with_pre_tokenizer(Some(Whitespace {}));
        base.with_post_processor(Some(
            TemplateProcessing::builder()
                .try_single("[CLS] $A [SEP]")
                .unwrap()
                .special_tokens(vec![("[CLS]", 1), ("[SEP]", 2)])
                .build()
                .unwrap(),
        ));
        let splitter = base.clone();
        let mut tokenizer = base;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: window,
                ..Default::default()
            }))
            .unwrap();
        (tokenizer, splitter)
    }

    /// One token per word or punctuation run.
    fn word_tokenizers(window: usize) -> (Tokenizer, Tokenizer) {
        let mut vocab: HashMap<String, u32> = [("[UNK]", 0), ("[CLS]", 1), ("[SEP]", 2)]
            .into_iter()
            .map(|(t, i)| (t.to_string(), i))
            .collect();
        for i in 0..6000u32 {
            vocab.insert(format!("w{i}"), 3 + i);
        }
        vocab.insert(",".to_string(), 7000);
        let model = WordLevel::builder()
            .vocab(vocab.into_iter().collect())
            .unk_token("[UNK]".to_string())
            .build()
            .unwrap();
        with_specials(Tokenizer::new(model), window)
    }

    /// One token per character: a word is as many tokens as it is long.
    fn char_tokenizers(window: usize) -> (Tokenizer, Tokenizer) {
        let mut vocab: HashMap<String, u32> = [("[UNK]", 0), ("[CLS]", 1), ("[SEP]", 2)]
            .into_iter()
            .map(|(t, i)| (t.to_string(), i))
            .collect();
        for (i, c) in ('a'..='z').enumerate() {
            vocab.insert(c.to_string(), 3 + i as u32);
        }
        let model = BPE::builder()
            .vocab_and_merges(
                vocab
                    .into_iter()
                    .collect::<tokenizers::models::bpe::Vocab>(),
                vec![],
            )
            .unk_token("[UNK]".to_string())
            .build()
            .unwrap();
        with_specials(Tokenizer::new(model), window)
    }

    fn words(n: usize) -> String {
        (0..n)
            .map(|i| {
                if i % 7 == 6 {
                    format!("w{i},")
                } else {
                    format!("w{i}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// What the cut changes: nothing the model sees. Every text tokenizes to
    /// the ids, type ids and truncation of the whole text.
    fn assert_same_as_whole(
        tokenizer: &Tokenizer,
        splitter: &Tokenizer,
        texts: &[String],
        window: usize,
    ) {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encoded = encode_texts(tokenizer, splitter, &refs, window).unwrap();
        assert_eq!(encoded.len(), texts.len());
        for (text, e) in texts.iter().zip(&encoded) {
            let whole = tokenizer.encode(text.as_str(), true).unwrap();
            assert_eq!(e.ids, whole.get_ids(), "{} bytes", text.len());
            assert_eq!(e.type_ids, whole.get_type_ids());
            assert_eq!(e.truncated, !whole.get_overflowing().is_empty());
            assert!(e.ids.len() <= window);
        }
    }

    #[test]
    fn texts_cut_before_tokenizing_keep_the_tokens_of_the_whole_text() {
        let window = 32;
        let (tokenizer, splitter) = word_tokenizers(window);
        let texts: Vec<String> = [0, 1, 5, 29, 30, 31, 45, 47, 48, 49, 60, 300, 5000]
            .iter()
            .map(|&n| words(n))
            .collect();
        assert_same_as_whole(&tokenizer, &splitter, &texts, window);
        // More texts than one group.
        let many: Vec<String> = (0..ENCODE_GROUP * 2 + 3).map(|i| words(i % 90)).collect();
        assert_same_as_whole(&tokenizer, &splitter, &many, window);
    }

    #[test]
    fn a_long_text_reaches_the_tokenizer_cut_just_past_the_window() {
        let window = 32;
        let (_, splitter) = word_tokenizers(window);
        let text = words(5000);
        let short = words(10);
        let cut = cut_for_window(&splitter, &[text.as_str(), short.as_str()], window).unwrap();
        let tokens = splitter.encode(cut[0], false).unwrap().len();
        assert_eq!(tokens, window + CUT_MARGIN_TOKENS, "one token per word");
        assert_eq!(cut[1], short, "a short text is not cut");
    }

    #[test]
    fn a_word_longer_than_the_search_is_cut_inside_it() {
        let window = 16;
        let (tokenizer, splitter) = char_tokenizers(window);
        let texts = vec![
            format!("ab {}", "c".repeat(2000)),
            format!("{} de", "f".repeat(window + 3)),
            "gh ij".repeat(400),
        ];
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let cut = cut_for_window(&splitter, &refs, window).unwrap();
        let kept = splitter.encode(cut[0], false).unwrap().len();
        assert_eq!(kept, window + CUT_MARGIN_TOKENS + CUT_WORD_SEARCH_TOKENS);
        assert_same_as_whole(&tokenizer, &splitter, &texts, window);
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        assert_eq!(prefix_on_char_boundary("aé", 2), "aé");
        assert_eq!(prefix_on_char_boundary("aé", 1), "a");
        assert_eq!(prefix_on_char_boundary("aé", 99), "aé");
    }

    #[test]
    fn downloads_go_over_https_only() {
        assert_eq!(hf_endpoint(None).unwrap(), "https://huggingface.co");
        assert_eq!(
            hf_endpoint(Some(" ".into())).unwrap(),
            "https://huggingface.co"
        );
        assert_eq!(
            hf_endpoint(Some("https://mirror.example/".into())).unwrap(),
            "https://mirror.example"
        );
        let err = hf_endpoint(Some("http://mirror.example".into())).unwrap_err();
        assert!(err.to_string().contains("https://"), "{err}");
    }

    /// What a test server was asked: the path and the first byte of the
    /// range, if any.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Asked {
        path: String,
        range: Option<u64>,
    }

    /// What a test server answers: status line, headers, body. Without a
    /// `Content-Length` header the body ends when the connection closes.
    struct Reply {
        status: &'static str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    type Handler = std::sync::Arc<dyn Fn(&Asked) -> Reply + Send + Sync>;

    /// A certificate authority and a server certificate for 127.0.0.1
    /// signed by it: the authority is the client's only root.
    fn test_certificates() -> (
        openssl::x509::X509,
        openssl::x509::X509,
        openssl::pkey::PKey<openssl::pkey::Private>,
    ) {
        use openssl::asn1::Asn1Time;
        use openssl::bn::BigNum;
        use openssl::ec::{EcGroup, EcKey};
        use openssl::hash::MessageDigest;
        use openssl::nid::Nid;
        use openssl::pkey::PKey;
        use openssl::x509::extension::{
            BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
        };
        use openssl::x509::{X509NameBuilder, X509};
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let key = || PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let name = |cn: &str| {
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", cn).unwrap();
            name.build()
        };
        let builder = |serial: u32| {
            let mut b = X509::builder().unwrap();
            b.set_version(2).unwrap();
            b.set_serial_number(&BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap())
                .unwrap();
            b.set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            b.set_not_after(&Asn1Time::days_from_now(2).unwrap())
                .unwrap();
            b
        };
        let ca_key = key();
        let mut ca = builder(1);
        ca.set_subject_name(&name("myceliums test CA")).unwrap();
        ca.set_issuer_name(&name("myceliums test CA")).unwrap();
        ca.set_pubkey(&ca_key).unwrap();
        ca.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        ca.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
        ca.sign(&ca_key, MessageDigest::sha256()).unwrap();
        let ca = ca.build();

        let server_key = key();
        let mut server = builder(2);
        server.set_subject_name(&name("127.0.0.1")).unwrap();
        server.set_issuer_name(ca.subject_name()).unwrap();
        server.set_pubkey(&server_key).unwrap();
        server
            .append_extension(BasicConstraints::new().critical().build().unwrap())
            .unwrap();
        server
            .append_extension(
                KeyUsage::new()
                    .critical()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
        server
            .append_extension(ExtendedKeyUsage::new().server_auth().build().unwrap())
            .unwrap();
        let san = SubjectAlternativeName::new()
            .ip("127.0.0.1")
            .dns("localhost")
            .build(&server.x509v3_context(Some(&ca), None))
            .unwrap();
        server.append_extension(san).unwrap();
        server.sign(&ca_key, MessageDigest::sha256()).unwrap();
        (ca, server.build(), server_key)
    }

    /// An HTTPS server on 127.0.0.1 answering every request with `handler`;
    /// its URL for `path`, a downloader that trusts it, and what it was
    /// asked.
    fn https_server(
        path: &str,
        handler: Handler,
    ) -> (
        String,
        Downloader,
        std::sync::Arc<std::sync::Mutex<Vec<Asked>>>,
    ) {
        use openssl::ssl::{SslAcceptor, SslMethod};
        use std::io::{BufRead, BufReader, Write};
        let (ca, cert, key) = test_certificates();
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_certificate(&cert).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.check_private_key().unwrap();
        let acceptor = std::sync::Arc::new(acceptor.build());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "https://127.0.0.1:{}{path}",
            listener.local_addr().unwrap().port()
        );
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for tcp in listener.incoming() {
                let Ok(tcp) = tcp else { continue };
                let (acceptor, handler, log) = (acceptor.clone(), handler.clone(), log.clone());
                std::thread::spawn(move || {
                    let Ok(mut stream) = acceptor.accept(tcp) else {
                        return;
                    };
                    let mut request = Asked {
                        path: String::new(),
                        range: None,
                    };
                    {
                        let mut reader = BufReader::new(&mut stream);
                        let mut first = true;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                            if first {
                                request.path = line.split(' ').nth(1).unwrap_or("").to_string();
                                first = false;
                            }
                            let lower = line.to_ascii_lowercase();
                            if let Some(range) = lower.strip_prefix("range: bytes=") {
                                request.range = range.trim().trim_end_matches('-').parse().ok();
                            }
                        }
                    }
                    log.lock().unwrap().push(request.clone());
                    let reply = handler(&request);
                    let mut head = format!("HTTP/1.1 {}\r\nConnection: close\r\n", reply.status);
                    for (name, value) in &reply.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    if stream.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    for piece in reply.body.chunks(16 * 1024) {
                        if stream.write_all(piece).is_err() {
                            return;
                        }
                    }
                    let _ = stream.shutdown();
                });
            }
        });
        let tls = native_tls::TlsConnector::builder()
            .add_root_certificate(native_tls::Certificate::from_der(&ca.to_der().unwrap()).unwrap())
            .build()
            .unwrap();
        (url, Downloader::with_tls(tls), asked)
    }

    /// Serves `content` with range requests answered correctly.
    fn file_handler(content: Vec<u8>) -> Handler {
        std::sync::Arc::new(move |asked: &Asked| {
            let len = content.len() as u64;
            match asked.range {
                Some(from) => Reply {
                    status: "206 Partial Content",
                    headers: vec![
                        ("Content-Length".into(), (len - from).to_string()),
                        (
                            "Content-Range".into(),
                            format!("bytes {from}-{}/{len}", len - 1),
                        ),
                    ],
                    body: content[from as usize..].to_vec(),
                },
                None => Reply {
                    status: "200 OK",
                    headers: vec![("Content-Length".into(), len.to_string())],
                    body: content.clone(),
                },
            }
        })
    }

    fn content(len: u32) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (
            dir.join("snapshots/rev/onnx/model.onnx"),
            dir.join("snapshots/rev/onnx/model.onnx.part"),
        )
    }

    /// A download that is stopped keeps its partial file, and the next one
    /// resumes it with a range request; the result is the whole file.
    #[test]
    fn a_stopped_download_resumes_where_it_stopped() {
        let body = content(1_000_000);
        let (url, downloader, asked) = https_server("/model.onnx", file_handler(body.clone()));
        let dir = tempfile::tempdir().unwrap();
        let (target, part) = paths(dir.path());

        let reached = std::cell::Cell::new(0u64);
        let err = downloader
            .fetch(
                &url,
                &target,
                1_000_000,
                &|| reached.get() >= 100_000,
                &mut |done, total| {
                    assert_eq!(total, Some(1_000_000));
                    reached.set(done);
                },
            )
            .unwrap_err();
        assert!(err.is::<crate::AnalysisCancelled>(), "{err}");
        assert!(!target.exists());
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!((100_000..1_000_000).contains(&kept), "{kept}");

        let mut last = (0, None);
        downloader
            .fetch(&url, &target, 1_000_000, &|| false, &mut |done, total| {
                last = (done, total)
            })
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        assert!(!part.exists());
        assert_eq!(last, (1_000_000, Some(1_000_000)));
        let ranges: Vec<Option<u64>> = asked.lock().unwrap().iter().map(|a| a.range).collect();
        assert_eq!(
            ranges,
            vec![None, Some(kept)],
            "resumed at the partial size"
        );
    }

    /// The client follows no redirect from HTTPS to plain HTTP: the
    /// download fails without a request reaching the HTTP server.
    #[test]
    fn a_redirect_to_plain_http_is_refused() {
        let plain = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let plain_port = plain.local_addr().unwrap().port();
        plain.set_nonblocking(true).unwrap();
        let location = format!("http://127.0.0.1:{plain_port}/model.onnx");
        let (url, downloader, asked) = https_server(
            "/model.onnx",
            std::sync::Arc::new(move |_: &Asked| Reply {
                status: "302 Found",
                headers: vec![
                    ("Location".into(), location.clone()),
                    ("Content-Length".into(), "0".into()),
                ],
                body: vec![],
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        let (target, part) = paths(dir.path());
        let err = downloader
            .fetch(&url, &target, 1_000, &|| false, &mut |_, _| {})
            .unwrap_err();
        let message = format!("{err:#}").to_lowercase();
        assert!(
            message.contains("https") || message.contains("insecure"),
            "{message}"
        );
        assert_eq!(asked.lock().unwrap().len(), 1, "the redirect was served");
        assert!(
            plain.accept().is_err(),
            "nothing reached the plain HTTP server"
        );
        assert!(!target.exists() && !part.exists());
    }

    /// A server that sends more than the pinned size — here without a
    /// length, until it closes the connection — is cut off, and its bytes
    /// are removed.
    #[test]
    fn a_download_longer_than_the_pinned_size_is_aborted() {
        let body = content(600_000);
        let (url, downloader, _) = https_server(
            "/model.onnx",
            std::sync::Arc::new(move |_: &Asked| Reply {
                status: "200 OK",
                headers: vec![],
                body: body.clone(),
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        let (target, part) = paths(dir.path());
        let err = downloader
            .fetch(&url, &target, 300_000, &|| false, &mut |_, _| {})
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("more than the pinned"),
            "{err:#}"
        );
        assert!(!target.exists() && !part.exists());

        // An announced length other than the pinned size fails at once.
        let (url, downloader, _) = https_server("/model.onnx", file_handler(content(1_000)));
        let err = downloader
            .fetch(&url, &target, 999, &|| false, &mut |_, _| {})
            .unwrap_err();
        assert!(format!("{err:#}").contains("pinned size"), "{err:#}");
    }

    /// A 206 for another range than the one asked for cannot extend the
    /// partial file: it is dropped and the download starts over.
    #[test]
    fn a_partial_answer_for_another_range_restarts_the_download() {
        let body = content(50_000);
        let served = body.clone();
        let (url, downloader, asked) = https_server(
            "/model.onnx",
            std::sync::Arc::new(move |asked: &Asked| {
                let len = served.len();
                match asked.range {
                    // Ignores the range but says 206.
                    Some(_) => Reply {
                        status: "206 Partial Content",
                        headers: vec![
                            ("Content-Length".into(), len.to_string()),
                            ("Content-Range".into(), format!("bytes 0-{}/{len}", len - 1)),
                        ],
                        body: served.clone(),
                    },
                    None => Reply {
                        status: "200 OK",
                        headers: vec![("Content-Length".into(), len.to_string())],
                        body: served.clone(),
                    },
                }
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        let (target, part) = paths(dir.path());
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, vec![7u8; 1_000]).unwrap();
        downloader
            .fetch(&url, &target, 50_000, &|| false, &mut |_, _| {})
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        let ranges: Vec<Option<u64>> = asked.lock().unwrap().iter().map(|a| a.range).collect();
        assert_eq!(ranges, vec![Some(1_000), None]);
    }

    /// A download another process holds the lock of is waited for, and what
    /// that process downloaded is used: no second request.
    #[test]
    fn a_download_in_another_process_is_waited_for() {
        let (url, downloader, asked) = https_server("/model.onnx", file_handler(content(10)));
        let dir = tempfile::tempdir().unwrap();
        let (target, _) = paths(dir.path());
        let lock = target.with_file_name("model.onnx.part.lock");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        // A live process holds it: this one.
        std::fs::write(&lock, format!("{}\nnow", std::process::id())).unwrap();
        let (finished, release) = (target.clone(), lock.clone());
        let other = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            std::fs::write(&finished, content(10)).unwrap();
            std::fs::remove_file(&release).unwrap();
        });
        downloader
            .fetch(&url, &target, 10, &|| false, &mut |_, _| {})
            .unwrap();
        other.join().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), content(10));
        assert!(
            asked.lock().unwrap().is_empty(),
            "nothing was downloaded twice"
        );
        assert!(!lock.exists());
    }

    #[test]
    fn content_ranges_are_read_from_their_first_byte() {
        assert_eq!(content_range_start(Some("bytes 100-999/1000")), Some(100));
        assert_eq!(content_range_start(Some("bytes 0-9/*")), Some(0));
        assert_eq!(content_range_start(Some("items 1-2/3")), None);
        assert_eq!(content_range_start(None), None);
    }

    /// The downloader against HuggingFace itself: a file stored in git and
    /// one stored in LFS (served through a redirect to the CDN), each with
    /// its pinned size and SHA-256. Needs the network:
    /// `cargo test -p myceliums-core downloads_pinned_files -- --ignored`
    #[test]
    #[ignore]
    fn downloads_pinned_files_from_huggingface() {
        let spec =
            super::super::embedding_model_spec("granite-embedding-small-english-r2").unwrap();
        let cache = tempfile::tempdir().unwrap();
        for file in [spec.tokenizer.config.as_ref().unwrap(), &spec.onnx] {
            let path = fetch_file(spec, cache.path(), file, true).unwrap();
            verify_file(spec, file, &path).unwrap();
            println!("{}: {} bytes, hash ok", file.path, file.bytes);
        }
    }

    fn pinned_for(content: &[u8]) -> PinnedFile {
        let sha = format!("{:x}", Sha256::digest(content));
        PinnedFile {
            path: "tokenizer.json",
            bytes: content.len() as u64,
            // A registry pin lives forever; so may this one.
            sha256: Box::leak(sha.into_boxed_str()),
        }
    }

    #[test]
    fn every_pinned_file_must_have_its_size_and_hash() {
        let spec = &super::super::EMBEDDING_MODELS[0];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokenizer.json");
        let marker = dir.path().join("tokenizer.json.verified");

        std::fs::write(&path, b"{\"pinned\":1}").unwrap();
        let pinned = pinned_for(b"{\"pinned\":1}");
        verify_file(spec, &pinned, &path).expect("the pinned content passes");
        assert!(marker.is_file(), "the verification is remembered");
        verify_file(spec, &pinned, &path).expect("and passes again");

        // Same size, other content, a marker from another file version: the
        // hash is checked again and the file is removed.
        std::fs::write(&path, b"{\"pinned\":2}").unwrap();
        std::fs::write(&marker, format!("{} 0", pinned.sha256)).unwrap();
        let err = verify_file(spec, &pinned, &path).unwrap_err();
        assert!(err.to_string().contains("SHA-256"), "{err}");
        assert!(!path.exists() && !marker.exists());

        std::fs::write(&path, b"{}").unwrap();
        let err = verify_file(spec, &pinned, &path).unwrap_err();
        assert!(err.to_string().contains("bytes"), "{err}");
        assert!(!path.exists());
    }

    #[test]
    fn a_model_is_cached_only_with_every_pinned_file() {
        let spec = super::super::embedding_model_spec("multilingual-e5-small").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut files: Vec<&PinnedFile> = spec.files().collect();
        assert_eq!(
            files.len(),
            5,
            "graph, tokenizer and three configuration files"
        );
        let last = files.pop().unwrap();
        for file in files {
            let path = snapshot_path(spec, cache.path(), file.path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        assert!(!is_cached(spec, cache.path()), "{} is missing", last.path);
        std::fs::write(snapshot_path(spec, cache.path(), last.path), b"").unwrap();
        assert!(is_cached(spec, cache.path()));
    }

    /// The cut against real tokenizers: every registry model's
    /// `tokenizer.json` (and configuration files) in
    /// `$MYC_TOKENIZER_DIR/<model id>/`, texts from this repository's sources
    /// cut at 8 KB like a header chunk.
    /// `MYC_TOKENIZER_DIR=... cargo test -p myceliums-core cut_matches_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cut_matches_real_tokenizers() {
        let dir = PathBuf::from(std::env::var("MYC_TOKENIZER_DIR").expect("MYC_TOKENIZER_DIR"));
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut texts = Vec::new();
        for entry in walkdir::WalkDir::new(&sources).into_iter().flatten() {
            if entry.path().extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(entry.path()).unwrap();
                texts.push(super::super::text::cut(&text, 8192).to_string());
                texts.push(super::super::text::cut(&text, 700).to_string());
            }
        }
        for spec in super::super::EMBEDDING_MODELS {
            let model_dir = dir.join(spec.id);
            let optional = |name: &str| Some(model_dir.join(name)).filter(|p| p.is_file());
            let files = ModelFiles {
                onnx: PathBuf::new(),
                tokenizer: model_dir.join("tokenizer.json"),
                tokenizer_config: optional("tokenizer_config.json"),
                config: optional("config.json"),
                special_tokens_map: optional("special_tokens_map.json"),
            };
            let (tokenizer, splitter, _) = load_tokenizers(&files, spec.window_tokens).unwrap();
            assert_same_as_whole(&tokenizer, &splitter, &texts, spec.window_tokens);
            println!("{}: {} texts, ids identical", spec.id, texts.len());
        }
    }

    /// This process's peak resident memory so far, in MB.
    #[cfg(unix)]
    fn peak_rss_mb() -> f64 {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: getrusage writes into the zeroed struct we own.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        let raw = usage.ru_maxrss as f64;
        // Bytes on macOS, kilobytes on Linux.
        if cfg!(target_os = "macos") {
            raw / 1e6
        } else {
            raw / 1e3
        }
    }

    #[cfg(unix)]
    fn large_corpus() -> (Tokenizer, Tokenizer, Vec<String>) {
        let path = std::env::var("MYC_TOKENIZER").expect("MYC_TOKENIZER");
        let mut base = Tokenizer::from_file(path).unwrap();
        base.with_padding(None);
        base.with_truncation(None).unwrap();
        let splitter = base.clone();
        let mut tokenizer = base;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 512,
                ..Default::default()
            }))
            .unwrap();
        let line = "    let value = compute_the_next_value(previous, &config.limits)?; // why\n";
        let texts = (0..20_000)
            .map(|i| format!("fn f{i}() {{\n{}", line.repeat(8192 / line.len())))
            .collect();
        (tokenizer, splitter, texts)
    }

    /// Peak memory of tokenizing a large synthetic corpus (20,000 texts of
    /// 8 KB, the header-chunk limit) the way indexing does, in groups with
    /// the cut, against all at once as before. Peak memory only grows, so run
    /// each in its own process:
    /// `MYC_TOKENIZER=<tokenizer.json> cargo test -p myceliums-core --lib -- --ignored --nocapture --exact embeddings::onnx::tests::tokenizing_a_large_corpus_in_groups`
    /// (and `..._at_once`).
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn tokenizing_a_large_corpus_in_groups() {
        let (tokenizer, splitter, texts) = large_corpus();
        let before = peak_rss_mb();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encoded = encode_texts(&tokenizer, &splitter, &refs, 512).unwrap();
        let ids: usize = encoded.iter().map(|e| e.ids.len()).sum();
        println!(
            "in groups of {ENCODE_GROUP}: peak RSS {before:.0} MB -> {:.0} MB ({ids} ids kept)",
            peak_rss_mb()
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore]
    fn tokenizing_a_large_corpus_at_once() {
        let (tokenizer, _, texts) = large_corpus();
        let before = peak_rss_mb();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encodings = tokenizer.encode_batch_fast(refs, true).unwrap();
        let ids: usize = encodings.iter().map(|e| e.get_ids().len()).sum();
        println!(
            "all at once: peak RSS {before:.0} MB -> {:.0} MB ({ids} ids kept)",
            peak_rss_mb()
        );
    }
}
