//! A small, fixed in-memory code graph shared by the Cypher property tests
//! and the `cypher_execute` fuzz target (`fuzz/fuzz_targets`), so both
//! exercise the executor against the same shapes: several symbol kinds,
//! files, a call cycle, a self-loop, non-call edges, and `metadata` values of
//! mixed JSON types (the #91 `ORDER BY` regression).

use myceliums_cypher::CypherExecutor;
use myceliums_storage::{CodeSymbol, FileNode, Relationship, RelationshipKind, SymbolKind};

fn symbol(
    uid: &str,
    name: &str,
    kind: SymbolKind,
    file: &str,
    line: u32,
    metadata: Option<&str>,
) -> CodeSymbol {
    CodeSymbol {
        uid: uid.to_string(),
        name: name.to_string(),
        qualified_name: format!("{file}::{name}"),
        kind,
        file_path: file.to_string(),
        start_line: line,
        end_line: line + 4,
        signature: format!("fn {name}()"),
        content: String::new(),
        repo_id: "small-graph".to_string(),
        metadata: metadata.map(str::to_string),
    }
}

fn edge(uid: &str, from: &str, to: &str, kind: RelationshipKind) -> Relationship {
    Relationship {
        uid: uid.to_string(),
        source_uid: from.to_string(),
        target_uid: to.to_string(),
        kind,
        repo_id: "small-graph".to_string(),
        metadata: "{}".to_string(),
    }
}

/// Builds an executor over the small graph.
pub fn small_graph_executor() -> CypherExecutor {
    use RelationshipKind::*;
    use SymbolKind::*;

    let symbols = vec![
        symbol("main", "main", Function, "src/app.ts", 1, None),
        symbol(
            "helper",
            "helper",
            Function,
            "src/app.ts",
            20,
            Some("\"x\""),
        ),
        symbol("leaf", "leaf", Function, "src/util.ts", 3, Some("7")),
        symbol(
            "widget",
            "Widget",
            Class,
            "src/widget.ts",
            1,
            Some("{\"decorators\":[\"a\"]}"),
        ),
        symbol("render", "render", Method, "src/widget.ts", 5, Some("true")),
        symbol(
            "shape",
            "Shape",
            Interface,
            "src/widget.ts",
            30,
            Some("[1,\"b\"]"),
        ),
        symbol("cfg", "CONFIG", Variable, "src/util.ts", 40, Some("null")),
        symbol(
            "loop_a",
            "loop_a",
            Function,
            "src/loop.ts",
            1,
            Some("not json"),
        ),
        symbol("loop_b", "loop_b", Function, "src/loop.ts", 9, Some("-3.5")),
        symbol("dup1", "dup", Function, "src/dup.ts", 1, None),
        symbol("dup2", "dup", Function, "src/dup.ts", 10, Some("\"\"")),
        symbol("unicode", "größe_日本", Function, "src/ü.ts", 1, None),
    ];
    let files = vec![
        FileNode {
            uid: "f_app".into(),
            path: "src/app.ts".into(),
            language: "typescript".into(),
            repo_id: "small-graph".into(),
            num_symbols: 2,
        },
        FileNode {
            uid: "f_widget".into(),
            path: "src/widget.ts".into(),
            language: "typescript".into(),
            repo_id: "small-graph".into(),
            num_symbols: 3,
        },
    ];
    let relationships = vec![
        edge("r1", "main", "helper", Calls),
        edge("r2", "helper", "leaf", Calls),
        edge("r3", "main", "widget", Calls),
        edge("r4", "render", "widget", MemberOf),
        edge("r5", "render", "leaf", Calls),
        edge("r6", "loop_a", "loop_b", Calls),
        edge("r7", "loop_b", "loop_a", Calls),
        edge("r8", "loop_a", "loop_a", Calls),
        edge("r9", "main", "cfg", Imports),
        edge("r10", "widget", "shape", References),
        edge("r11", "dup1", "dup2", Calls),
        edge("r12", "helper", "missing_uid", Calls),
        edge("r13", "main", "dup1", Calls),
        edge("r14", "leaf", "main", Calls),
    ];
    CypherExecutor::from_parts(symbols, files, relationships)
}
