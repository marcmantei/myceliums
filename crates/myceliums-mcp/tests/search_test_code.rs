//! `search_code` ranks test code below product code by default, and its
//! `tests` parameter includes or prefers it (#146), checked over a real MCP
//! session on a small project with one product function and its tests.

use myceliums_core::Analyzer;
use myceliums_mcp::{MyceliumsMcp, Toolsets};
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::OnceCell;

type Client = RunningService<RoleClient, ()>;

const PRODUCT: &str = "src/cookies.py";

const COOKIES: &str = r#"
def parse_cookie_header(header):
    """Split a Cookie header into name/value pairs."""
    pairs = [part.strip().split("=", 1) for part in header.split(";") if part]
    return {name: value for name, value in pairs}
"#;

const COOKIE_TESTS: &str = r#"
from cookies import parse_cookie_header


def test_parse_cookie_header_single():
    assert parse_cookie_header("a=1") == {"a": "1"}


def test_parse_cookie_header_many():
    assert parse_cookie_header("a=1; b=2") == {"a": "1", "b": "2"}


def test_parse_cookie_header_empty():
    assert parse_cookie_header("") == {}


def test_parse_cookie_header_spaces():
    assert parse_cookie_header(" a=1 ;b=2") == {"a": "1", "b": "2"}
"#;

/// The project and data directory, built once per test binary: `ranking`
/// is registered as analyzed by this build, `legacy` (the same sources) as
/// an index from before test code was classified.
static SETUP: OnceCell<(TempDir, TempDir)> = OnceCell::const_new();

async fn setup() {
    SETUP
        .get_or_init(|| async {
            // Not a dot-directory: discovery skips hidden directories.
            let project = tempfile::Builder::new()
                .prefix("cookies")
                .tempdir()
                .unwrap();
            std::fs::create_dir_all(project.path().join("src")).unwrap();
            std::fs::create_dir_all(project.path().join("tests")).unwrap();
            std::fs::write(project.path().join(PRODUCT), COOKIES).unwrap();
            std::fs::write(project.path().join("tests/test_cookies.py"), COOKIE_TESTS).unwrap();

            let data = tempfile::tempdir().unwrap();
            std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
            let mut registry = RepoRegistry::load(&data.path().join("repos.json")).unwrap();
            for (id, classification_version) in [
                (
                    "ranking",
                    myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
                ),
                ("legacy", 0),
            ] {
                let db = RepoRegistry::repo_db_path(data.path(), id);
                let store = Store::open(&db, id).await.unwrap();
                let result = Analyzer::new(store, project.path().to_path_buf())
                    .set_skip_embeddings(true)
                    .analyze()
                    .await
                    .unwrap();
                registry.register(RepoInfo {
                    id: id.to_string(),
                    name: id.to_string(),
                    path: project.path().to_string_lossy().to_string(),
                    analyzed_at: "2026-01-01T00:00:00Z".to_string(),
                    symbol_count: result.symbol_count as u32,
                    file_count: result.file_count as u32,
                    analyzed_commit: None,
                    vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
                    classification_version,
                });
            }
            registry.save().unwrap();
            (project, data)
        })
        .await;
}

async fn connect() -> Client {
    setup().await;
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&Toolsets::default())
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn search(client: &Client, args: Value) -> String {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new("search_code".to_string()).with_arguments(args))
        .await
        .expect("no protocol error");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert_ne!(result.is_error, Some(true), "{text}");
    text
}

/// The files of the result rows, best first.
fn files(text: &str) -> Vec<&'static str> {
    text.lines()
        .filter(|line| line.contains(".py"))
        .map(|line| {
            if line.contains(PRODUCT) {
                "product"
            } else {
                assert!(line.contains("tests/"), "unexpected row: {line}");
                "test"
            }
        })
        .collect()
}

fn product_rank(text: &str) -> usize {
    files(text)
        .iter()
        .position(|f| *f == "product")
        .expect("the product function is found")
}

#[tokio::test]
async fn test_code_is_demoted_by_default_and_included_or_preferred_on_request() {
    let client = connect().await;
    let query = |tests: Option<&str>| {
        let mut args = json!({
            "query": "parse_cookie_header",
            "mode": "keyword",
            "repo_id": "ranking",
        });
        if let Some(tests) = tests {
            args["tests"] = json!(tests);
        }
        args
    };

    let default = search(&client, query(None)).await;
    let demoted = search(&client, query(Some("demote"))).await;
    let included = search(&client, query(Some("include"))).await;
    let preferred = search(&client, query(Some("prefer"))).await;

    // Nothing is filtered: every mode returns the same symbols.
    let count = files(&included).len();
    assert!(count >= 5, "{included}");
    for text in [&default, &demoted, &preferred] {
        assert_eq!(files(text).len(), count, "{text}");
    }
    assert_eq!(files(&default), files(&demoted));

    // Demotion moves the product function up past the tests in front of it
    // (with one product function among tests, past at least the one
    // directly before it); preferring tests puts one first.
    let included_rank = product_rank(&included);
    if included_rank > 0 {
        assert!(
            product_rank(&demoted) < included_rank,
            "{demoted}\n{included}"
        );
    } else {
        assert_eq!(product_rank(&demoted), 0, "{demoted}");
    }
    assert_eq!(files(&preferred)[0], "test", "{preferred}");
    assert!(product_rank(&preferred) >= included_rank, "{preferred}");

    // An index from this build carries the classification: no notice.
    assert!(
        !default.contains("before test code was classified"),
        "{default}"
    );
}

#[tokio::test]
async fn an_unclassified_index_says_tests_rank_like_product_code() {
    let client = connect().await;
    let demoted = search(
        &client,
        json!({"query": "parse_cookie_header", "mode": "keyword", "repo_id": "legacy"}),
    )
    .await;
    assert!(
        demoted.contains("before test code was classified"),
        "{demoted}"
    );
    let included = search(
        &client,
        json!({
            "query": "parse_cookie_header",
            "mode": "keyword",
            "repo_id": "legacy",
            "tests": "include",
        }),
    )
    .await;
    assert!(
        !included.contains("before test code was classified"),
        "{included}"
    );
}

/// The symbol names of the result rows, best first.
fn names(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains(".py"))
        .filter_map(|line| line.split_whitespace().nth(1).map(str::to_string))
        .collect()
}

/// Pages of a demoted ranking partition it: every symbol once, in order.
#[tokio::test]
async fn pages_partition_the_demoted_ranking() {
    let client = connect().await;
    let page = |limit: usize, offset: usize| {
        json!({
            "query": "parse_cookie_header",
            "mode": "keyword",
            "repo_id": "ranking",
            "limit": limit,
            "offset": offset,
        })
    };
    let all = names(&search(&client, page(20, 0)).await);
    assert!(all.len() >= 5, "{all:?}");
    let mut paged = Vec::new();
    for offset in (0..all.len()).step_by(2) {
        paged.extend(names(&search(&client, page(2, offset)).await));
    }
    assert_eq!(paged, all);
}

/// With `explain`, the rows test code was moved down from are named.
#[tokio::test]
async fn explain_names_the_demoted_rows() {
    let client = connect().await;
    let args = |tests: &str| {
        json!({
            "query": "parse_cookie_header",
            "mode": "keyword",
            "repo_id": "ranking",
            "explain": true,
            "tests": tests,
        })
    };
    let demoted = search(&client, args("demote")).await;
    assert!(demoted.contains("Demoted as test code"), "{demoted}");
    let included = search(&client, args("include")).await;
    assert!(!included.contains("Demoted as"), "{included}");
}
