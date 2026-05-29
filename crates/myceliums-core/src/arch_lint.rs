//! Architecture linting — static quality rules for codebase structure.
//!
//! Checks for common architectural anti-patterns using the knowledge graph:
//! circular dependencies, god nodes, isolated modules, high coupling,
//! unstable dependencies, and high fan-out.

use crate::cycles::detect_cycles;
use crate::dependencies::compute_module_coupling;
use crate::god_nodes::compute_god_nodes;
use myceliums_storage::{CodeSymbol, Relationship};
use serde::Serialize;

/// Severity level for lint findings.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintSeverity::Error => write!(f, "error"),
            LintSeverity::Warning => write!(f, "warning"),
            LintSeverity::Info => write!(f, "info"),
        }
    }
}

/// A single lint finding.
#[derive(Debug, Clone, Serialize)]
pub struct LintFinding {
    pub rule_id: String,
    pub severity: LintSeverity,
    pub message: String,
    pub affected_entities: Vec<String>,
}

/// Summary of all lint findings.
#[derive(Debug, Clone, Serialize)]
pub struct LintReport {
    pub findings: Vec<LintFinding>,
    pub rules_checked: Vec<String>,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}

const ALL_RULES: &[&str] = &[
    "circular_dependency",
    "god_node",
    "high_fan_out",
    "unstable_dependency",
];

/// Run architecture quality checks on the knowledge graph.
///
/// If `rules` is `Some`, only the listed rules are checked.
/// `god_node_threshold` defaults to 20 — symbols with degree above this are flagged.
pub fn lint_architecture(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    rules: Option<&[&str]>,
    god_node_threshold: u32,
) -> LintReport {
    let active_rules: Vec<&str> = match rules {
        Some(r) => r
            .iter()
            .filter(|rule| ALL_RULES.contains(rule))
            .copied()
            .collect(),
        None => ALL_RULES.to_vec(),
    };

    let mut findings = Vec::new();

    for rule in &active_rules {
        match *rule {
            "circular_dependency" => {
                check_circular_dependencies(symbols, relationships, &mut findings);
            }
            "god_node" => {
                check_god_nodes(symbols, relationships, god_node_threshold, &mut findings);
            }
            "high_fan_out" => {
                check_high_fan_out(symbols, relationships, god_node_threshold, &mut findings);
            }
            "unstable_dependency" => {
                check_unstable_dependencies(symbols, relationships, &mut findings);
            }
            _ => {}
        }
    }

    let error_count = findings
        .iter()
        .filter(|f| f.severity == LintSeverity::Error)
        .count();
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == LintSeverity::Warning)
        .count();
    let info_count = findings
        .iter()
        .filter(|f| f.severity == LintSeverity::Info)
        .count();

    LintReport {
        findings,
        rules_checked: active_rules.iter().map(|r| r.to_string()).collect(),
        error_count,
        warning_count,
        info_count,
    }
}

fn check_circular_dependencies(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    findings: &mut Vec<LintFinding>,
) {
    let cycles = match detect_cycles(symbols, relationships, true, true, 2) {
        Ok(c) => c,
        Err(_) => return,
    };
    for cycle in cycles {
        findings.push(LintFinding {
            rule_id: "circular_dependency".to_string(),
            severity: LintSeverity::Error,
            message: format!(
                "Circular dependency of {} symbols: {}",
                cycle.size,
                cycle.member_names.join(" → ")
            ),
            affected_entities: cycle.member_names,
        });
    }
}

fn check_god_nodes(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    threshold: u32,
    findings: &mut Vec<LintFinding>,
) {
    let god_nodes = compute_god_nodes(symbols, relationships, usize::MAX, threshold);
    for node in god_nodes {
        if node.is_high_coupling {
            findings.push(LintFinding {
                rule_id: "god_node".to_string(),
                severity: LintSeverity::Warning,
                message: format!(
                    "{} has {} connections (threshold: {}), consider splitting",
                    node.name, node.degree, threshold
                ),
                affected_entities: vec![node.name],
            });
        }
    }
}

fn check_high_fan_out(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    threshold: u32,
    findings: &mut Vec<LintFinding>,
) {
    let god_nodes = compute_god_nodes(symbols, relationships, usize::MAX, threshold);
    for node in god_nodes {
        if node.out_degree > threshold {
            findings.push(LintFinding {
                rule_id: "high_fan_out".to_string(),
                severity: LintSeverity::Warning,
                message: format!(
                    "{} calls {} other symbols (threshold: {}), consider extracting",
                    node.name, node.out_degree, threshold
                ),
                affected_entities: vec![node.name],
            });
        }
    }
}

fn check_unstable_dependencies(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    findings: &mut Vec<LintFinding>,
) {
    let coupling = compute_module_coupling(symbols, relationships, false);

    let stable_modules: Vec<&str> = coupling
        .iter()
        .filter(|c| c.instability < 0.3 && c.afferent > 0)
        .map(|c| c.module_path.as_str())
        .collect();

    let unstable_modules: Vec<&str> = coupling
        .iter()
        .filter(|c| c.instability > 0.7)
        .map(|c| c.module_path.as_str())
        .collect();

    // Check if any stable module imports from an unstable module
    for rel in relationships {
        if rel.kind != myceliums_storage::RelationshipKind::Imports {
            continue;
        }
        let src_file = symbols
            .iter()
            .find(|s| s.uid == rel.source_uid)
            .map(|s| s.file_path.as_str());
        let tgt_file = symbols
            .iter()
            .find(|s| s.uid == rel.target_uid)
            .map(|s| s.file_path.as_str());

        if let (Some(src), Some(tgt)) = (src_file, tgt_file) {
            if stable_modules.contains(&src) && unstable_modules.contains(&tgt) {
                findings.push(LintFinding {
                    rule_id: "unstable_dependency".to_string(),
                    severity: LintSeverity::Info,
                    message: format!("Stable module {} depends on unstable module {}", src, tgt),
                    affected_entities: vec![src.to_string(), tgt.to_string()],
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{RelationshipKind, SymbolKind};

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_call(source: &str, target: &str) -> Relationship {
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
    fn test_lint_empty_graph() {
        let report = lint_architecture(&[], &[], None, 20);
        assert!(report.findings.is_empty());
        assert_eq!(report.error_count, 0);
        assert_eq!(report.warning_count, 0);
        assert_eq!(report.info_count, 0);
    }

    #[test]
    fn test_lint_god_node() {
        let mut symbols = vec![make_symbol("hub", "HubNode")];
        let targets: Vec<CodeSymbol> = (0..25)
            .map(|i| make_symbol(&format!("t{}", i), &format!("target{}", i)))
            .collect();
        symbols.extend(targets);

        let rels: Vec<Relationship> = (0..25)
            .map(|i| make_call(&format!("t{}", i), "hub"))
            .collect();

        let report = lint_architecture(&symbols, &rels, Some(&["god_node"]), 20);
        assert!(report.findings.iter().any(|f| f.rule_id == "god_node"));
        assert!(report.warning_count > 0);
    }

    #[test]
    fn test_lint_circular_dependency() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "c"),
            make_call("c", "a"),
        ];

        let report = lint_architecture(&symbols, &rels, Some(&["circular_dependency"]), 20);
        assert!(report
            .findings
            .iter()
            .any(|f| f.rule_id == "circular_dependency"));
        assert!(report.error_count > 0);
    }

    #[test]
    fn test_lint_rule_filter() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "c"),
            make_call("c", "a"),
        ];

        // Only run god_node rule — should not find circular dependency
        let report = lint_architecture(&symbols, &rels, Some(&["god_node"]), 20);
        assert!(!report
            .findings
            .iter()
            .any(|f| f.rule_id == "circular_dependency"));
        assert_eq!(report.rules_checked, vec!["god_node"]);
    }

    #[test]
    fn test_lint_severity_counts() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // Create a cycle (error) but no god node at threshold 20 (warning)
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "c"),
            make_call("c", "a"),
        ];

        let report = lint_architecture(&symbols, &rels, None, 20);
        assert_eq!(
            report.error_count + report.warning_count + report.info_count,
            report.findings.len()
        );
    }
}
