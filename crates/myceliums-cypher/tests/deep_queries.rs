//! Regression (#91): very long `AND`/`OR` chains must not overflow the stack.
//!
//! The parser used to fold `a OR b OR c …` into a left-deep tree as deep as
//! the chain was long; 200k operands parsed fine and then overflowed while the
//! AST was evaluated or dropped. These tests run parse + execute + drop on a
//! 2 MiB thread — the default stack of tokio worker threads, which is where
//! the MCP server executes queries.

mod support;

use myceliums_cypher::parser::{parse_cypher, ParseError};
use support::small_graph::small_graph_executor;

const OPERANDS: usize = 200_000;
const WORKER_STACK: usize = 2 * 1024 * 1024;

fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("no panic / overflow on a 2 MiB stack")
}

fn long_chain(op: &str) -> String {
    let chain = std::iter::repeat_n("n.name = 'main'", OPERANDS)
        .collect::<Vec<_>>()
        .join(format!(" {op} ").as_str());
    format!("MATCH (n) WHERE {chain} RETURN n.name")
}

fn parse_execute_drop(op: &'static str, expected_rows: usize) {
    on_small_stack(move || {
        let query = long_chain(op);
        let ast = parse_cypher(&query).expect("parses");
        let _ = format!("{ast:?}");
        drop(ast);
        let rows = small_graph_executor().execute(&query).expect("executes");
        assert_eq!(rows.len(), expected_rows);
    });
}

#[test]
fn a_200k_operand_or_chain_parses_executes_and_drops() {
    // Exactly one symbol is named `main`.
    parse_execute_drop("OR", 1);
}

#[test]
fn a_200k_operand_and_chain_parses_executes_and_drops() {
    parse_execute_drop("AND", 1);
}

#[test]
fn deep_nesting_is_rejected_on_a_small_stack() {
    on_small_stack(|| {
        let query = format!("MATCH (n) RETURN {}n.name", "NOT ".repeat(OPERANDS));
        assert!(matches!(
            parse_cypher(&query),
            Err(ParseError::TooDeeplyNested(_))
        ));
        assert!(small_graph_executor().execute(&query).is_err());
    });
}

/// Mixed chains regroup without changing results.
#[test]
fn regrouped_chains_keep_their_meaning() {
    let exec = small_graph_executor();
    let names = |q: &str| {
        let mut v: Vec<String> = exec
            .execute(q)
            .unwrap()
            .iter()
            .map(|r| r["n.name"].as_str().unwrap_or_default().to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names("MATCH (n) WHERE n.name = 'main' OR n.name = 'leaf' OR n.name = 'helper' OR n.name = 'nope' RETURN n.name"),
        vec!["helper", "leaf", "main"]
    );
    assert_eq!(
        names("MATCH (n) WHERE n.kind = 'Function' AND n.file_path = 'src/app.ts' AND n.start_line > 5 RETURN n.name"),
        vec!["helper"]
    );
    assert_eq!(
        names("MATCH (n) WHERE n.name = 'main' OR n.name = 'leaf' AND n.start_line > 100 OR n.name = 'helper' RETURN n.name"),
        vec!["helper", "main"]
    );
}
