//! # myceliums-storage
//!
//! The persistence layer for the Myceliums knowledge engine.
//!
//! This crate stores and retrieves the code graph — symbols, files,
//! relationships, communities, processes, and their vector embeddings — using
//! [LanceDB](https://lancedb.github.io/lancedb/) as an embedded, columnar
//! vector store. One database is opened per repository via [`Store`]; shared
//! team metadata lives in a separate database via [`TeamStore`].
//!
//! ## Layout
//!
//! - [`models`] — the domain types persisted by the store ([`CodeSymbol`],
//!   [`Relationship`], [`Community`], and friends).
//! - [`schema`] — Arrow schemas describing each on-disk table.
//! - [`store`] — [`Store`], the per-repository read/write API.
//! - [`registry`] — [`RepoRegistry`], the index of analyzed repositories.
//! - [`team_store`] — [`TeamStore`], the shared team-metadata API.
//!
//! ## Example
//!
//! ```no_run
//! use myceliums_storage::Store;
//! use std::path::Path;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let store = Store::open(Path::new("/tmp/myceliums-data"), "my-repo-id").await?;
//! let symbols = store.get_symbols().await?;
//! println!("{} symbols indexed", symbols.len());
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

/// Domain types persisted by the store (symbols, relationships, teams, ...).
pub mod models;
/// The registry of analyzed repositories.
pub mod registry;
/// Arrow schemas for the on-disk tables.
pub mod schema;
/// The per-repository LanceDB-backed store.
pub mod store;
/// The shared team-metadata store.
pub mod team_store;

pub use models::*;
pub use registry::RepoRegistry;
pub use store::Store;
pub use team_store::TeamStore;
