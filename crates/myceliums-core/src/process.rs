//! Process tracing — discovers execution flows through the call graph.
//!
//! [`ProcessTracer`] walks from entry-point symbols (e.g. `main`, `handler`)
//! through the call graph via DFS, producing a list of [`Process`]
//! records that describe end-to-end code flows.

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Process, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use tracing::info;
use uuid::Uuid;

/// Filter criteria for narrowing traced processes.
#[derive(Debug, Clone, Default)]
pub struct ProcessFilter {
    /// Only include processes whose entry point contains this substring.
    pub entry: Option<String>,
    /// Keyword filter that searches name, entry point, and description.
    pub filter: Option<String>,
    /// Maximum number of processes to return.
    pub limit: Option<usize>,
    /// Minimum number of steps a process must have.
    pub min_steps: Option<u32>,
}

impl ProcessFilter {
    /// Apply filter to a list of processes
    pub fn apply(&self, processes: &[Process]) -> Vec<Process> {
        processes
            .iter()
            .filter(|p| self.matches(p))
            .take(self.limit.unwrap_or(usize::MAX))
            .cloned()
            .collect()
    }

    /// Check if a process matches all filter criteria
    fn matches(&self, process: &Process) -> bool {
        // Check entry point filter
        if let Some(ref entry_filter) = self.entry {
            if !process
                .entry_point
                .to_lowercase()
                .contains(&entry_filter.to_lowercase())
            {
                return false;
            }
        }

        // Check keyword filter (searches in name, entry_point, and description)
        if let Some(ref keyword_filter) = self.filter {
            let keyword_lower = keyword_filter.to_lowercase();
            if !process.name.to_lowercase().contains(&keyword_lower)
                && !process.entry_point.to_lowercase().contains(&keyword_lower)
                && !process.description.to_lowercase().contains(&keyword_lower)
            {
                return false;
            }
        }

        // Check minimum steps filter
        if let Some(min) = self.min_steps {
            if process.step_count < min {
                return false;
            }
        }

        true
    }
}

/// Traces execution flows through the call graph.
///
/// Discovers entry-point symbols and performs DFS to produce process
/// descriptions that show how data/control flows through the codebase.
pub struct ProcessTracer;

impl ProcessTracer {
    /// Trace all processes in the given symbols and relationships.
    ///
    /// Returns a list of [`Process`] records, each describing a flow
    /// from an entry-point symbol through its callees.
    pub fn trace(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
    ) -> Result<Vec<Process>> {
        let call_rels: Vec<&Relationship> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .collect();

        if call_rels.is_empty() {
            return Ok(vec![]);
        }

        // Build adjacency: caller_uid -> [callee_uid]
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut is_callee: HashSet<&str> = HashSet::new();
        for rel in &call_rels {
            adj.entry(rel.source_uid.as_str())
                .or_default()
                .push(rel.target_uid.as_str());
            is_callee.insert(rel.target_uid.as_str());
        }

        let uid_to_symbol: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Entry points: symbols that call others but are not called themselves,
        // OR functions named main/handler/index/app/run
        let entry_points: Vec<&str> = symbols
            .iter()
            .filter(|s| {
                let uid = s.uid.as_str();
                let calls_something = adj.contains_key(uid);
                let not_called = !is_callee.contains(uid);
                let is_entry_name = matches!(
                    s.name.as_str(),
                    "main" | "handler" | "index" | "app" | "run" | "start" | "init"
                );
                (calls_something && not_called) || is_entry_name
            })
            .map(|s| s.uid.as_str())
            .collect();

        let mut processes = Vec::new();

        for entry_uid in &entry_points {
            let sym = match uid_to_symbol.get(entry_uid) {
                Some(s) => s,
                None => continue,
            };

            // DFS through call graph to trace the process
            let mut steps = Vec::new();
            let mut visited = HashSet::new();
            Self::dfs(entry_uid, &adj, &uid_to_symbol, &mut steps, &mut visited);

            if steps.len() < 2 {
                continue;
            }

            let description = steps
                .iter()
                .enumerate()
                .map(|(i, name)| format!("{}. {}", i + 1, name))
                .collect::<Vec<_>>()
                .join(" → ");

            processes.push(Process {
                uid: Uuid::new_v4().to_string(),
                name: format!("{} flow", sym.name),
                repo_id: repo_id.to_string(),
                entry_point: sym.name.clone(),
                step_count: steps.len() as u32,
                description,
            });
        }

        info!("Traced {} processes", processes.len());
        Ok(processes)
    }

    fn dfs<'a>(
        uid: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        uid_to_symbol: &HashMap<&'a str, &'a CodeSymbol>,
        steps: &mut Vec<String>,
        visited: &mut HashSet<&'a str>,
    ) {
        if visited.contains(uid) {
            return;
        }
        visited.insert(uid);

        if let Some(sym) = uid_to_symbol.get(uid) {
            steps.push(sym.name.clone());
        }

        if let Some(callees) = adj.get(uid) {
            for callee in callees {
                Self::dfs(callee, adj, uid_to_symbol, steps, visited);
            }
        }
    }
}
