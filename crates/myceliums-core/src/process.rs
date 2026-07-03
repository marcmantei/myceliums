//! Process tracing — discovers execution flows through the call graph.
//!
//! [`ProcessTracer`] walks from entry-point symbols (e.g. `main`, `handler`)
//! through the call graph via DFS, producing a list of [`Process`]
//! records that describe end-to-end code flows.
//!
//! ## Branching Structure
//!
//! The tracer preserves the branching structure of the call tree. Each call path
//! is rendered as an indented branch in the process description, making the
//! control flow explicit. Shared callees (functions called from multiple callers)
//! may appear under multiple branches.
//!
//! ## Depth and Node Limits
//!
//! To prevent unbounded exploration of deep graphs, the tracer enforces:
//! - **Max depth**: Maximum nesting level (default: 10)
//! - **Max nodes per process**: Total nodes to explore (default: 100)
//!
//! These limits are configurable via [`ProcessTraceConfig`].

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Process, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use tracing::info;
use uuid::Uuid;

/// Configuration for process tracing behavior.
#[derive(Debug, Clone)]
pub struct ProcessTraceConfig {
    /// Maximum depth (nesting level) to traverse. Default: 10.
    pub max_depth: usize,
    /// Maximum number of nodes per process to trace. Default: 100.
    pub max_nodes_per_process: usize,
}

impl Default for ProcessTraceConfig {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_nodes_per_process: 100,
        }
    }
}

/// A node in the call tree representing a function and its branching callees.
#[derive(Debug, Clone)]
struct CallTreeNode {
    /// Name of the function/symbol
    name: String,
    /// Depth in the tree (root is 0)
    depth: usize,
    /// Children (callees) of this node
    children: Vec<CallTreeNode>,
}

impl CallTreeNode {
    /// Create a new call tree node
    fn new(name: String, depth: usize) -> Self {
        Self {
            name,
            depth,
            children: Vec::new(),
        }
    }

    /// Render the tree to a formatted string with indentation and branch markers
    fn render_with_branches(&self, indent: usize) -> String {
        let mut result = String::new();
        let prefix = "  ".repeat(indent);

        result.push_str(&format!("{}├─ {}\n", prefix, self.name));

        for (_i, child) in self.children.iter().enumerate() {
            let child_str = child.render_with_branches(indent + 1);
            result.push_str(&child_str);
        }

        result
    }

    /// Count total nodes in the tree
    fn count_nodes(&self) -> usize {
        1 + self.children.iter().map(|c| c.count_nodes()).sum::<usize>()
    }

    /// Render tree as a simple chain (for backward compatibility)
    #[allow(dead_code)]
    fn render_as_chain(&self) -> Vec<String> {
        let mut chain = vec![self.name.clone()];
        if !self.children.is_empty() {
            // For the first child, continue the chain; for others, branch
            let first_child_chain = self.children[0].render_as_chain();
            chain.extend(first_child_chain);
        }
        chain
    }
}

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
/// 
/// The tracer preserves the branching structure by building a tree of calls
/// and rendering it with clear indentation and branch markers. Shared callees
/// may appear under multiple branches (since each branch has its own path).
pub struct ProcessTracer;

impl ProcessTracer {
    /// Trace all processes in the given symbols and relationships.
    ///
    /// Returns a list of [`Process`] records, each describing a flow
    /// from an entry-point symbol through its callees. Uses default
    /// configuration for depth and node limits.
    pub fn trace(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
    ) -> Result<Vec<Process>> {
        Self::trace_with_config(symbols, relationships, repo_id, ProcessTraceConfig::default())
    }

    /// Trace all processes with custom configuration.
    ///
    /// Allows specifying custom max_depth and max_nodes_per_process limits.
    pub fn trace_with_config(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
        config: ProcessTraceConfig,
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

            // Build tree structure through call graph
            let tree = Self::build_tree(
                entry_uid,
                &adj,
                &uid_to_symbol,
                0,
                &config,
                &mut HashSet::new(),
            );

            if tree.count_nodes() < 2 {
                continue;
            }

            // Render tree as formatted branches
            let tree_description = tree.render_with_branches(0);

            processes.push(Process {
                uid: Uuid::new_v4().to_string(),
                name: format!("{} flow", sym.name),
                repo_id: repo_id.to_string(),
                entry_point: sym.name.clone(),
                step_count: tree.count_nodes() as u32,
                description: tree_description,
            });
        }

        info!("Traced {} processes", processes.len());
        Ok(processes)
    }

    /// Build a call tree structure preserving branching.
    ///
    /// Performs DFS but maintains the tree structure instead of flattening.
    /// Each branch can contain the same callee if it's reachable from different
    /// call paths. Respects max_depth and max_nodes_per_process limits.
    fn build_tree<'a>(
        uid: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        uid_to_symbol: &HashMap<&'a str, &'a CodeSymbol>,
        depth: usize,
        config: &ProcessTraceConfig,
        visited_in_path: &mut HashSet<&'a str>,
    ) -> CallTreeNode {
        let sym_name = uid_to_symbol
            .get(uid)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let mut node = CallTreeNode::new(sym_name, depth);

        // Check depth limit
        if depth >= config.max_depth {
            return node;
        }

        // Avoid infinite recursion in a single path (cycles within the same branch)
        if visited_in_path.contains(uid) {
            return node;
        }

        visited_in_path.insert(uid);

        // Add callees as children, allowing shared callees under multiple branches
        if let Some(callees) = adj.get(uid) {
            for callee in callees {
                // Stop if we've hit max nodes
                if node.count_nodes() >= config.max_nodes_per_process {
                    break;
                }

                let child_tree = Self::build_tree(
                    callee,
                    adj,
                    uid_to_symbol,
                    depth + 1,
                    config,
                    visited_in_path,
                );
                node.children.push(child_tree);
            }
        }

        visited_in_path.remove(uid);
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: myceliums_storage::SymbolKind::Function,
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 1,
            signature: "".to_string(),
            content: "".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }
    }

    fn make_call_rel(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test-repo".to_string(),
            metadata: "".to_string(),
        }
    }

    #[test]
    fn test_simple_linear_chain() {
        // main -> foo -> bar
        let symbols = vec![
            make_symbol("1", "main"),
            make_symbol("2", "foo"),
            make_symbol("3", "bar"),
        ];

        let relationships = vec![
            make_call_rel("1", "2"),
            make_call_rel("2", "3"),
        ];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].entry_point, "main");
        assert_eq!(processes[0].step_count, 3);
        // Verify description contains all three functions
        assert!(processes[0].description.contains("main"));
        assert!(processes[0].description.contains("foo"));
        assert!(processes[0].description.contains("bar"));
    }

    #[test]
    fn test_branching_structure() {
        // main -> foo
        //      -> bar
        let symbols = vec![
            make_symbol("1", "main"),
            make_symbol("2", "foo"),
            make_symbol("3", "bar"),
        ];

        let relationships = vec![
            make_call_rel("1", "2"),
            make_call_rel("1", "3"),
        ];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].entry_point, "main");
        // Should have 3 nodes: main, foo, bar
        assert_eq!(processes[0].step_count, 3);
        // Verify both branches are in description
        assert!(processes[0].description.contains("main"));
        assert!(processes[0].description.contains("foo"));
        assert!(processes[0].description.contains("bar"));
    }

    #[test]
    fn test_depth_limit() {
        // Create a deep chain: main -> a -> b -> c -> d -> e -> f -> g -> h -> i -> j -> k
        let mut symbols = vec![make_symbol("1", "main")];
        let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
        for (i, name) in names.iter().enumerate() {
            symbols.push(make_symbol(&format!("{}", i + 2), name));
        }

        let mut relationships = vec![];
        for i in 0..names.len() {
            relationships.push(make_call_rel(&i.to_string(), &(i + 1).to_string()));
        }

        // Use a config with max_depth = 5
        let config = ProcessTraceConfig {
            max_depth: 5,
            max_nodes_per_process: 1000,
        };

        let processes =
            ProcessTracer::trace_with_config(&symbols, &relationships, "test-repo", config)
                .unwrap();

        assert_eq!(processes.len(), 1);
        // Should stop at depth 5, so: main -> a -> b -> c -> d (5 nodes total, depth goes 0-4)
        assert!(processes[0].step_count <= 6, "Expected <= 6 nodes with max_depth=5, got {}", processes[0].step_count);
    }

    #[test]
    fn test_max_nodes_per_process() {
        // Create a tree with many branches
        // main -> a, b, c, d, e
        let mut symbols = vec![make_symbol("1", "main")];
        let names = ["a", "b", "c", "d", "e", "f", "g"];
        for (i, name) in names.iter().enumerate() {
            symbols.push(make_symbol(&format!("{}", i + 2), name));
        }

        let mut relationships = vec![];
        // main calls a, b, c, d, e, f, g (7 callees)
        for i in 0..names.len() {
            relationships.push(make_call_rel("1", &(i + 2).to_string()));
        }

        let config = ProcessTraceConfig {
            max_depth: 10,
            max_nodes_per_process: 5,
        };

        let processes =
            ProcessTracer::trace_with_config(&symbols, &relationships, "test-repo", config)
                .unwrap();

        assert_eq!(processes.len(), 1);
        // Should limit to max_nodes_per_process
        assert!(
            processes[0].step_count <= 5,
            "Expected <= 5 nodes with max_nodes_per_process=5, got {}",
            processes[0].step_count
        );
    }

    #[test]
    fn test_shared_callees_in_different_branches() {
        // main -> foo -> shared
        //      -> bar -> shared
        // Both paths should include "shared" under their respective branches
        let symbols = vec![
            make_symbol("1", "main"),
            make_symbol("2", "foo"),
            make_symbol("3", "bar"),
            make_symbol("4", "shared"),
        ];

        let relationships = vec![
            make_call_rel("1", "2"),
            make_call_rel("1", "3"),
            make_call_rel("2", "4"),
            make_call_rel("3", "4"),
        ];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();

        assert_eq!(processes.len(), 1);
        // Should have: main, foo, bar, shared (from both paths)
        // When shared is reached through foo, it's included; when through bar, it's also there
        // But using the tree structure, we get main -> [foo, bar] where foo -> [shared], bar -> [shared]
        // Count: 1 (main) + 1 (foo) + 1 (bar) + 2 (shared in each branch) = 5
        assert!(processes[0].step_count >= 4, "Expected at least 4 nodes (main, foo, bar, shared), got {}", processes[0].step_count);
    }

    #[test]
    fn test_cycle_handling() {
        // main -> foo -> bar -> foo (cycle)
        // The tracer should avoid infinite loops
        let symbols = vec![
            make_symbol("1", "main"),
            make_symbol("2", "foo"),
            make_symbol("3", "bar"),
        ];

        let relationships = vec![
            make_call_rel("1", "2"),
            make_call_rel("2", "3"),
            make_call_rel("3", "2"), // cycle back to foo
        ];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();

        assert_eq!(processes.len(), 1);
        // Should have: main, foo, bar (and foo should not be revisited in the same path)
        assert_eq!(processes[0].step_count, 3, "Expected 3 nodes with cycle, got {}", processes[0].step_count);
    }

    #[test]
    fn test_no_entry_points() {
        let symbols = vec![];
        let relationships = vec![];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();
        assert_eq!(processes.len(), 0);
    }

    #[test]
    fn test_single_function_no_calls() {
        // Single function that doesn't call anything
        let symbols = vec![make_symbol("1", "main")];
        let relationships = vec![];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();
        // Should not create a process (requires at least 2 steps)
        assert_eq!(processes.len(), 0);
    }

    #[test]
    fn test_entry_point_detection_by_name() {
        // foo calls bar, but foo is never called
        // and main (not in the graph) should not be found
        // But if we have a symbol named "handler", it should be an entry point
        let symbols = vec![
            make_symbol("1", "handler"),
            make_symbol("2", "authenticate"),
        ];

        let relationships = vec![make_call_rel("1", "2")];

        let processes = ProcessTracer::trace(&symbols, &relationships, "test-repo").unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].entry_point, "handler");
    }

    #[test]
    fn test_call_tree_node_render() {
        let mut root = CallTreeNode::new("main".to_string(), 0);
        let mut foo = CallTreeNode::new("foo".to_string(), 1);
        let bar = CallTreeNode::new("bar".to_string(), 2);
        foo.children.push(bar);
        root.children.push(foo);

        let rendered = root.render_with_branches(0);
        assert!(rendered.contains("main"));
        assert!(rendered.contains("foo"));
        assert!(rendered.contains("bar"));
        assert!(rendered.contains("├─"));
    }

    #[test]
    fn test_call_tree_node_count() {
        let mut root = CallTreeNode::new("main".to_string(), 0);
        let mut foo = CallTreeNode::new("foo".to_string(), 1);
        let bar = CallTreeNode::new("bar".to_string(), 2);
        foo.children.push(bar);
        root.children.push(foo);

        assert_eq!(root.count_nodes(), 3);
    }
}
