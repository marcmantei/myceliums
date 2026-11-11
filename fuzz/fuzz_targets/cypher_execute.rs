//! Parse and execute arbitrary input as Cypher against a small fixed graph
//! (the same one the property tests use). Any panic is a bug: every query
//! must produce rows or an error.
#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use myceliums_cypher::CypherExecutor;

#[path = "../../crates/myceliums-cypher/tests/support/small_graph.rs"]
mod small_graph;

fn executor() -> &'static CypherExecutor {
    static EXECUTOR: OnceLock<CypherExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(small_graph::small_graph_executor)
}

fuzz_target!(|data: &[u8]| {
    let Ok(query) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(rows) = executor().execute(query) {
        if let Ok(ast) = myceliums_cypher::parser::parse_cypher(query) {
            if let Some(limit) = ast.limit {
                assert!(rows.len() as i64 <= limit, "LIMIT {limit} returned {} rows", rows.len());
            }
        }
    }
});
