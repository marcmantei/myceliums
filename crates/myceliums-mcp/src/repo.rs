//! Data-home paths and repository/symbol resolution shared by the tools.

use crate::errors::{repo_id_error, service_error, ToolError};
use crate::MyceliumsMcp;
use myceliums_core::GraphTable;
use myceliums_storage::{
    CodeSymbol, Community, FileNode, Process, Relationship, RepoInfo, RepoRegistry, Store,
};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) fn data_dir() -> PathBuf {
    // `MYCELIUMS_DATA_DIR` overrides the data home so the server can be pointed
    // at an isolated directory. This is a **test/dev-only seam** (the
    // integration harness sets it to a throwaway tempdir) and also serves
    // multi-tenant setups; it is intentionally not a security boundary — the
    // path is taken verbatim and not validated, so production deployments must
    // not expose it to untrusted input.
    if let Some(dir) = std::env::var_os("MYCELIUMS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}

pub(crate) fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

/// Loads the repository registry; a load failure is a server fault.
pub(crate) fn registry() -> Result<RepoRegistry, ToolError> {
    RepoRegistry::load(&registry_path())
        .map_err(|e| ToolError::internal("cannot read the repository registry", e))
}

/// The registry entry of a caller-supplied repo id (malformed → invalid
/// params, unknown → recoverable).
pub(crate) fn repo_info(repo_id: &str) -> Result<RepoInfo, ToolError> {
    registry()?.require(repo_id).cloned().map_err(repo_id_error)
}

/// Database directory for a caller-supplied repo id.
///
/// This is the MCP side of the repo-id choke point
/// ([`RepoRegistry::registered_db_path`]): malformed or unregistered ids are
/// rejected with an error that lists the known ids, before any path is built
/// or any directory is created.
pub(crate) fn registered_db_path(repo_id: &str) -> Result<PathBuf, ToolError> {
    registry()?
        .registered_db_path(&data_dir(), repo_id)
        .map_err(repo_id_error)
}

/// How many candidates an "ambiguous symbol" error lists.
const AMBIGUOUS_SHOWN: usize = 8;

/// `file_path:qualified_name`: the handle that tells same-named symbols apart
/// (top-level definitions have `qualified_name == name`, so two files that
/// each define `parse` share both).
pub(crate) fn located_name(symbol: &CodeSymbol) -> String {
    format!("{}:{}", symbol.file_path, symbol.qualified_name)
}

/// Whether `name` is `symbol`'s [`located_name`].
fn is_located_name(symbol: &CodeSymbol, name: &str) -> bool {
    name.strip_prefix(symbol.file_path.as_str())
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| rest == symbol.qualified_name)
}

/// Finds exactly one symbol by `file_path:qualified_name`, qualified name,
/// or short name, in that order.
///
/// Not found and ambiguous are recoverable: the error names the next step
/// (search for it, or pass one of the listed `file_path:qualified_name`
/// handles).
pub(crate) fn resolve_symbol<'a>(
    symbols: &'a [CodeSymbol],
    name: &str,
) -> Result<&'a CodeSymbol, ToolError> {
    if let Some(symbol) = symbols.iter().find(|s| is_located_name(s, name)) {
        return Ok(symbol);
    }
    let by_qualified: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| s.qualified_name == name)
        .collect();
    let matches = if by_qualified.is_empty() {
        symbols.iter().filter(|s| s.name == name).collect()
    } else {
        by_qualified
    };
    let Some(first) = matches.first() else {
        return Err(ToolError::symbol_not_found(name));
    };
    let mut handles: Vec<String> = matches.iter().map(|s| located_name(s)).collect();
    handles.sort();
    handles.dedup();
    // Several symbols with one handle (e.g. overloads in one file) cannot be
    // told apart any further: the first stands for them all.
    if handles.len() == 1 {
        return Ok(first);
    }
    let more = handles.len().saturating_sub(AMBIGUOUS_SHOWN);
    let more = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    let shown: Vec<String> = handles
        .iter()
        .take(AMBIGUOUS_SHOWN)
        .map(|h| format!("`{h}`"))
        .collect();
    Err(ToolError::recoverable(
        format!("Symbol `{name}` is ambiguous: {} matches.", handles.len()),
        format!(
            "pass one of these as the symbol name: {}{more}",
            shown.join(", ")
        ),
    ))
}

/// Labels CALLS edges with how their target was resolved (`exact`,
/// `import`, `name-match`, `ambiguous`).
///
/// Edges indexed since #96 carry the label the resolver recorded. Older
/// indexes have none; for those the label is derived here: `ambiguous` when
/// several symbols share the target's name, else `name-match` — never
/// `exact`, because the resolver's evidence is gone.
pub(crate) struct CallLabels<'a> {
    shared_names: std::collections::HashSet<&'a str>,
}

impl<'a> CallLabels<'a> {
    pub(crate) fn new(symbols: &'a [CodeSymbol]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut shared_names = std::collections::HashSet::new();
        for s in symbols {
            if !seen.insert(s.name.as_str()) {
                shared_names.insert(s.name.as_str());
            }
        }
        CallLabels { shared_names }
    }

    /// The label of `edge`, whose target symbol is named `target_name`.
    pub(crate) fn label(&self, edge: &Relationship, target_name: &str) -> &'static str {
        match myceliums_core::CallResolution::from_metadata(&edge.metadata) {
            Some(resolution) => resolution.label(),
            None if self.shared_names.contains(target_name) => "ambiguous",
            None => "name-match",
        }
    }

    /// `name`, followed by the label when the edge is a guess.
    pub(crate) fn mark(label: &str, name: &str) -> String {
        if Self::is_heuristic(label) {
            format!("{name} (resolved: {label})")
        } else {
            name.to_string()
        }
    }

    /// Whether the label marks a name-based guess rather than a certain edge.
    pub(crate) fn is_heuristic(label: &str) -> bool {
        matches!(label, "name-match" | "ambiguous")
    }
}

impl MyceliumsMcp {
    /// Turns an optional caller-supplied repo id into a registered one.
    ///
    /// An explicit id must pass [`RepoRegistry::require`]; without one, the
    /// repo containing the working directory (or else the most recently
    /// analyzed repo) is used.
    pub(crate) fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String, ToolError> {
        let registry = registry()?;
        if let Some(id) = repo_id {
            registry.require(id).map_err(repo_id_error)?;
            return Ok(id.to_string());
        }

        // Try to match current working directory to a registered repo
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(abs_cwd) = std::fs::canonicalize(&cwd) {
                let cwd_str = abs_cwd.to_string_lossy();
                for repo in registry.list().iter().rev() {
                    if let Ok(repo_path) = std::fs::canonicalize(&repo.path) {
                        let repo_str = repo_path.to_string_lossy();
                        if cwd_str.starts_with(repo_str.as_ref()) {
                            return Ok(repo.id.clone());
                        }
                    }
                }
            }
        }

        // Fallback: the most recently analyzed repo. RFC 3339 timestamps in
        // the same offset sort chronologically as strings.
        registry
            .list()
            .into_iter()
            .max_by(|a, b| a.analyzed_at.cmp(&b.analyzed_at))
            .map(|r| r.id.clone())
            .ok_or_else(|| {
                ToolError::recoverable(
                    "No repository is indexed yet.",
                    "run analyze with the project path",
                )
            })
    }
}

/// Graph reads for the tools, served from the [`myceliums_core::GraphService`]
/// cache: each table is read from disk once per repository and reused until
/// `analyze`/`delete` (or an on-disk change) invalidates it. Every read
/// validates `repo_id` first — malformed ids are `invalid_params`, unknown
/// ones a recoverable error.
impl MyceliumsMcp {
    /// The shared store handle, for reads the graph cache does not cover
    /// (vector search, embeddings, index metadata).
    pub(crate) async fn store(&self, repo_id: &str) -> Result<Arc<Store>, ToolError> {
        self.service
            .open_store(repo_id)
            .await
            .map_err(service_error)
    }

    pub(crate) async fn symbols(&self, repo_id: &str) -> Result<GraphTable<CodeSymbol>, ToolError> {
        self.service.symbols(repo_id).await.map_err(service_error)
    }

    pub(crate) async fn relationships(
        &self,
        repo_id: &str,
    ) -> Result<GraphTable<Relationship>, ToolError> {
        self.service
            .relationships(repo_id)
            .await
            .map_err(service_error)
    }

    pub(crate) async fn files(&self, repo_id: &str) -> Result<GraphTable<FileNode>, ToolError> {
        self.service.files(repo_id).await.map_err(service_error)
    }

    pub(crate) async fn communities(
        &self,
        repo_id: &str,
    ) -> Result<GraphTable<Community>, ToolError> {
        self.service
            .communities(repo_id)
            .await
            .map_err(service_error)
    }

    pub(crate) async fn processes(&self, repo_id: &str) -> Result<GraphTable<Process>, ToolError> {
        self.service.processes(repo_id).await.map_err(service_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn sym(name: &str, qualified: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: format!("{file}:{qualified}"),
            name: name.into(),
            qualified_name: qualified.into(),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: 1,
            end_line: 2,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    #[test]
    fn ambiguous_name_is_recoverable_and_lists_located_names() {
        let symbols = vec![
            sym("parse", "a.parse", "a.ts"),
            sym("parse", "b.parse", "b.ts"),
        ];
        let err = resolve_symbol(&symbols, "parse").unwrap_err();
        let msg = err.message();
        assert!(matches!(err, ToolError::Recoverable { .. }));
        assert!(msg.contains("ambiguous: 2 matches"), "{msg}");
        assert!(
            msg.contains(
                "Next: pass one of these as the symbol name: `a.ts:a.parse`, `b.ts:b.parse`"
            ),
            "{msg}"
        );
        // A qualified name resolves it, and so does the listed handle.
        assert_eq!(
            resolve_symbol(&symbols, "b.parse").unwrap().file_path,
            "b.ts"
        );
        assert_eq!(
            resolve_symbol(&symbols, "b.ts:b.parse").unwrap().file_path,
            "b.ts"
        );
    }

    #[test]
    fn shared_qualified_names_are_ambiguous_too() {
        // Top-level definitions: qualified name == name in both files.
        let symbols = vec![sym("parse", "parse", "a.ts"), sym("parse", "parse", "b.ts")];
        let err = resolve_symbol(&symbols, "parse").unwrap_err();
        assert!(
            err.message().contains("`a.ts:parse`, `b.ts:parse`"),
            "{}",
            err.message()
        );
        assert_eq!(
            resolve_symbol(&symbols, "b.ts:parse").unwrap().file_path,
            "b.ts"
        );
        // Rust-style qualified names keep their `::`.
        let rust = vec![
            sym("f", "m::f", "src/lib.rs"),
            sym("f", "n::f", "src/lib.rs"),
        ];
        assert_eq!(
            resolve_symbol(&rust, "src/lib.rs:n::f")
                .unwrap()
                .qualified_name,
            "n::f"
        );
    }

    #[test]
    fn one_handle_for_several_symbols_resolves_to_the_first() {
        let mut overload = sym("f", "C.f", "a.ts");
        overload.uid = "second".into();
        let symbols = vec![sym("f", "C.f", "a.ts"), overload];
        assert_eq!(resolve_symbol(&symbols, "f").unwrap().uid, "a.ts:C.f");
    }

    #[test]
    fn missing_symbol_is_recoverable() {
        let err = resolve_symbol(&[], "nope").unwrap_err();
        assert!(err.message().starts_with("Symbol `nope` not found"));
        assert!(err.message().contains("Next: "));
    }
}
