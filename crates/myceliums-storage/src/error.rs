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
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// [`crate::Store::open_existing`] found no index at the path.
    #[error("no index at {}", .0.display())]
    NoIndex(PathBuf),

    /// The database path is not valid UTF-8, which LanceDB's URI-based
    /// `connect` cannot express.
    #[error("the database path {} is not valid UTF-8", .0.display())]
    NonUtf8Path(PathBuf),

    /// A repository id is malformed or not registered.
    #[error(transparent)]
    RepoId(#[from] RepoIdError),

    /// A LanceDB operation failed.
    #[error("{}", with_context(context, source))]
    Database {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The LanceDB error.
        #[source]
        source: lancedb::Error,
    },

    /// Building or reading an Arrow record batch failed.
    #[error("{}", with_context(context, source))]
    Arrow {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The Arrow error.
        #[source]
        source: arrow_schema::ArrowError,
    },

    /// A file of the registry or the store could not be read or written.
    #[error("{}", with_context(context, source))]
    Io {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },

    /// JSON (the registry file, metadata columns) could not be encoded or
    /// decoded.
    #[error("{}", with_context(context, source))]
    Json {
        /// What the store was doing; empty when there is nothing to add.
        context: String,
        /// The serde_json error.
        #[source]
        source: serde_json::Error,
    },

    /// Stored data does not have the expected shape (a missing column, an
    /// unknown enum value).
    #[error("{0}")]
    InvalidData(String),
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
fn with_context(context: &str, source: &dyn fmt::Display) -> String {
    if context.is_empty() {
        source.to_string()
    } else {
        format!("{context}: {source}")
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
    fn in_context(self, context: String) -> StorageError {
        match self {
            StorageError::Database { source, .. } => StorageError::Database { context, source },
            StorageError::Arrow { source, .. } => StorageError::Arrow { context, source },
            StorageError::Io { source, .. } => StorageError::Io { context, source },
            StorageError::Json { source, .. } => StorageError::Json { context, source },
            StorageError::InvalidData(msg) => {
                StorageError::InvalidData(format!("{context}: {msg}"))
            }
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
        assert!(std::error::Error::source(&err).is_some());

        let bare = StorageError::from(std::io::Error::other("disk full"));
        assert_eq!(bare.to_string(), "disk full");
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
