//! The error type of the storage layer.
//!
//! Every fallible public function of this crate returns [`StorageError`], so
//! callers can tell a missing index or a bad repository id from a database
//! failure without parsing messages.

use crate::registry::RepoIdError;
use std::fmt;
use std::path::PathBuf;

/// Errors returned by [`crate::Store`], [`crate::TeamStore`] and
/// [`crate::RepoRegistry`].
///
/// The message of a variant that wraps a library error is
/// `"{context}: {error}"`, so printing it alone says everything. Its
/// [`std::error::Error::source`] is therefore the wrapped error's own
/// source, not the wrapped error: a report that prints the whole chain
/// (`anyhow`'s `{:#}` or `Caused by:`) shows each message once (#118).
#[derive(Debug)]
pub enum StorageError {
    /// [`crate::Store::open_existing`] found no index at the path.
    NoIndex(PathBuf),

    /// The database path is not valid UTF-8, which LanceDB's URI-based
    /// `connect` cannot express.
    NonUtf8Path(PathBuf),

    /// A repository id is malformed or not registered.
    RepoId(RepoIdError),

    /// A LanceDB operation failed.
    Database {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The LanceDB error.
        source: lancedb::Error,
    },

    /// Building or reading an Arrow record batch failed.
    Arrow {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The Arrow error.
        source: arrow_schema::ArrowError,
    },

    /// A file of the registry or the store could not be read or written.
    Io {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The I/O error.
        source: std::io::Error,
    },

    /// JSON (the registry file, metadata columns) could not be encoded or
    /// decoded.
    Json {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The serde_json error.
        source: serde_json::Error,
    },

    /// Stored data does not have the expected shape (a missing column, an
    /// unknown enum value).
    InvalidData(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::NoIndex(path) => write!(f, "no index at {}", path.display()),
            StorageError::NonUtf8Path(path) => {
                write!(f, "the database path {} is not valid UTF-8", path.display())
            }
            StorageError::RepoId(e) => fmt::Display::fmt(e, f),
            StorageError::Database { context, source } => write_with_context(f, context, source),
            StorageError::Arrow { context, source } => write_with_context(f, context, source),
            StorageError::Io { context, source } => write_with_context(f, context, source),
            StorageError::Json { context, source } => write_with_context(f, context, source),
            StorageError::InvalidData(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // The wrapped error's message is part of ours: continue the chain
        // below it, so no message is printed twice.
        match self {
            StorageError::RepoId(e) => e.source(),
            StorageError::Database { source, .. } => source.source(),
            StorageError::Arrow { source, .. } => source.source(),
            StorageError::Io { source, .. } => source.source(),
            StorageError::Json { source, .. } => source.source(),
            StorageError::NoIndex(_) | StorageError::NonUtf8Path(_) => None,
            StorageError::InvalidData(_) => None,
        }
    }
}

impl From<RepoIdError> for StorageError {
    fn from(e: RepoIdError) -> Self {
        StorageError::RepoId(e)
    }
}

/// Convenience alias for results of this crate.
pub type Result<T, E = StorageError> = std::result::Result<T, E>;

/// `path` as the `&str` LanceDB's `connect` takes; a typed error instead of
/// a panic when it is not UTF-8.
pub(crate) fn db_uri(path: &std::path::Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| StorageError::NonUtf8Path(path.to_path_buf()))
}

/// `"{context}: {source}"`, or just the source without a context.
fn write_with_context(
    f: &mut fmt::Formatter<'_>,
    context: &str,
    source: &dyn fmt::Display,
) -> fmt::Result {
    if context.is_empty() {
        write!(f, "{source}")
    } else {
        write!(f, "{context}: {source}")
    }
}

/// `outer` in front of the context an error already has: `.context()`
/// called twice keeps both (`"outer: inner: source"`).
fn nest(outer: String, inner: String) -> String {
    if inner.is_empty() {
        outer
    } else if outer.is_empty() {
        inner
    } else {
        format!("{outer}: {inner}")
    }
}

macro_rules! from_source {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl From<$ty> for StorageError {
            fn from(source: $ty) -> Self {
                StorageError::$variant { context: String::new(), source }
            }
        }
        impl Contextual for $ty {
            fn in_context(self, context: String) -> StorageError {
                StorageError::$variant { context, source: self }
            }
        }
    )*};
}

from_source!(
    lancedb::Error => Database,
    arrow_schema::ArrowError => Arrow,
    std::io::Error => Io,
    serde_json::Error => Json,
);

/// An error that can be wrapped into a [`StorageError`] with a context
/// message.
pub(crate) trait Contextual {
    fn in_context(self, context: String) -> StorageError;
}

impl Contextual for StorageError {
    /// Adds `context` in front of the one the error already carries, rather
    /// than replacing it (#118).
    fn in_context(self, context: String) -> StorageError {
        match self {
            StorageError::Database {
                context: inner,
                source,
            } => StorageError::Database {
                context: nest(context, inner),
                source,
            },
            StorageError::Arrow {
                context: inner,
                source,
            } => StorageError::Arrow {
                context: nest(context, inner),
                source,
            },
            StorageError::Io {
                context: inner,
                source,
            } => StorageError::Io {
                context: nest(context, inner),
                source,
            },
            StorageError::Json {
                context: inner,
                source,
            } => StorageError::Json {
                context: nest(context, inner),
                source,
            },
            StorageError::InvalidData(msg) => StorageError::InvalidData(nest(context, msg)),
            other => other,
        }
    }
}

/// `.context(..)` for results of this crate, in the style of `anyhow`.
pub(crate) trait Context<T> {
    /// Attaches `context` to the error.
    fn context(self, context: impl Into<String>) -> Result<T>;
}

impl<T, E: Contextual> Context<T> for std::result::Result<T, E> {
    fn context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.in_context(context.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_prefixes_the_message_and_keeps_the_source() {
        let err: Result<()> = Err(std::io::Error::other("disk full")).context("Failed to write");
        let err = err.unwrap_err();
        assert_eq!(err.to_string(), "Failed to write: disk full");
        assert!(matches!(err, StorageError::Io { .. }));

        let bare = StorageError::from(std::io::Error::other("disk full"));
        assert_eq!(bare.to_string(), "disk full");
    }

    /// #118: the wrapped error's message is in ours, so it is not also the
    /// source — a full chain prints "disk full" once.
    #[test]
    fn the_chain_prints_each_message_once() {
        let err: Result<()> = Err(std::io::Error::other("disk full")).context("Failed to write");
        let err = err.unwrap_err();
        let mut chain = vec![err.to_string()];
        let mut cause = std::error::Error::source(&err);
        while let Some(e) = cause {
            chain.push(e.to_string());
            cause = e.source();
        }
        assert_eq!(chain, ["Failed to write: disk full"]);

        // Deeper causes of the wrapped error are still in the chain.
        #[derive(Debug)]
        struct Cause;
        impl std::fmt::Display for Cause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("sector unreadable")
            }
        }
        impl std::error::Error for Cause {}
        #[derive(Debug)]
        struct ReadFailed(Cause);
        impl std::fmt::Display for ReadFailed {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("read failed")
            }
        }
        impl std::error::Error for ReadFailed {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let io = std::io::Error::other(ReadFailed(Cause));
        let err = StorageError::from(io).in_context("Failed to read".to_string());
        assert_eq!(err.to_string(), "Failed to read: read failed");
        let source = std::error::Error::source(&err).expect("the cause below the io error");
        assert_eq!(source.to_string(), "sector unreadable");
    }

    /// #118: a second `.context()` used to replace the first.
    #[test]
    fn nested_context_keeps_both_messages() {
        let inner: Result<()> = Err(std::io::Error::other("disk full")).context("write symbols");
        let outer = inner.context("store analysis");
        assert_eq!(
            outer.unwrap_err().to_string(),
            "store analysis: write symbols: disk full"
        );
        let invalid: Result<()> = Err(StorageError::InvalidData("no column uid".into()));
        assert_eq!(
            invalid.context("read symbols").unwrap_err().to_string(),
            "read symbols: no column uid"
        );
    }

    /// A non-UTF-8 database path is an error, not a panic (#101).
    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_db_path_is_an_error_not_a_panic() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(std::ffi::OsStr::from_bytes(b"repo-\xff"));
        let err = crate::Store::open(&path, "r")
            .await
            .err()
            .expect("must fail");
        assert!(matches!(err, StorageError::NonUtf8Path(_)), "{err:?}");
        let err = crate::TeamStore::open(&path)
            .await
            .err()
            .expect("must fail");
        assert!(matches!(err, StorageError::NonUtf8Path(_)), "{err:?}");
        assert!(!path.exists(), "nothing is created");
    }

    #[test]
    fn repo_id_errors_stay_matchable() {
        let err = StorageError::from(RepoIdError::Invalid {
            id: "..".into(),
            reason: "test",
        });
        assert!(matches!(
            err,
            StorageError::RepoId(RepoIdError::Invalid { .. })
        ));
    }
}
