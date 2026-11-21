//! Splitting long symbols into overlapping, model-window-sized chunks (#157).
//!
//! One vector per symbol cut at the model's window loses the end of every long
//! function. With chunking a symbol is embedded as:
//!
//! 1. its **header chunk** — the whole document (template 2), cut at the window
//!    by the tokenizer; this is the symbol's own vector, stored on its row;
//! 2. **body chunks** — consecutive windows over the body tokens the header
//!    chunk did not reach, each overlapping the previous by
//!    [`Chunking::overlap_tokens`] and starting with the symbol's context line
//!    (`language kind file::qualified.name`), so a chunk still says whose code
//!    it is.
//!
//! Boundaries are counted in the model's own tokens, so a chunk fills the window
//! without being truncated. A symbol is scored by the best of its vectors and
//! search returns one row per symbol (see `Store::vector_search`).

use super::text::{cut, Document};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

/// How long symbols are split. Recorded in the index and part of its
/// fingerprint: other parameters produce other vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunking {
    /// Tokens per embedded text, special tokens included.
    pub window_tokens: u32,
    /// Body tokens a chunk repeats from the end of the previous one.
    pub overlap_tokens: u32,
    /// Most vectors per symbol, header chunk included.
    pub max_chunks: u32,
}

impl Chunking {
    /// Overlap between consecutive chunks (#157; fixed, not tuned).
    pub const DEFAULT_OVERLAP: u32 = 64;
    /// Most vectors per symbol (#157; fixed, not tuned): 16 windows of 512
    /// tokens reach several hundred lines; longer bodies keep their tail out
    /// of the vectors (BM25 still indexes all of it).
    pub const DEFAULT_MAX_CHUNKS: u32 = 16;

    /// The shipped parameters for a model whose window is `window` tokens.
    pub fn for_window(window: usize) -> Self {
        Self {
            window_tokens: window as u32,
            overlap_tokens: Self::DEFAULT_OVERLAP,
            max_chunks: Self::DEFAULT_MAX_CHUNKS,
        }
    }

    /// `window/overlap/max`, as the fingerprint and reports spell it.
    pub fn label(&self) -> String {
        format!(
            "{}/{}/{}",
            self.window_tokens, self.overlap_tokens, self.max_chunks
        )
    }
}

/// Tokens of slack between the counted budget and the window: a substring can
/// tokenize a token or two longer than the slice of the whole it came from.
const MARGIN_TOKENS: usize = 4;

/// Bytes the tokenizer is given per window token at most: no tokenizer spends
/// fewer than this many tokens on 16 bytes of code, so the cut never shortens
/// what the window would have held, and a megabyte of generated code is not
/// tokenized only to be thrown away.
pub(crate) const BYTES_PER_WINDOW_TOKEN: usize = 16;

/// What shares a model's window with the text of a chunk.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Window {
    /// Tokens per embedded text, special tokens included.
    pub tokens: usize,
    /// Special tokens the tokenizer adds to every text.
    pub special_tokens: usize,
    /// Tokens of the model's passage prefix.
    pub prefix_tokens: usize,
}

/// The texts `doc` is embedded as: the header chunk first, then any body
/// chunks. `splitter` is the model's tokenizer without truncation or special
/// tokens.
pub(crate) fn chunk_texts(
    doc: &Document,
    chunking: Option<Chunking>,
    splitter: &Tokenizer,
    window: Window,
) -> Result<Vec<String>> {
    let header_chunk = cut(&doc.text(), window.tokens * BYTES_PER_WINDOW_TOKEN).to_string();
    let Some(chunking) = chunking else {
        return Ok(vec![header_chunk]);
    };
    if !doc.chunkable || doc.body.is_empty() || chunking.max_chunks <= 1 {
        return Ok(vec![header_chunk]);
    }
    let count = |text: &str| -> Result<usize> {
        Ok(splitter
            .encode_fast(text, false)
            .map_err(|e| anyhow!("tokenizing for chunking: {e}"))?
            .len())
    };
    let budget = window
        .tokens
        .saturating_sub(window.special_tokens + window.prefix_tokens + MARGIN_TOKENS);
    let overlap = chunking.overlap_tokens as usize;
    let max_chunks = chunking.max_chunks as usize;

    let header_tokens = count(&format!("{}\n", doc.header))?;
    let body = cut(&doc.body, budget * max_chunks * BYTES_PER_WINDOW_TOKEN);
    let encoding = splitter
        .encode(body, false)
        .map_err(|e| anyhow!("tokenizing a body for chunking: {e}"))?;
    let offsets = encoding.get_offsets();
    let body_tokens = offsets.len();
    if header_tokens + body_tokens <= budget {
        return Ok(vec![header_chunk]);
    }

    let context_tokens = count(&format!("{}\n", doc.context))?;
    let per_chunk = budget.saturating_sub(context_tokens);
    if per_chunk <= 2 * overlap {
        // The context line leaves no room for a useful window.
        return Ok(vec![header_chunk]);
    }
    let mut texts = vec![header_chunk];
    let covered = budget.saturating_sub(header_tokens);
    let mut start = covered.saturating_sub(overlap);
    while texts.len() < max_chunks && start < body_tokens {
        let end = (start + per_chunk).min(body_tokens);
        let from = offsets[start].0;
        let to = offsets[end - 1].1.max(from);
        let slice = slice_on_boundaries(body, from, to);
        if !slice.trim().is_empty() {
            texts.push(format!("{}\n{}", doc.context, slice));
        }
        if end == body_tokens {
            break;
        }
        start = end - overlap;
    }
    Ok(texts)
}

/// `text[from..to]`, widened to the nearest character boundaries — byte-level
/// tokenizers may cut a multi-byte character between two tokens.
fn slice_on_boundaries(text: &str, from: usize, to: usize) -> &str {
    let mut from = from.min(text.len());
    while from > 0 && !text.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = to.clamp(from, text.len());
    while to < text.len() && !text.is_char_boundary(to) {
        to += 1;
    }
    &text[from..to]
}

/// The key a chunk vector is stored and reused under: SHA-256 of the text the
/// model embedded (without the passage prefix, which the fingerprint covers).
pub fn text_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokenizers::models::wordlevel::WordLevel;
    use tokenizers::pre_tokenizers::whitespace::Whitespace;

    /// One token per whitespace-separated word or punctuation run.
    fn words() -> Tokenizer {
        let model = WordLevel::builder()
            .unk_token("[UNK]".to_string())
            .vocab([("[UNK]".to_string(), 0)].into_iter().collect())
            .build()
            .unwrap();
        let mut tokenizer = Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(Whitespace {}));
        tokenizer
    }

    fn doc(body_words: usize, chunkable: bool) -> Document {
        let body = (0..body_words)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        Document {
            context: "rust function a.rs::f".to_string(),
            header: "rust function a.rs::f\nfn f()".to_string(),
            body,
            chunkable,
        }
    }

    const WINDOW: Window = Window {
        tokens: 40,
        special_tokens: 2,
        prefix_tokens: 0,
    };

    fn chunking(overlap: u32, max_chunks: u32) -> Option<Chunking> {
        Some(Chunking {
            window_tokens: 40,
            overlap_tokens: overlap,
            max_chunks,
        })
    }

    #[test]
    fn a_short_symbol_is_one_text() {
        let texts = chunk_texts(&doc(10, true), chunking(4, 16), &words(), WINDOW).unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], doc(10, true).text());
    }

    #[test]
    fn a_long_body_continues_where_the_header_chunk_stops_with_overlap() {
        // Budget 40 - 2 - 4 = 34 tokens. The header is 8 tokens ("rust",
        // "function", "a", ".", "rs", "::", "f", "fn", "f", "()" = 10 with
        // punctuation runs), so the header chunk reaches body word 24; body
        // chunks carry the 7-token context line and 27 body words each.
        let texts = chunk_texts(&doc(100, true), chunking(4, 16), &words(), WINDOW).unwrap();
        assert!(texts.len() > 2, "{texts:#?}");
        let first_body = &texts[1];
        assert!(first_body.starts_with("rust function a.rs::f\n"));
        // Every body word is in some chunk, and consecutive chunks overlap.
        for i in 0..100 {
            let word = format!("w{i}");
            assert!(
                texts
                    .iter()
                    .any(|t| t.split_whitespace().any(|w| w == word)),
                "{word} missing"
            );
        }
        let last_of = |t: &str| t.split_whitespace().last().unwrap().to_string();
        let first_word_of = |t: &str| {
            t.lines()
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_string()
        };
        for pair in texts[1..].windows(2) {
            let prev_last: usize = last_of(&pair[0])[1..].parse().unwrap();
            let next_first: usize = first_word_of(&pair[1])[1..].parse().unwrap();
            assert_eq!(prev_last + 1 - next_first, 4, "overlap of 4 words");
        }
    }

    #[test]
    fn chunks_stop_at_the_cap_and_containers_are_never_split() {
        let capped = chunk_texts(&doc(1000, true), chunking(4, 3), &words(), WINDOW).unwrap();
        assert_eq!(capped.len(), 3);
        let container = chunk_texts(&doc(1000, false), chunking(4, 16), &words(), WINDOW).unwrap();
        assert_eq!(container.len(), 1);
        let off = chunk_texts(&doc(1000, true), None, &words(), WINDOW).unwrap();
        assert_eq!(off.len(), 1);
    }

    #[test]
    fn labels_spell_every_parameter() {
        assert_eq!(Chunking::for_window(512).label(), "512/64/16");
    }

    #[test]
    fn slices_widen_to_char_boundaries() {
        let text = "aé b";
        // 'é' is bytes 1..3: a cut inside it widens outwards.
        assert_eq!(slice_on_boundaries(text, 2, 2), "é");
        assert_eq!(slice_on_boundaries(text, 0, 2), "aé");
        assert_eq!(slice_on_boundaries(text, 3, 99), " b");
    }

    #[test]
    fn hashes_are_stable_hex() {
        assert_eq!(text_hash("abc").len(), 64);
        assert_eq!(text_hash("abc"), text_hash("abc"));
        assert_ne!(text_hash("abc"), text_hash("abd"));
    }
}
