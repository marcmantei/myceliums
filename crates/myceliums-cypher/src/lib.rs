//! # myceliums-cypher
//!
//! A small, **read-only** Cypher query engine over the Myceliums code graph.
//!
//! This crate parses a deliberately narrow subset of the Cypher query language
//! and evaluates it against symbols, files, and relationships loaded from a
//! [`myceliums_storage::Store`]. It is intended for *querying* code graphs, not
//! for mutating them: write clauses (`CREATE`, `DELETE`, `SET`, `MERGE`, `DROP`,
//! `ALTER`) are rejected at parse time by design.
//!
//! ## Pipeline
//!
//! A query flows through three stages, one per module:
//!
//! 1. [`lexer`] — tokenizes the query string into [`lexer::Token`]s.
//! 2. [`parser`] — builds a [`parser::Query`] AST, rejecting write operations.
//! 3. [`executor`] — evaluates the AST against loaded graph data, producing rows.
//!
//! ## Example
//!
//! ```no_run
//! use myceliums_cypher::CypherExecutor;
//! use myceliums_storage::Store;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let store = Store::open(std::path::Path::new("/tmp/data"), "repo-id").await?;
//! let executor = CypherExecutor::from_store(&store).await?;
//! let rows = executor.execute("MATCH (f:Function) RETURN f.name LIMIT 10")?;
//! for row in rows {
//!     println!("{row:?}");
//! }
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

/// Query evaluation: runs a parsed AST against loaded graph data.
pub mod executor;
/// Tokenizer turning a query string into [`lexer::Token`]s.
pub mod lexer;
/// Parser turning tokens into a read-only [`parser::Query`] AST.
pub mod parser;

pub use executor::CypherExecutor;
