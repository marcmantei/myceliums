//! Exports the knowledge graph in Mermaid diagram format.
//!
//! Produces Mermaid-compatible diagram strings for rendering as flowcharts,
//! class diagrams, or community-grouped graphs.

use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The type of Mermaid diagram to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MermaidDiagramType {
    /// Call graph rendered as a left-to-right flowchart.
    Flowchart,
    /// Classes and interfaces with their members.
    ClassDiagram,
    /// General graph with community-based subgraphs.
    Graph,
}

/// Escape characters that break Mermaid syntax inside labels.
fn mermaid_escape(s: &str) -> String {
    s.replace('"', "#quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('&', "&amp;")
}

/// Make a valid Mermaid node ID from a UID (alphanumeric + underscore only).
fn mermaid_id(uid: &str) -> String {
    uid.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Export symbols and relationships as a Mermaid diagram string.
///
/// `diagram_type` controls the output format:
/// - `Flowchart` — call graph as `flowchart LR` with arrows
/// - `ClassDiagram` — classes/interfaces with members as `classDiagram`
/// - `Graph` — community-grouped subgraphs (requires symbols to have community
///   assignments via metadata; falls back to flat graph if communities unavailable)
pub fn export_mermaid(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    diagram_type: MermaidDiagramType,
) -> String {
    match diagram_type {
        MermaidDiagramType::Flowchart => export_flowchart(symbols, relationships),
        MermaidDiagramType::ClassDiagram => export_class_diagram(symbols, relationships),
        MermaidDiagramType::Graph => export_graph(symbols, relationships),
    }
}

/// Export as a community-grouped graph using the provided community label map.
pub fn export_mermaid_with_communities(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    uid_to_community: &HashMap<String, String>,
) -> String {
    let mut out = String::from("graph LR\n");

    // Group symbols by community
    let mut communities: BTreeMap<String, Vec<&CodeSymbol>> = BTreeMap::new();
    let mut uncategorized: Vec<&CodeSymbol> = Vec::new();

    for sym in symbols {
        if let Some(label) = uid_to_community.get(&sym.uid) {
            communities.entry(label.clone()).or_default().push(sym);
        } else {
            uncategorized.push(sym);
        }
    }

    // Emit subgraphs
    for (label, members) in &communities {
        let safe_label = mermaid_escape(label);
        out.push_str(&format!("  subgraph {}\n", safe_label));
        for sym in members {
            let id = mermaid_id(&sym.uid);
            let name = mermaid_escape(&sym.name);
            out.push_str(&format!("    {}[\"{}\"]\n", id, name));
        }
        out.push_str("  end\n");
    }

    // Emit uncategorized symbols
    for sym in &uncategorized {
        let id = mermaid_id(&sym.uid);
        let name = mermaid_escape(&sym.name);
        out.push_str(&format!("  {}[\"{}\"]\n", id, name));
    }

    // Emit edges (only CALLS and IMPORTS)
    let valid_uids: BTreeSet<&str> = symbols.iter().map(|s| s.uid.as_str()).collect();
    for rel in relationships {
        if !matches!(
            rel.kind,
            RelationshipKind::Calls | RelationshipKind::Imports
        ) {
            continue;
        }
        if !valid_uids.contains(rel.source_uid.as_str())
            || !valid_uids.contains(rel.target_uid.as_str())
        {
            continue;
        }
        let src = mermaid_id(&rel.source_uid);
        let tgt = mermaid_id(&rel.target_uid);
        let label = rel.kind.to_string();
        out.push_str(&format!("  {} -->|{}| {}\n", src, label, tgt));
    }

    out
}

fn export_flowchart(symbols: &[CodeSymbol], relationships: &[Relationship]) -> String {
    let mut out = String::from("flowchart LR\n");

    let uid_to_name: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.name.as_str()))
        .collect();

    // Collect all UIDs that participate in CALLS edges
    let mut referenced_uids: BTreeSet<&str> = BTreeSet::new();
    let calls: Vec<&Relationship> = relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter(|r| {
            uid_to_name.contains_key(r.source_uid.as_str())
                && uid_to_name.contains_key(r.target_uid.as_str())
        })
        .collect();

    for rel in &calls {
        referenced_uids.insert(&rel.source_uid);
        referenced_uids.insert(&rel.target_uid);
    }

    // Emit nodes
    for uid in &referenced_uids {
        let id = mermaid_id(uid);
        let name = mermaid_escape(uid_to_name.get(uid).unwrap_or(&"?"));
        out.push_str(&format!("  {}[\"{}\"]\n", id, name));
    }

    // Emit edges
    for rel in &calls {
        let src = mermaid_id(&rel.source_uid);
        let tgt = mermaid_id(&rel.target_uid);
        out.push_str(&format!("  {} --> {}\n", src, tgt));
    }

    out
}

fn export_class_diagram(symbols: &[CodeSymbol], relationships: &[Relationship]) -> String {
    let mut out = String::from("classDiagram\n");

    // Find class/interface symbols
    let classes: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| matches!(s.kind, SymbolKind::Class | SymbolKind::Interface))
        .collect();

    // Build class UID -> members map via MemberOf relationships
    let mut class_members: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    let uid_to_sym: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    for rel in relationships {
        if rel.kind != RelationshipKind::MemberOf {
            continue;
        }
        // source is the member, target is the class
        if let Some(member) = uid_to_sym.get(rel.source_uid.as_str()) {
            class_members
                .entry(rel.target_uid.as_str())
                .or_default()
                .push(member);
        }
    }

    for cls in &classes {
        let name = mermaid_escape(&cls.name);
        // Mermaid uses "class" keyword for both classes and interfaces
        out.push_str(&format!("  class {} {{\n", name));

        if let Some(members) = class_members.get(cls.uid.as_str()) {
            for member in members {
                let prefix = match member.kind {
                    SymbolKind::Method | SymbolKind::Function => "+",
                    SymbolKind::Variable | SymbolKind::Constant => "-",
                    _ => "",
                };
                let member_name = mermaid_escape(&member.name);
                out.push_str(&format!("    {}{}()\n", prefix, member_name));
            }
        }

        out.push_str("  }\n");
    }

    // Emit inheritance relationships
    for rel in relationships {
        if rel.kind != RelationshipKind::Imports {
            continue;
        }
        let src = uid_to_sym.get(rel.source_uid.as_str());
        let tgt = uid_to_sym.get(rel.target_uid.as_str());
        if let (Some(src_sym), Some(tgt_sym)) = (src, tgt) {
            if matches!(src_sym.kind, SymbolKind::Class | SymbolKind::Interface)
                && matches!(tgt_sym.kind, SymbolKind::Class | SymbolKind::Interface)
            {
                let src_name = mermaid_escape(&src_sym.name);
                let tgt_name = mermaid_escape(&tgt_sym.name);
                out.push_str(&format!("  {} --|> {}\n", src_name, tgt_name));
            }
        }
    }

    // Mark interfaces
    for cls in &classes {
        if cls.kind == SymbolKind::Interface {
            let name = mermaid_escape(&cls.name);
            out.push_str(&format!("  <<interface>> {}\n", name));
        }
    }

    out
}

fn export_graph(symbols: &[CodeSymbol], relationships: &[Relationship]) -> String {
    // Without community data, fall back to a flat graph
    let mut out = String::from("graph LR\n");

    let uid_to_name: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.name.as_str()))
        .collect();

    // Emit nodes
    for sym in symbols {
        let id = mermaid_id(&sym.uid);
        let name = mermaid_escape(&sym.name);
        out.push_str(&format!("  {}[\"{}\"]\n", id, name));
    }

    // Emit edges (CALLS and IMPORTS only)
    let valid_uids: BTreeSet<&str> = symbols.iter().map(|s| s.uid.as_str()).collect();
    for rel in relationships {
        if !matches!(
            rel.kind,
            RelationshipKind::Calls | RelationshipKind::Imports
        ) {
            continue;
        }
        if !valid_uids.contains(rel.source_uid.as_str())
            || !valid_uids.contains(rel.target_uid.as_str())
        {
            continue;
        }
        let src = mermaid_id(&rel.source_uid);
        let tgt = mermaid_id(&rel.target_uid);
        let label = rel.kind.to_string();
        out.push_str(&format!("  {} -->|{}| {}\n", src, label, tgt));
    }

    // Suppress unused variable warning
    let _ = uid_to_name;

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("mod::{}", name),
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_class(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Class,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 50,
            signature: format!("class {}", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_method(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Method,
            file_path: "src/lib.rs".to_string(),
            start_line: 5,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_rel(source: &str, target: &str, kind: RelationshipKind) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_export_mermaid_empty() {
        let result = export_mermaid(&[], &[], MermaidDiagramType::Flowchart);
        assert!(result.starts_with("flowchart LR"));

        let result = export_mermaid(&[], &[], MermaidDiagramType::ClassDiagram);
        assert!(result.starts_with("classDiagram"));

        let result = export_mermaid(&[], &[], MermaidDiagramType::Graph);
        assert!(result.starts_with("graph LR"));
    }

    #[test]
    fn test_export_flowchart() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Calls),
        ];

        let result = export_mermaid(&symbols, &rels, MermaidDiagramType::Flowchart);
        assert!(result.contains("flowchart LR"));
        assert!(result.contains("-->"));
        assert!(result.contains("alpha"));
        assert!(result.contains("beta"));
        assert!(result.contains("gamma"));
    }

    #[test]
    fn test_export_flowchart_ignores_non_calls() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        let rels = vec![make_rel("a", "b", RelationshipKind::Imports)];

        let result = export_mermaid(&symbols, &rels, MermaidDiagramType::Flowchart);
        assert!(result.starts_with("flowchart LR"));
        // No arrows because flowchart only uses CALLS
        assert!(!result.contains("-->"));
    }

    #[test]
    fn test_export_class_diagram() {
        let symbols = vec![
            make_class("c1", "UserService"),
            make_method("m1", "authenticate"),
            make_method("m2", "authorize"),
        ];
        let rels = vec![
            make_rel("m1", "c1", RelationshipKind::MemberOf),
            make_rel("m2", "c1", RelationshipKind::MemberOf),
        ];

        let result = export_mermaid(&symbols, &rels, MermaidDiagramType::ClassDiagram);
        assert!(result.contains("classDiagram"));
        assert!(result.contains("class UserService"));
        assert!(result.contains("+authenticate()"));
        assert!(result.contains("+authorize()"));
    }

    #[test]
    fn test_export_graph_with_communities() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        let rels = vec![make_rel("a", "c", RelationshipKind::Calls)];

        let mut communities = HashMap::new();
        communities.insert("a".to_string(), "Auth Module".to_string());
        communities.insert("b".to_string(), "Auth Module".to_string());
        communities.insert("c".to_string(), "Data Layer".to_string());

        let result = export_mermaid_with_communities(&symbols, &rels, &communities);
        assert!(result.contains("graph LR"));
        assert!(result.contains("subgraph Auth Module"));
        assert!(result.contains("subgraph Data Layer"));
        assert!(result.contains("-->"));
    }

    #[test]
    fn test_mermaid_escape() {
        let escaped = mermaid_escape("foo<bar>\"baz\"");
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(!escaped.contains('"'));
    }

    #[test]
    fn test_mermaid_id() {
        assert_eq!(mermaid_id("my-uid-123"), "my_uid_123");
        assert_eq!(mermaid_id("a.b.c"), "a_b_c");
    }
}
