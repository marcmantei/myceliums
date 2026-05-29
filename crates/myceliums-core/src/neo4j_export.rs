//! Exports the knowledge graph as Neo4j Cypher statements.
//!
//! Generates `CREATE` statements suitable for importing into Neo4j via
//! `cypher-shell` or the Neo4j Browser.

use myceliums_storage::{CodeSymbol, Relationship};

/// Escape single quotes for Cypher string literals.
fn cypher_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Export symbols and relationships as a Neo4j Cypher script.
///
/// The output includes:
/// 1. An index on `Symbol.uid` for fast MATCH lookups.
/// 2. `CREATE` statements for each symbol node.
/// 3. `MATCH`/`CREATE` statements for each relationship edge.
pub fn export_neo4j_cypher(symbols: &[CodeSymbol], relationships: &[Relationship]) -> String {
    let mut out = String::new();

    // Index creation
    out.push_str("CREATE INDEX symbol_uid IF NOT EXISTS FOR (n:Symbol) ON (n.uid);\n\n");

    // Nodes
    for s in symbols {
        out.push_str(&format!(
            "CREATE (n:Symbol {{uid: '{}', name: '{}', kind: '{}', file: '{}', line: {}}})\n",
            cypher_escape(&s.uid),
            cypher_escape(&s.name),
            s.kind,
            cypher_escape(&s.file_path),
            s.start_line,
        ));
    }

    if !symbols.is_empty() && !relationships.is_empty() {
        out.push('\n');
    }

    // Edges
    for r in relationships {
        out.push_str(&format!(
            "MATCH (a:Symbol {{uid: '{}'}}), (b:Symbol {{uid: '{}'}})\nCREATE (a)-[:{}]->(b)\n",
            cypher_escape(&r.source_uid),
            cypher_escape(&r.target_uid),
            r.kind,
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{RelationshipKind, SymbolKind};

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

    fn make_rel(source: &str, target: &str) -> Relationship {
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
    fn test_export_cypher_empty() {
        let cypher = export_neo4j_cypher(&[], &[]);
        assert!(cypher.contains("CREATE INDEX"));
        assert!(!cypher.contains("CREATE (n:Symbol"));
    }

    #[test]
    fn test_export_cypher_nodes_and_edges() {
        let symbols = vec![make_symbol("u1", "foo"), make_symbol("u2", "bar")];
        let rels = vec![make_rel("u1", "u2")];
        let cypher = export_neo4j_cypher(&symbols, &rels);

        assert!(cypher.contains("CREATE INDEX symbol_uid IF NOT EXISTS"));
        assert!(cypher.contains("CREATE (n:Symbol {uid: 'u1', name: 'foo'"));
        assert!(cypher.contains("CREATE (n:Symbol {uid: 'u2', name: 'bar'"));
        assert!(cypher.contains("kind: 'Function'"));
        assert!(cypher.contains("file: 'src/lib.rs'"));
        assert!(cypher.contains("line: 1"));
        assert!(cypher.contains("MATCH (a:Symbol {uid: 'u1'}), (b:Symbol {uid: 'u2'})"));
        assert!(cypher.contains("CREATE (a)-[:CALLS]->(b)"));
    }

    #[test]
    fn test_cypher_escape() {
        let symbols = vec![{
            let mut s = make_symbol("u1", "it's");
            s.file_path = "path\\to\\file".to_string();
            s
        }];
        let cypher = export_neo4j_cypher(&symbols, &[]);
        assert!(cypher.contains("it\\'s"));
        assert!(cypher.contains("path\\\\to\\\\file"));
    }
}
