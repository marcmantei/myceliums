//! Domain-specific error types for myceliums-core.
//!
//! [`MyceliumError`] replaces bare `anyhow::Error` in public API signatures,
//! giving downstream consumers structured, matchable error variants.

use std::path::PathBuf;

/// The main error type for myceliums-core operations.
///
/// Each variant captures a specific failure mode so callers can handle errors
/// programmatically rather than parsing error messages.
#[derive(Debug, thiserror::Error)]
pub enum MyceliumError {
    /// A file could not be read or does not exist.
    #[error("I/O error for {path}: {source}")]
    Io {
        /// The path that triggered the error.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Tree-sitter parsing failed for a source file.
    #[error("parse error: {0}")]
    Parse(String),

    /// A configuration file was malformed or missing required fields.
    #[error("config error: {0}")]
    Config(String),

    /// The requested symbol was not found in the knowledge graph.
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),

    /// A storage operation (LanceDB / store) failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An embedding model operation failed (loading, inference).
    #[error("embedding error: {0}")]
    Embedding(String),

    /// A git operation failed.
    #[error("git error: {0}")]
    Git(String),

    /// Catch-all for errors that don't fit other categories.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, MyceliumError>;

// ── Conversions ──────────────────────────────────────────────────────

impl From<std::io::Error> for MyceliumError {
    fn from(e: std::io::Error) -> Self {
        MyceliumError::Io {
            path: PathBuf::new(),
            source: e,
        }
    }
}
