//! A contiguous byte arena for interning strings.
//!
//! All strings are stored in a single `Vec<u8>` buffer.
//! Lookups via [`StrId`] avoid pointer chasing and improve cache locality.
//! Duplicate strings are detected via a simple FxHash-style hash and
//! return the same [`StrId`], giving amortised O(1) intern performance.
//!
//! Integration with [`crate::resolver::CallResolver`] will follow in a
//! separate PR to avoid merge conflicts with in-flight changes.

use std::collections::HashMap;

/// Opaque handle into the [`StringPool`] buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(pub u32);

/// A contiguous byte arena for interning strings.
pub struct StringPool {
    /// Backing store: all interned strings are concatenated here.
    buffer: Vec<u8>,
    /// `(start, len)` pairs — one per interned string.
    offsets: Vec<(u32, u32)>,
    /// FxHash of string content -> StrId, used for dedup.
    index: HashMap<u64, StrId>,
}

/// Simple FxHash-style hash (no external dependency).
fn fx_hash(bytes: &[u8]) -> u64 {
    const SEED: u64 = 0x517c_c1b7_2722_0a95;
    let mut hash: u64 = 0;
    for &b in bytes {
        hash = hash.wrapping_mul(SEED) ^ u64::from(b);
    }
    hash
}

impl Default for StringPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StringPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            offsets: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Create a pool with pre-allocated buffer capacity (in bytes).
    #[allow(dead_code)]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(cap),
            offsets: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Intern a string. Returns an existing [`StrId`] if the string was
    /// already interned, or appends it to the buffer and returns a new one.
    pub fn intern(&mut self, s: &str) -> StrId {
        let hash = fx_hash(s.as_bytes());

        // Fast path: already interned.
        if let Some(&id) = self.index.get(&hash) {
            // Verify it is actually the same string (hash collision guard).
            if self.get(id) == s {
                return id;
            }
            // On collision, fall through and store under a new id.
            // The index only caches the *first* string for a given hash,
            // so subsequent collisions result in duplicate storage — acceptable
            // for a simple implementation without chaining.
        }

        let start = self.buffer.len() as u32;
        let len = s.len() as u32;
        self.buffer.extend_from_slice(s.as_bytes());

        let id = StrId(self.offsets.len() as u32);
        self.offsets.push((start, len));
        self.index.insert(hash, id);
        id
    }

    /// Retrieve a previously interned string by its [`StrId`].
    ///
    /// # Panics
    /// Panics if `id` was not returned by this pool.
    pub fn get(&self, id: StrId) -> &str {
        let (start, len) = self.offsets[id.0 as usize];
        let bytes = &self.buffer[start as usize..(start + len) as usize];
        // SAFETY: we only ever store valid UTF-8 via `intern`.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }

    /// Look up a string without inserting it.
    ///
    /// Returns `Some(id)` if the string was previously interned, `None`
    /// otherwise. This is useful in read-only contexts where mutating the
    /// pool is not possible.
    pub fn intern_readonly(&self, s: &str) -> Option<StrId> {
        let hash = fx_hash(s.as_bytes());
        if let Some(&id) = self.index.get(&hash) {
            if self.get(id) == s {
                return Some(id);
            }
        }
        None
    }

    /// Number of unique strings interned (may be slightly higher than the
    /// true unique count if FxHash collisions occurred).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Returns `true` if no strings have been interned.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_and_get() {
        let mut pool = StringPool::new();
        let id = pool.intern("hello");
        assert_eq!(pool.get(id), "hello");
    }

    #[test]
    fn dedup_same_string() {
        let mut pool = StringPool::new();
        let a = pool.intern("world");
        let b = pool.intern("world");
        assert_eq!(a, b);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn different_strings_get_different_ids() {
        let mut pool = StringPool::new();
        let a = pool.intern("foo");
        let b = pool.intern("bar");
        assert_ne!(a, b);
        assert_eq!(pool.get(a), "foo");
        assert_eq!(pool.get(b), "bar");
    }

    #[test]
    fn large_number_of_strings() {
        let mut pool = StringPool::with_capacity(16_384);
        let ids: Vec<StrId> = (0..2_000)
            .map(|i| pool.intern(&format!("symbol_{i}")))
            .collect();

        // All retrievable
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(pool.get(id), format!("symbol_{i}"));
        }

        // Re-interning gives back the same ids
        for (i, &original) in ids.iter().enumerate() {
            assert_eq!(pool.intern(&format!("symbol_{i}")), original);
        }
    }

    #[test]
    fn empty_string() {
        let mut pool = StringPool::new();
        let id = pool.intern("");
        assert_eq!(pool.get(id), "");
        // Interning empty string again deduplicates
        assert_eq!(pool.intern(""), id);
    }

    #[test]
    fn unicode_strings() {
        let mut pool = StringPool::new();
        let cases = [
            "\u{00e9}moji",                             // accented Latin
            "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}", // Japanese hiragana
            "\u{1f600}\u{1f680}\u{2764}\u{fe0f}",       // emoji
            "\u{00fc}ber_na\u{00ef}ve",                 // mixed diacritics
        ];

        let ids: Vec<StrId> = cases.iter().map(|s| pool.intern(s)).collect();

        for (s, &id) in cases.iter().zip(&ids) {
            assert_eq!(pool.get(id), *s);
        }

        // Dedup check
        for (s, &id) in cases.iter().zip(&ids) {
            assert_eq!(pool.intern(s), id);
        }
    }

    #[test]
    fn is_empty_and_len() {
        let mut pool = StringPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);

        pool.intern("x");
        assert!(!pool.is_empty());
        assert_eq!(pool.len(), 1);
    }
}
