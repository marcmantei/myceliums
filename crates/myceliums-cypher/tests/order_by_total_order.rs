//! Regression (#91): `ORDER BY` over a column whose values mix JSON types.
//!
//! `compare_values` used to call every cross-type pair (and anything vs
//! `null`) "equal" while still ordering strings among themselves. That is not
//! a total order (`"a" == null == "b"` but `"a" < "b"`), and since Rust 1.81
//! `slice::sort_by` may panic on such comparators. The executor must sort any
//! mix of types without panicking and deterministically.

use myceliums_cypher::CypherExecutor;
use myceliums_storage::{CodeSymbol, SymbolKind};
use serde_json::Value;

fn symbol(i: usize, metadata: Option<String>) -> CodeSymbol {
    CodeSymbol {
        uid: format!("u{i}"),
        name: format!("s{i}"),
        qualified_name: format!("src/a.rs::s{i}"),
        kind: SymbolKind::Function,
        file_path: "src/a.rs".to_string(),
        start_line: i as u32,
        end_line: i as u32 + 1,
        signature: String::new(),
        content: String::new(),
        repo_id: "order-test".to_string(),
        metadata,
    }
}

/// Symbols whose `metadata` column is a string, a number, a bool, an object,
/// or missing (null), in pseudo-random order (fixed LCG seed, deterministic).
/// `kinds` picks how many of those types appear (first three: string, number,
/// null).
fn mixed_type_executor(seed: u64, n: usize, kinds: u32) -> CypherExecutor {
    let mut state = seed;
    let symbols = (0..n)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = (state >> 33) as u32;
            let metadata = match (r / 7) % kinds {
                0 => Some(format!("\"str{}\"", r % 1000)),
                1 => Some(format!("{}", r % 997)),
                2 => None,
                3 => Some("true".to_string()),
                _ => Some(format!("{{\"k\": {}}}", r % 13)),
            };
            symbol(i, metadata)
        })
        .collect();
    CypherExecutor::from_parts(symbols, vec![], vec![])
}

fn rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

#[test]
fn order_by_mixed_types_never_panics() {
    // The standard library's violation check fires most readily on mid-sized
    // inputs, so sweep many seeds at a size where the old comparator panicked.
    for seed in 0..200 {
        for query in [
            "MATCH (s) RETURN s.metadata ORDER BY s.metadata",
            "MATCH (s) RETURN s.metadata ORDER BY s.metadata DESC",
        ] {
            let exec = mixed_type_executor(seed, 50, 3);
            assert_eq!(exec.execute(query).expect("query runs").len(), 50);
        }
    }
}

#[test]
fn order_by_mixed_types_sort_groups_by_type() {
    for seed in 0..40 {
        let exec = mixed_type_executor(seed, 300, 5);
        let rows = exec
            .execute("MATCH (s) RETURN s.metadata ORDER BY s.metadata")
            .expect("query runs");
        assert_eq!(rows.len(), 300);
        assert_sorted(&rows);
    }
}

fn assert_sorted(rows: &[std::collections::HashMap<String, Value>]) {
    let values: Vec<&Value> = rows.iter().map(|r| &r["s.metadata"]).collect();
    // Types are grouped: null < bool < number < string < array < object.
    let ranks: Vec<u8> = values.iter().map(|v| rank(v)).collect();
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
    // Within one type the natural order holds.
    for w in values.windows(2) {
        match (w[0], w[1]) {
            (Value::String(a), Value::String(b)) => assert!(a <= b),
            (Value::Number(a), Value::Number(b)) => {
                assert!(a.as_f64().unwrap() <= b.as_f64().unwrap())
            }
            _ => {}
        }
    }
}

#[test]
fn order_by_mixed_types_desc_is_the_reverse() {
    let exec = mixed_type_executor(7, 300, 5);
    let asc = exec
        .execute("MATCH (s) RETURN s.metadata ORDER BY s.metadata ASC")
        .expect("asc");
    let desc = exec
        .execute("MATCH (s) RETURN s.metadata ORDER BY s.metadata DESC")
        .expect("desc");
    let ranks = |rows: &[std::collections::HashMap<String, Value>]| -> Vec<u8> {
        rows.iter().map(|r| rank(&r["s.metadata"])).collect()
    };
    let mut reversed = ranks(&asc);
    reversed.reverse();
    assert_eq!(ranks(&desc), reversed);
}
