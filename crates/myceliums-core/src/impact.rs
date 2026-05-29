//! Impact analysis for code changes.
//!
//! Given a unified diff, [`detect_impact`] identifies which symbols were
//! directly modified and then walks the call graph to find indirectly
//! affected symbols up to a configurable depth.

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use tracing::info;

/// Type of change detected in a diff.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ChangeType {
    /// A newly added symbol.
    Added,
    /// An existing symbol whose content changed.
    Modified,
    /// A symbol that was removed.
    Deleted,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Added => write!(f, "Added"),
            ChangeType::Modified => write!(f, "Modified"),
            ChangeType::Deleted => write!(f, "Deleted"),
        }
    }
}

/// A symbol directly changed by the diff.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedSymbol {
    /// Symbol name (e.g. `"authenticate"`).
    pub name: String,
    /// Fully qualified name (e.g. `"auth::authenticate"`).
    pub qualified_name: String,
    /// Symbol kind (e.g. `"Function"`, `"Class"`).
    pub kind: String,
    /// Path to the file containing this symbol.
    pub file_path: String,
    /// How this symbol was changed.
    pub change_type: ChangeType,
}

/// A symbol indirectly affected via the call graph.
#[derive(Debug, Clone, Serialize)]
pub struct AffectedSymbol {
    /// Symbol name.
    pub name: String,
    /// Fully qualified name.
    pub qualified_name: String,
    /// Symbol kind.
    pub kind: String,
    /// Path to the file containing this symbol.
    pub file_path: String,
    /// Graph distance from the nearest directly-changed symbol.
    pub distance: u32,
    /// Direction of the relationship (`"caller"` or `"callee"`).
    pub relationship: String,
}

/// Full impact report for a set of code changes.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    /// Symbols whose source lines overlap with the diff hunks.
    pub directly_changed: Vec<ChangedSymbol>,
    /// Symbols reachable through the call graph from directly-changed symbols.
    pub indirectly_affected: Vec<AffectedSymbol>,
    /// Sorted list of file paths containing any affected symbol.
    pub affected_files: Vec<String>,
    /// Normalized risk score from 0.0 (no risk) to 10.0 (high risk).
    pub risk_score: f64,
}

/// A hunk from a unified diff: file path and changed line ranges
#[derive(Debug, Clone)]
struct DiffHunk {
    file_path: String,
    /// Ranges of changed lines in the new version (post-change)
    new_ranges: Vec<(u32, u32)>,
    /// Whether the file is entirely new
    is_new: bool,
    /// Whether the file was deleted
    is_deleted: bool,
}

/// Parse a unified diff string into structured hunks.
fn parse_diff(diff: &str) -> Vec<DiffHunk> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_ranges: Vec<(u32, u32)> = Vec::new();
    let mut is_new = false;
    let mut is_deleted = false;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // Flush previous file
            if let Some(file) = current_file.take() {
                hunks.push(DiffHunk {
                    file_path: file,
                    new_ranges: std::mem::take(&mut current_ranges),
                    is_new,
                    is_deleted,
                });
            }
            is_new = false;
            is_deleted = false;
        } else if let Some(stripped) = line.strip_prefix("+++ b/") {
            current_file = Some(stripped.to_string());
        } else if line.starts_with("+++ /dev/null") {
            is_deleted = true;
        } else if line.starts_with("--- /dev/null") {
            is_new = true;
        } else if line.starts_with("@@ ") {
            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            if let Some(range) = parse_hunk_header(line) {
                current_ranges.push(range);
            }
        }
    }

    // Flush last file
    if let Some(file) = current_file {
        hunks.push(DiffHunk {
            file_path: file,
            new_ranges: current_ranges,
            is_new,
            is_deleted,
        });
    }

    hunks
}

/// Parse a hunk header like `@@ -10,5 +12,7 @@` and return the new-side range (start, end).
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    // Find the +start,count portion
    let plus_idx = line.find('+')?;
    let rest = &line[plus_idx + 1..];
    let end_idx = rest.find([' ', '@'])?;
    let range_str = &rest[..end_idx];

    let parts: Vec<&str> = range_str.split(',').collect();
    let start: u32 = parts.first()?.parse().ok()?;
    let count: u32 = if parts.len() > 1 {
        parts[1].parse().ok()?
    } else {
        1
    };

    if count == 0 {
        return None;
    }

    Some((start, start + count - 1))
}

/// Run `git diff HEAD` in the given directory and return the diff string.
pub fn run_git_diff(repo_path: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Check if a line range overlaps with a symbol's line range.
fn ranges_overlap(range_start: u32, range_end: u32, sym_start: u32, sym_end: u32) -> bool {
    range_start <= sym_end && range_end >= sym_start
}

/// Detect the impact of a diff on the codebase.
pub fn detect_impact(
    diff: &str,
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    depth: u32,
) -> ImpactReport {
    let hunks = parse_diff(diff);

    if hunks.is_empty() {
        return ImpactReport {
            directly_changed: vec![],
            indirectly_affected: vec![],
            affected_files: vec![],
            risk_score: 0.0,
        };
    }

    // Build lookup: file_path -> symbols in that file
    let mut file_symbols: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    for sym in symbols {
        file_symbols
            .entry(sym.file_path.as_str())
            .or_default()
            .push(sym);
    }

    // Build adjacency from CALLS + IMPORTS relationships (both directions)
    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new(); // target -> sources (callers + importers)
    let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new(); // source -> targets (callees + imported)

    for rel in relationships {
        if rel.kind == RelationshipKind::Calls || rel.kind == RelationshipKind::Imports {
            callers_of
                .entry(rel.target_uid.as_str())
                .or_default()
                .push(rel.source_uid.as_str());
            callees_of
                .entry(rel.source_uid.as_str())
                .or_default()
                .push(rel.target_uid.as_str());
        }
    }

    // Step 1: Find directly changed symbols
    let mut directly_changed: Vec<ChangedSymbol> = Vec::new();
    let mut directly_changed_uids: HashSet<String> = HashSet::new();
    let mut affected_files: HashSet<String> = HashSet::new();

    for hunk in &hunks {
        affected_files.insert(hunk.file_path.clone());

        let change_type = if hunk.is_new {
            ChangeType::Added
        } else if hunk.is_deleted {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        };

        // Find symbols in this file whose line ranges overlap with changed ranges
        let matching_symbols = find_matching_symbols(&file_symbols, &hunk.file_path);

        for sym in matching_symbols {
            let overlaps = if hunk.is_new || hunk.is_deleted {
                true // All symbols in new/deleted files are affected
            } else {
                hunk.new_ranges
                    .iter()
                    .any(|(start, end)| ranges_overlap(*start, *end, sym.start_line, sym.end_line))
            };

            if overlaps && !directly_changed_uids.contains(&sym.uid) {
                directly_changed_uids.insert(sym.uid.clone());
                directly_changed.push(ChangedSymbol {
                    name: sym.name.clone(),
                    qualified_name: sym.qualified_name.clone(),
                    kind: sym.kind.to_string(),
                    file_path: sym.file_path.clone(),
                    change_type: change_type.clone(),
                });
            }
        }
    }

    info!("Found {} directly changed symbols", directly_changed.len());

    // Step 2: BFS through call graph to find indirectly affected symbols
    let mut indirectly_affected: Vec<AffectedSymbol> = Vec::new();
    let mut visited: HashSet<String> = directly_changed_uids.clone();

    // BFS queue: (uid, distance, relationship_direction)
    let mut queue: VecDeque<(String, u32, &str)> = VecDeque::new();

    // Seed the queue with direct neighbors of changed symbols
    for uid in &directly_changed_uids {
        // Callers of this symbol (upstream impact)
        if let Some(callers) = callers_of.get(uid.as_str()) {
            for caller_uid in callers {
                if !visited.contains(*caller_uid) {
                    queue.push_back((caller_uid.to_string(), 1, "caller"));
                }
            }
        }
        // Callees of this symbol (downstream impact)
        if let Some(callees) = callees_of.get(uid.as_str()) {
            for callee_uid in callees {
                if !visited.contains(*callee_uid) {
                    queue.push_back((callee_uid.to_string(), 1, "callee"));
                }
            }
        }
    }

    while let Some((uid, dist, rel_label)) = queue.pop_front() {
        if dist > depth || visited.contains(&uid) {
            continue;
        }
        visited.insert(uid.clone());

        if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
            affected_files.insert(sym.file_path.clone());
            indirectly_affected.push(AffectedSymbol {
                name: sym.name.clone(),
                qualified_name: sym.qualified_name.clone(),
                kind: sym.kind.to_string(),
                file_path: sym.file_path.clone(),
                distance: dist,
                relationship: rel_label.to_string(),
            });

            // Continue traversal if within depth
            if dist < depth {
                if let Some(callers) = callers_of.get(uid.as_str()) {
                    for caller_uid in callers {
                        if !visited.contains(*caller_uid) {
                            queue.push_back((caller_uid.to_string(), dist + 1, "caller"));
                        }
                    }
                }
                if let Some(callees) = callees_of.get(uid.as_str()) {
                    for callee_uid in callees {
                        if !visited.contains(*callee_uid) {
                            queue.push_back((callee_uid.to_string(), dist + 1, "callee"));
                        }
                    }
                }
            }
        }
    }

    info!(
        "Found {} indirectly affected symbols",
        indirectly_affected.len()
    );

    // Step 3: Compute risk score
    let direct_count = directly_changed.len() as f64;
    let indirect_count = indirectly_affected.len() as f64;
    let file_count = affected_files.len() as f64;

    // Count downstream dependents (callers) of changed symbols
    let downstream_count: f64 = directly_changed_uids
        .iter()
        .map(|uid| callers_of.get(uid.as_str()).map(|c| c.len()).unwrap_or(0) as f64)
        .sum();

    // Risk score: normalized to 0-10 scale
    let raw_score =
        direct_count * 1.0 + indirect_count * 0.5 + downstream_count * 0.3 + file_count * 0.2;
    let risk_score = (raw_score.min(50.0) / 50.0 * 10.0 * 100.0).round() / 100.0;

    let mut affected_files_vec: Vec<String> = affected_files.into_iter().collect();
    affected_files_vec.sort();

    ImpactReport {
        directly_changed,
        indirectly_affected,
        affected_files: affected_files_vec,
        risk_score,
    }
}

/// Find symbols that match a diff file path.
/// Handles cases where stored paths may be relative or absolute.
fn find_matching_symbols<'a>(
    file_symbols: &HashMap<&str, Vec<&'a CodeSymbol>>,
    diff_path: &str,
) -> Vec<&'a CodeSymbol> {
    // Direct match
    if let Some(syms) = file_symbols.get(diff_path) {
        return syms.clone();
    }

    // Try suffix matching (diff path might be relative, stored path absolute or vice versa)
    for (stored_path, syms) in file_symbols {
        if stored_path.ends_with(diff_path) || diff_path.ends_with(stored_path) {
            return syms.clone();
        }
    }

    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

    fn make_symbol(uid: &str, name: &str, file: &str, start: u32, end: u32) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("{}::{}", file, name),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: start,
            end_line: end,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_call_rel(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(
            parse_hunk_header("@@ -10,5 +12,7 @@ fn foo"),
            Some((12, 18))
        );
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,3 @@"), Some((1, 3)));
    }

    #[test]
    fn test_parse_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@ fn main() {
+    println!("hello");
+    println!("world");
"#;
        let hunks = parse_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, "src/main.rs");
        assert_eq!(hunks[0].new_ranges, vec![(10, 14)]);
        assert!(!hunks[0].is_new);
        assert!(!hunks[0].is_deleted);
    }

    #[test]
    fn test_detect_impact_direct() {
        let symbols = vec![
            make_symbol("s1", "foo", "src/main.rs", 5, 15),
            make_symbol("s2", "bar", "src/main.rs", 20, 30),
            make_symbol("s3", "baz", "src/lib.rs", 1, 10),
        ];
        let relationships = vec![];

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@
+changed
"#;
        let report = detect_impact(diff, &symbols, &relationships, 2);
        assert_eq!(report.directly_changed.len(), 1);
        assert_eq!(report.directly_changed[0].name, "foo");
    }

    #[test]
    fn test_detect_impact_indirect() {
        let symbols = vec![
            make_symbol("s1", "foo", "src/main.rs", 5, 15),
            make_symbol("s2", "bar", "src/main.rs", 20, 30),
            make_symbol("s3", "baz", "src/lib.rs", 1, 10),
        ];
        // bar calls foo, baz calls bar
        let relationships = vec![make_call_rel("s2", "s1"), make_call_rel("s3", "s2")];

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@
+changed
"#;
        let report = detect_impact(diff, &symbols, &relationships, 2);
        assert_eq!(report.directly_changed.len(), 1);
        assert_eq!(report.directly_changed[0].name, "foo");
        // bar is a caller of foo (distance 1), baz is a caller of bar (distance 2)
        assert!(!report.indirectly_affected.is_empty());
        assert!(report.indirectly_affected.iter().any(|a| a.name == "bar"));
    }

    #[test]
    fn test_empty_diff() {
        let report = detect_impact("", &[], &[], 2);
        assert!(report.directly_changed.is_empty());
        assert!(report.indirectly_affected.is_empty());
        assert_eq!(report.risk_score, 0.0);
    }

    fn make_import_rel(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("imp_{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Imports,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_impact_via_imports() {
        let symbols = vec![
            make_symbol("s1", "foo", "src/main.rs", 5, 15),
            make_symbol("s2", "bar", "src/other.rs", 1, 10),
        ];
        // bar imports foo
        let relationships = vec![make_import_rel("s2", "s1")];

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@
+changed
"#;
        let report = detect_impact(diff, &symbols, &relationships, 2);
        assert_eq!(report.directly_changed.len(), 1);
        assert_eq!(report.directly_changed[0].name, "foo");
        // bar should be indirectly affected via import
        assert!(
            report.indirectly_affected.iter().any(|a| a.name == "bar"),
            "bar should be affected via import: {:?}",
            report.indirectly_affected
        );
    }

    #[test]
    fn test_impact_calls_and_imports_combined() {
        let symbols = vec![
            make_symbol("s1", "foo", "src/main.rs", 5, 15),
            make_symbol("s2", "bar", "src/caller.rs", 1, 10),
            make_symbol("s3", "baz", "src/importer.rs", 1, 10),
        ];
        // bar calls foo, baz imports foo
        let relationships = vec![make_call_rel("s2", "s1"), make_import_rel("s3", "s1")];

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@
+changed
"#;
        let report = detect_impact(diff, &symbols, &relationships, 2);
        assert_eq!(report.directly_changed.len(), 1);
        // Both bar (caller) and baz (importer) should be affected
        let affected_names: Vec<&str> = report
            .indirectly_affected
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert!(
            affected_names.contains(&"bar"),
            "bar should be affected via call"
        );
        assert!(
            affected_names.contains(&"baz"),
            "baz should be affected via import"
        );
    }

    #[test]
    fn test_impact_depth_limit_with_imports() {
        let symbols = vec![
            make_symbol("s1", "foo", "src/main.rs", 5, 15),
            make_symbol("s2", "bar", "src/mid.rs", 1, 10),
            make_symbol("s3", "baz", "src/far.rs", 1, 10),
        ];
        // Chain: baz imports bar, bar imports foo. Modify foo with depth=1.
        let relationships = vec![make_import_rel("s2", "s1"), make_import_rel("s3", "s2")];

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@
+changed
"#;
        // depth=1: only bar should be affected, not baz
        let report = detect_impact(diff, &symbols, &relationships, 1);
        assert!(
            report.indirectly_affected.iter().any(|a| a.name == "bar"),
            "bar at depth 1 should be affected"
        );
        assert!(
            !report.indirectly_affected.iter().any(|a| a.name == "baz"),
            "baz at depth 2 should NOT be affected with depth=1"
        );
    }
}
