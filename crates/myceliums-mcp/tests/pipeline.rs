//! End-to-end pipeline test: `Analyzer::analyze()` over the real
//! `sample-ts-project` fixture, asserting exact graph content (symbol counts,
//! named symbols, and specific call / membership edges) — not just `is_ok()`.
//!
//! Runs offline and deterministically with embeddings disabled.

mod harness;

use myceliums_core::Analyzer;
use myceliums_storage::{RelationshipKind, Store};
use std::collections::HashMap;

/// Index the fixture into a fresh tempdir and return (symbols, relationships).
///
/// A dedicated store (separate from the shared handler harness) keeps this
/// pipeline assertion self-contained and independent of test ordering.
async fn analyze_fixture() -> (
    Vec<myceliums_storage::CodeSymbol>,
    Vec<myceliums_storage::Relationship>,
    myceliums_core::analyzer::AnalysisResult,
) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let store = Store::open(tmp.path(), "pipeline-test")
        .await
        .expect("open store");
    let analyzer = Analyzer::new(store, harness::fixture_repo_path()).set_skip_embeddings(true);
    let result = analyzer.analyze().await.expect("analyze fixture");
    let symbols = analyzer.store().get_symbols().await.expect("get symbols");
    let relationships = analyzer
        .store()
        .get_relationships()
        .await
        .expect("get relationships");
    // Keep tmp alive until after reads complete.
    drop(tmp);
    (symbols, relationships, result)
}

#[tokio::test]
async fn analyze_produces_exact_entity_counts() {
    let (_symbols, _rels, result) = analyze_fixture().await;
    // Ground truth for tests/fixtures/sample-ts-project (4 .ts files).
    assert_eq!(result.file_count, 4, "expected 4 indexed files");
    assert_eq!(result.symbol_count, 28, "expected 28 indexed symbols");
    assert_eq!(
        result.relationship_count, 50,
        "expected 50 indexed relationships"
    );
}

#[tokio::test]
async fn analyze_extracts_named_symbols_with_kinds() {
    let (symbols, _rels, _result) = analyze_fixture().await;
    let by_name: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.name.as_str(), s)).collect();

    // Classes / interfaces / type aliases from the fixture.
    assert_eq!(
        by_name.get("UserService").map(|s| s.kind.to_string()),
        Some("Class".to_string())
    );
    assert_eq!(
        by_name.get("Database").map(|s| s.kind.to_string()),
        Some("Class".to_string())
    );
    assert_eq!(
        by_name.get("User").map(|s| s.kind.to_string()),
        Some("Interface".to_string())
    );
    assert_eq!(
        by_name.get("Config").map(|s| s.kind.to_string()),
        Some("TypeAlias".to_string())
    );

    // A free function and a method resolve to the expected files.
    assert_eq!(
        by_name.get("formatName").map(|s| s.file_path.as_str()),
        Some("src/utils.ts")
    );
    assert_eq!(
        by_name.get("getUser").map(|s| s.file_path.as_str()),
        Some("src/services/user.ts")
    );
}

#[tokio::test]
async fn analyze_resolves_call_edges() {
    let (symbols, rels, _result) = analyze_fixture().await;
    let uid_to_name: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.name.as_str()))
        .collect();

    let call_edges: Vec<(&str, &str)> = rels
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter_map(|r| {
            Some((
                *uid_to_name.get(r.source_uid.as_str())?,
                *uid_to_name.get(r.target_uid.as_str())?,
            ))
        })
        .collect();

    // Exactly 11 CALLS edges are resolved for this fixture.
    assert_eq!(call_edges.len(), 11, "expected 11 resolved CALLS edges");

    // Specific, meaningful call relationships must be present.
    for expected in [
        ("main", "getUser"),
        ("main", "formatName"),
        ("getUser", "findById"),
        ("createUser", "insert"),
        ("handler", "processRequest"),
        ("processRequest", "parseBody"),
        ("processRequest", "validateInput"),
    ] {
        assert!(
            call_edges.contains(&expected),
            "missing CALLS edge {expected:?}; got {call_edges:?}"
        );
    }
}

#[tokio::test]
async fn analyze_resolves_membership_edges() {
    let (symbols, rels, _result) = analyze_fixture().await;
    let uid_to_name: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.name.as_str()))
        .collect();

    let member_edges: Vec<(&str, &str)> = rels
        .iter()
        .filter(|r| r.kind == RelationshipKind::MemberOf)
        .filter_map(|r| {
            Some((
                *uid_to_name.get(r.source_uid.as_str())?,
                *uid_to_name.get(r.target_uid.as_str())?,
            ))
        })
        .collect();

    // Methods are members of their declaring class.
    for expected in [
        ("getUser", "UserService"),
        ("createUser", "UserService"),
        ("findById", "Database"),
        ("insert", "Database"),
    ] {
        assert!(
            member_edges.contains(&expected),
            "missing MEMBER_OF edge {expected:?}; got {member_edges:?}"
        );
    }
}
