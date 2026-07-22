//! # myceliums-core
//!
//! Code intelligence core for the Myceliums knowledge engine.
//!
//! This crate provides tree-sitter-based parsing, call-graph construction,
//! community detection, impact analysis, and hybrid search over codebases.
//!
//! ## Getting Started
//!
//! ```rust,no_run
//! use myceliums_core::{Analyzer, ProjectConfig};
//! use myceliums_storage::Store;
//! use std::path::{Path, PathBuf};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Open a store (LanceDB-backed)
//! let store = Store::open(Path::new("/tmp/myceliums-data"), "my-repo-id").await?;
//!
//! // 2. Build an analyzer for your repository
//! let repo = PathBuf::from("/path/to/repo");
//! let analyzer = Analyzer::new(store, repo);
//!
//! // 3. Run full analysis (parsing + call-graph + embeddings)
//! let result = analyzer.analyze().await?;
//! println!("Indexed {} symbols across {} files", result.symbol_count, result.file_count);
//!
//! // 4. Search the indexed symbols
//! let symbols = analyzer.store().get_symbols().await?;
//! let hits = myceliums_core::search_symbols(&symbols, "authenticate user");
//! for hit in &hits {
//!     println!("{} (score {:.2})", hit.symbol.name, hit.score);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `embeddings` | **yes** | Enables fastembed-based semantic search and reranking |
//! | `pdf` | no | Enables PDF-to-markdown conversion via `opendataloader-pdf` CLI |

// ── Public modules ───────────────────────────────────────────────────

pub mod adr;
pub mod analyzer;
pub mod arch_diagram;
pub mod arch_lint;
pub mod batch_writer;
pub mod cache;
pub mod centrality;
pub mod codeowners;
pub mod community;
pub mod config;
pub mod contracts;
pub mod cross_repo;
pub mod cycles;
pub mod dependencies;
pub mod drift;
pub mod dsl;
pub mod email;
#[cfg(feature = "embeddings")]
pub mod embeddings;
pub mod error;
pub mod file_guard;
pub mod git_metadata;
pub mod global_config;
pub mod god_nodes;
pub mod graphml_export;
pub mod hotspot_score;
pub mod hybrid_search;
pub mod imap;
pub mod impact;
pub mod llm;
pub mod lock;
pub mod mbox;
pub mod mentions;
pub mod mermaid_export;
pub mod neo4j_export;
pub mod notebook;
pub mod ontology;
pub mod parser;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod process;
pub mod progress;
pub mod rename;
pub mod search;
pub mod service_map;
pub mod snapshot;
pub mod surprising_connections;
pub mod timing;
pub mod watch;
pub mod wiki_export;

// ── Internal modules (not part of the public API) ────────────────────

pub(crate) mod content;
pub(crate) mod module_graph;
pub(crate) mod resolver;
#[allow(dead_code)]
pub(crate) mod ssa;
pub(crate) mod string_pool;

// ── Top-level re-exports ─────────────────────────────────────────────

pub use adr::{
    adr_path, link_decision_to_symbol, load_decisions, save_decision, AdrStatus, ArchDecisionRecord,
};
pub use analyzer::{compute_member_of_relationships, AnalysisResult, Analyzer};
pub use arch_diagram::{
    generate_architecture_diagram, ArchDiagram, ServiceConnection, ServiceNode,
};
pub use arch_lint::{lint_architecture, LintFinding, LintReport, LintSeverity};
pub use cache::{check_cache, get_head_commit, CacheCheckConfig, CacheDecision, QueryCache};
pub use centrality::{compute_centrality, NodeCentrality};
pub use codeowners::{
    compute_ownership, parse_codeowners, FileOwnership, OwnershipEntry, OwnershipReport,
};
pub use community::{
    compute_community_metrics, compute_uid_to_community_label, CommunityCoupling,
    CommunityDetector, CommunityMetrics,
};
pub use config::ProjectConfig;
pub use contracts::{detect_contracts, ContractsReport};
pub use cycles::{detect_cycles, DependencyCycle};
pub use dependencies::{
    compute_file_dependencies, compute_module_coupling, FileDependency, ModuleCoupling,
};
pub use drift::{detect_drift, DriftReport};
pub use email::{parse_eml, parse_eml_file, EmailAttachment, ParsedEmail};
#[cfg(feature = "embeddings")]
pub use embeddings::{
    check_model_cache, embedder_for_index, embedding_cache_info, embedding_model_spec,
    get_embedder_for, get_reranker, index_embedding_meta, local_model_code, reranker_spec,
    EmbedInput, Embedder, EmbeddingModelSpec, IndexEmbeddingMeta, ModelCacheInfo, Reranker,
    RerankerSpec, DEFAULT_LOCAL_EMBEDDING_MODEL, DEFAULT_RERANKER_MODEL, EMBEDDING_MODELS,
    LEGACY_LOCAL_EMBEDDING_MODEL, RERANKER_MODELS,
};
pub use error::MyceliumError;
pub use file_guard::{should_skip_file, FileSkipReason};
pub use git_metadata::{GitMetadata, GitMetadataExtractor};
pub use global_config::GlobalConfig;
pub use god_nodes::{compute_god_nodes, GodNodeItem};
pub use graphml_export::export_graphml;
pub use hotspot_score::{compute_hotspot_scores, HotspotItem};
pub use hybrid_search::{
    attach_graph_edges, reciprocal_rank_fusion, GraphEdge, HybridExplain, HybridSearchResult,
};
#[cfg(feature = "embeddings")]
pub use hybrid_search::{hybrid_search, hybrid_search_explain, rerank_results};
pub use impact::{detect_impact, run_git_diff, ImpactReport};
pub use lock::{AnalysisLock, LockOutcome};
pub use mbox::parse_mbox;
pub use mentions::{extract_mentions, extract_mentions_llm};
pub use mermaid_export::{export_mermaid, export_mermaid_with_communities, MermaidDiagramType};
pub use neo4j_export::export_neo4j_cypher;
pub use ontology::{EdgeType, EntityType, Ontology, Property};
pub use process::{ProcessFilter, ProcessTracer};
pub use progress::{AnalysisPhase, ProgressReporter, SilentReporter};
pub use rename::RenamePlan;
pub use search::{search_symbols, search_symbols_explain, SearchExplain, TermScore};
pub use service_map::{load_service_mappings, save_service_mapping, ServiceMapping};
pub use snapshot::{
    build_snapshot, diff_snapshots, list_snapshots, load_snapshot, load_snapshot_by_id,
    save_snapshot, save_versioned_snapshot, snapshot_dir, snapshot_path, DiffEntry, GraphDiff,
    GraphSnapshot, SnapshotSummary,
};
pub use surprising_connections::{compute_surprising_connections, SurprisingConnectionItem};
pub use timing::{Timer, TimingReport};
pub use wiki_export::{export_wiki, WikiExportConfig, WikiExportResult};
