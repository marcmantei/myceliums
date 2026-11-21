//! Every Cypher example in `docs/guides/cypher-queries.md` runs (#118): the
//! guide once listed labels (`Function`, `Class`, `Community`) the executor
//! rejects. Each example also returns rows on a graph shaped like an indexed
//! project and named like the guide's examples: one that runs but finds
//! nothing teaches the wrong edge (the guide once looked for a class's
//! methods through `CONTAINED_BY`, which links a symbol to its file).

mod support;

use myceliums_cypher::CypherExecutor;
use myceliums_storage::{CodeSymbol, FileNode, Relationship, RelationshipKind, SymbolKind};
use support::small_graph::small_graph_executor;

/// The bodies of the ```` ```cypher ```` blocks of `markdown`.
fn cypher_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim()) {
            (None, "```cypher") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push(std::mem::take(block));
                current = None;
            }
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

fn guide() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/guides/cypher-queries.md"
    ))
    .expect("the guide exists")
}

#[test]
fn every_example_in_the_cypher_guide_executes() {
    let blocks = cypher_blocks(&guide());
    assert!(blocks.len() >= 10, "found only {} examples", blocks.len());
    let executor = small_graph_executor();
    for query in blocks {
        if let Err(e) = executor.execute(&query) {
            panic!("guide example fails: {e}\n{query}");
        }
    }
}

/// A small project with the edges the analyzer builds: `CONTAINED_BY` from
/// every symbol to its file, `MEMBER_OF` from a method to its class, calls
/// from the API layer into the database layer, a test, and a rationale
/// comment — named like the guide's examples.
fn guide_graph() -> CypherExecutor {
    use RelationshipKind::*;
    use SymbolKind::*;

    let symbols = [
        ("authenticate", Function, "src/auth.rs", 10),
        ("handleRequest", Function, "src/api/handlers.rs", 5),
        ("saveUser", Function, "src/database/users.rs", 3),
        ("Router", Class, "src/router.rs", 1),
        ("route", Method, "src/router.rs", 4),
        ("dispatch", Method, "src/router.rs", 12),
        ("test_authenticate", Function, "tests/auth_test.rs", 1),
        ("WHY: tokens expire", Rationale, "src/auth.rs", 8),
    ]
    .map(|(name, kind, file, line)| CodeSymbol {
        uid: name.to_string(),
        name: name.to_string(),
        qualified_name: name.to_string(),
        kind,
        file_path: file.to_string(),
        start_line: line,
        end_line: line + 3,
        signature: String::new(),
        content: String::new(),
        repo_id: "guide".to_string(),
        metadata: None,
    })
    .to_vec();
    let mut files: Vec<FileNode> = Vec::new();
    for s in &symbols {
        match files.iter_mut().find(|f| f.path == s.file_path) {
            Some(f) => f.num_symbols += 1,
            None => files.push(FileNode {
                uid: format!("file:{}", s.file_path),
                path: s.file_path.clone(),
                language: "rust".to_string(),
                repo_id: "guide".to_string(),
                num_symbols: 1,
            }),
        }
    }
    let edge = |from: &str, to: &str, kind: RelationshipKind| Relationship {
        uid: format!("{from}-{to}"),
        source_uid: from.to_string(),
        target_uid: to.to_string(),
        kind,
        repo_id: "guide".to_string(),
        metadata: String::new(),
    };
    let mut relationships: Vec<Relationship> = symbols
        .iter()
        .map(|s| edge(&s.uid, &format!("file:{}", s.file_path), ContainedBy))
        .collect();
    relationships.extend([
        edge("handleRequest", "authenticate", Calls),
        edge("handleRequest", "saveUser", Calls),
        edge("test_authenticate", "authenticate", Calls),
        edge("route", "Router", MemberOf),
        edge("dispatch", "Router", MemberOf),
        edge("dispatch", "handleRequest", Calls),
        edge("WHY: tokens expire", "authenticate", RationaleFor),
    ]);
    CypherExecutor::from_parts(symbols, files, relationships)
}

#[test]
fn every_example_in_the_cypher_guide_returns_rows() {
    let executor = guide_graph();
    for query in cypher_blocks(&guide()) {
        let rows = executor
            .execute(&query)
            .unwrap_or_else(|e| panic!("guide example fails: {e}\n{query}"));
        assert!(
            !rows.is_empty(),
            "guide example returns no rows on a project it describes:\n{query}"
        );
    }
}

/// The query the guide once gave for a class's methods (through
/// `CONTAINED_BY`, a symbol-to-file edge) finds none of them: the rows
/// check above fails on it.
#[test]
fn the_old_class_methods_example_finds_nothing() {
    let executor = guide_graph();
    let old = "MATCH (m)-[:CONTAINED_BY]->(c) WHERE c.name = \"Router\" AND c.kind = \"Class\" RETURN m.name";
    assert!(executor.execute(old).unwrap().is_empty());
}
