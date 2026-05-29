//! Exports the knowledge graph in GraphML format.
//!
//! Produces a standard GraphML XML document suitable for import into
//! Gephi, yEd, Cytoscape, and other graph-analysis tools.

use myceliums_storage::{CodeSymbol, Relationship};

/// Escape special XML characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Export symbols and relationships as a GraphML XML string.
///
/// Nodes carry the attributes `name`, `kind`, `file`, `line`, and `signature`.
/// Edges carry a `kind` attribute describing the relationship type.
pub fn export_graphml(symbols: &[CodeSymbol], relationships: &[Relationship]) -> String {
    let mut xml = String::new();

    // XML header
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(
        r#"<graphml xmlns="http://graphml.graphstruct.org/graphml"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://graphml.graphstruct.org/graphml http://graphml.graphstruct.org/xmlns/1.0/graphml.xsd">"#,
    );
    xml.push('\n');

    // Key declarations for node attributes
    xml.push_str(r#"  <key id="name" for="node" attr.name="name" attr.type="string"/>"#);
    xml.push('\n');
    xml.push_str(r#"  <key id="kind" for="node" attr.name="kind" attr.type="string"/>"#);
    xml.push('\n');
    xml.push_str(r#"  <key id="file" for="node" attr.name="file" attr.type="string"/>"#);
    xml.push('\n');
    xml.push_str(r#"  <key id="line" for="node" attr.name="line" attr.type="int"/>"#);
    xml.push('\n');
    xml.push_str(r#"  <key id="signature" for="node" attr.name="signature" attr.type="string"/>"#);
    xml.push('\n');

    // Key declaration for edge attributes
    xml.push_str(r#"  <key id="edge_kind" for="edge" attr.name="kind" attr.type="string"/>"#);
    xml.push('\n');

    xml.push_str(r#"  <graph id="G" edgedefault="directed">"#);
    xml.push('\n');

    // Nodes
    for s in symbols {
        xml.push_str(&format!(r#"    <node id="{}">"#, xml_escape(&s.uid)));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="name">{}</data>"#,
            xml_escape(&s.name)
        ));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="kind">{}</data>"#,
            xml_escape(&s.kind.to_string())
        ));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="file">{}</data>"#,
            xml_escape(&s.file_path)
        ));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="line">{}</data>"#,
            s.start_line
        ));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="signature">{}</data>"#,
            xml_escape(&s.signature)
        ));
        xml.push('\n');
        xml.push_str("    </node>\n");
    }

    // Edges
    for (i, r) in relationships.iter().enumerate() {
        xml.push_str(&format!(
            r#"    <edge id="e{}" source="{}" target="{}">"#,
            i,
            xml_escape(&r.source_uid),
            xml_escape(&r.target_uid)
        ));
        xml.push('\n');
        xml.push_str(&format!(
            r#"      <data key="edge_kind">{}</data>"#,
            xml_escape(&r.kind.to_string())
        ));
        xml.push('\n');
        xml.push_str("    </edge>\n");
    }

    xml.push_str("  </graph>\n");
    xml.push_str("</graphml>\n");

    xml
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
    fn test_export_graphml_empty() {
        let xml = export_graphml(&[], &[]);
        assert!(xml.contains("<?xml"));
        assert!(xml.contains("<graphml"));
        assert!(xml.contains("</graphml>"));
    }

    #[test]
    fn test_export_graphml_nodes_and_edges() {
        let symbols = vec![make_symbol("u1", "foo"), make_symbol("u2", "bar")];
        let rels = vec![make_rel("u1", "u2")];
        let xml = export_graphml(&symbols, &rels);

        assert!(xml.contains(r#"<node id="u1">"#));
        assert!(xml.contains(r#"<node id="u2">"#));
        assert!(xml.contains(r#"<data key="name">foo</data>"#));
        assert!(xml.contains(r#"<data key="name">bar</data>"#));
        assert!(xml.contains(r#"<data key="kind">Function</data>"#));
        assert!(xml.contains(r#"<data key="file">src/lib.rs</data>"#));
        assert!(xml.contains(r#"<data key="line">1</data>"#));
        assert!(xml.contains(r#"<data key="signature">fn foo()</data>"#));
        assert!(xml.contains(r#"source="u1" target="u2">"#));
        assert!(xml.contains(r#"<data key="edge_kind">CALLS</data>"#));
    }

    #[test]
    fn test_xml_escape() {
        let symbols = vec![{
            let mut s = make_symbol("u1", "a<b");
            s.signature = "fn a<b>()".to_string();
            s
        }];
        let xml = export_graphml(&symbols, &[]);
        assert!(xml.contains("a&lt;b"));
        assert!(!xml.contains("a<b"));
    }
}
