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

    /// A caller-supplied repository id is malformed or not registered.
    #[error(transparent)]
    Repo(#[from] myceliums_storage::RepoIdError),

    /// The repository's index was replaced while a [`crate::RepoGraph`]
    /// still had tables to load: serving them would mix two index
    /// generations in one answer. Retrying the call reads the new index.
    #[error("the index of repository {0} changed during this call (a re-analysis finished)")]
    IndexChanged(String),

    /// A storage condition described by a message (no repository
    /// registered, repository not found).
    #[error("storage error: {0}")]
    Storage(String),

    /// A storage-layer operation (registry, LanceDB store) failed; the
    /// [`myceliums_storage::StorageError`] says how.
    #[error(transparent)]
    Store(myceliums_storage::StorageError),

    /// An embedding model operation failed (loading, inference).
    #[error("embedding error: {0}")]
    Embedding(String),

    /// A git operation failed.
    #[error("git error: {0}")]
    Git(String),

    /// The parser abandoned a file on its time or nesting-depth budget.
    #[error(transparent)]
    ParseAborted(#[from] crate::parser::ParseAbort),

    /// An analysis was cancelled through its [`crate::CancelCheck`].
    #[error(transparent)]
    Cancelled(#[from] crate::AnalysisCancelled),

    /// JSON could not be encoded or decoded.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Catch-all for errors that don't fit other categories.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = MyceliumError> = std::result::Result<T, E>;

/// `anyhow::bail!` for functions returning [`Result`] (or `anyhow::Result`):
/// returns early with the formatted message as [`MyceliumError::Other`].
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(anyhow::anyhow!($($arg)*).into())
    };
}
pub(crate) use bail;

// ── Conversions ──────────────────────────────────────────────────────

/// A repository-id error from the storage layer stays a
/// [`MyceliumError::Repo`]; every other storage error becomes
/// [`MyceliumError::Store`].
impl From<myceliums_storage::StorageError> for MyceliumError {
    fn from(e: myceliums_storage::StorageError) -> Self {
        match e {
            myceliums_storage::StorageError::RepoId(e) => MyceliumError::Repo(e),
            other => MyceliumError::Store(other),
        }
    }
}

/// Command output (git) that is not UTF-8.
impl From<std::string::FromUtf8Error> for MyceliumError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        MyceliumError::Other(anyhow::Error::new(e))
    }
}

/// A background task panicked or was aborted.
impl From<tokio::task::JoinError> for MyceliumError {
    fn from(e: tokio::task::JoinError) -> Self {
        MyceliumError::Other(anyhow::Error::new(e).context("background task failed"))
    }
}

impl MyceliumError {
    /// The [`crate::parser::ParseAbort`] that ended a parse, if this error
    /// is one or wraps one (under added context).
    pub fn parse_abort(&self) -> Option<&crate::parser::ParseAbort> {
        match self {
            MyceliumError::ParseAborted(abort) => Some(abort),
            MyceliumError::Other(e) => e.chain().find_map(|cause| {
                cause
                    .downcast_ref::<crate::parser::ParseAbort>()
                    .or_else(|| {
                        cause
                            .downcast_ref::<MyceliumError>()
                            .and_then(MyceliumError::parse_abort)
                    })
            }),
            _ => None,
        }
    }

    /// Whether this error is, or wraps, a cancelled analysis.
    pub fn is_cancelled(&self) -> bool {
        match self {
            MyceliumError::Cancelled(_) => true,
            MyceliumError::Other(e) => e.chain().any(|cause| {
                cause.is::<crate::AnalysisCancelled>()
                    || cause
                        .downcast_ref::<MyceliumError>()
                        .is_some_and(MyceliumError::is_cancelled)
            }),
            _ => false,
        }
    }
}

impl From<std::io::Error> for MyceliumError {
    fn from(e: std::io::Error) -> Self {
        MyceliumError::Io {
            path: PathBuf::new(),
            source: e,
        }
    }
}
