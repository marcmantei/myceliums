//! `repository://` resource reading.

use crate::repo::{registered_db_path, registry_path};
use crate::MyceliumsMcp;
use myceliums_storage::RepoRegistry;
use std::collections::HashMap;

// Resource reading implementation
impl MyceliumsMcp {
    pub(crate) async fn read_resource_inner(&self, uri: &str) -> Result<String, rmcp::ErrorData> {
        let mcp_err = |msg: String| rmcp::ErrorData::internal_error(msg, None);

        if uri == "repository://list" {
            let registry =
                RepoRegistry::load(&registry_path()).map_err(|e| mcp_err(format!("{}", e)))?;
            let repos: Vec<serde_json::Value> = registry
                .list()
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "path": r.path,
                        "analyzed_at": r.analyzed_at,
                        "symbol_count": r.symbol_count,
                        "file_count": r.file_count,
                    })
                })
                .collect();
            return serde_json::to_string_pretty(&repos).map_err(|e| mcp_err(format!("{}", e)));
        }

        let path = uri
            .strip_prefix("repository://")
            .ok_or_else(|| mcp_err(format!("Unknown resource URI scheme: {}", uri)))?;
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.is_empty() {
            return Err(mcp_err(format!("Invalid resource URI: {}", uri)));
        }

        let repo_id = parts[0];
        // Reject malformed and unknown ids up front, whatever the sub-path.
        registered_db_path(repo_id)?;
        let sub_path = if parts.len() > 1 { parts[1] } else { "" };

        match sub_path {
            "schema" => {
                let symbols = self.symbols(repo_id).await?;
                let files = self.files(repo_id).await?;
                let relationships = self.relationships(repo_id).await?;
                let communities = self.communities(repo_id).await?;
                let processes = self.processes(repo_id).await?;
                let mut kind_counts: HashMap<String, usize> = HashMap::new();
                for s in &symbols {
                    *kind_counts.entry(s.kind.to_string()).or_default() += 1;
                }
                let mut rel_counts: HashMap<String, usize> = HashMap::new();
                for r in &relationships {
                    *rel_counts.entry(r.kind.to_string()).or_default() += 1;
                }
                let result = serde_json::json!({
                    "repo_id": repo_id, "symbol_count": symbols.len(), "file_count": files.len(),
                    "relationship_count": relationships.len(), "community_count": communities.len(),
                    "process_count": processes.len(), "symbols_by_kind": kind_counts, "relationships_by_kind": rel_counts,
                });
                serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
            }
            "map" => {
                let communities = self.communities(repo_id).await?;
                let result: Vec<serde_json::Value> = communities.iter().map(|c| {
                    serde_json::json!({"uid": c.uid, "label": c.label, "member_count": c.member_count, "top_symbols": c.top_symbols, "summary": c.summary})
                }).collect();
                serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
            }
            rest => {
                let sub_parts: Vec<&str> = rest.splitn(2, '/').collect();
                match sub_parts.as_slice() {
                    ["community", community_id] => {
                        let communities = self.communities(repo_id).await?;
                        let community = communities
                            .iter()
                            .find(|c| c.uid == *community_id)
                            .ok_or_else(|| {
                                mcp_err(format!("Community not found: {}", community_id))
                            })?;
                        let result = serde_json::json!({"uid": community.uid, "label": community.label, "member_count": community.member_count, "top_symbols": community.top_symbols, "summary": community.summary});
                        serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
                    }
                    ["process", process_id] => {
                        let processes = self.processes(repo_id).await?;
                        let process = processes
                            .iter()
                            .find(|p| p.uid == *process_id)
                            .ok_or_else(|| mcp_err(format!("Process not found: {}", process_id)))?;
                        let result = serde_json::json!({"uid": process.uid, "name": process.name, "entry_point": process.entry_point, "step_count": process.step_count, "description": process.description});
                        serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
                    }
                    _ => Err(mcp_err(format!("Unknown resource path: {}", uri))),
                }
            }
        }
    }
}
