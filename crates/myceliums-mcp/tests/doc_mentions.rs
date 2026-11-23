//! `impact_of_change` and `find_callers` name the docs that mention a
//! symbol (#180), over a real MCP session: a Java project whose AsciiDoc
//! and Markdown pages and Javadoc comments name its classes and methods.
//! The section lists a few docs with file and line, says how many there
//! are, and names the call that lists them all; that call does.
//! `trace_flow` is unchanged.
//!
//! This binary owns its data directory, so it may analyze.

use myceliums_mcp::{AnalyzeParams, MyceliumsMcp, Toolsets};
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};

type Client = RunningService<RoleClient, ()>;

const STORE: &str = r#"package org.acme;

/** Reads through {@link Cache#get(String)}. */
public class Store {
    public String open(Cache cache) {
        return cache.get("k");
    }

    public void evict() {
    }
}
"#;

const CACHE: &str = r#"package org.acme;

public class Cache {
    public String get(String key) {
        return key;
    }

    public void evict() {
    }
}
"#;

const SERVICE: &str = r#"package org.acme;

public class Service {
    /** Opens a {@link Store}; see {@link Store#open(Cache)}. */
    public String run(Store store, Cache cache) {
        return store.open(cache);
    }
}
"#;

const GUIDE: &str = "= Guide\n\n== Opening\n\nCall `Store#open` once.\n\n== Caching\n\nA `Store` reads a `Cache`.\n\n== Eviction\n\nThen `evict()` it.\n";

const README: &str = "# Readme\n\n## Usage\n\nUse `Store.open()`.\n\n## Notes\n\n`Store#open(Cache)` again.\n\n## More\n\n`org.acme.Store#open`.\n";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

async fn connect() -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&Toolsets::all())
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, tool: &str, args: Value) -> String {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .expect("no protocol error");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert_ne!(result.is_error, Some(true), "{tool}: {text}");
    text
}

/// The text from the `Docs mentioning` line on.
fn docs(text: &str) -> &str {
    let at = text
        .find("\nDocs mentioning ")
        .unwrap_or_else(|| panic!("no docs section: {text}"));
    &text[at + 1..]
}

#[tokio::test]
async fn impact_and_callers_name_the_docs_that_mention_a_symbol() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let projects = tempfile::tempdir().unwrap();
    let project = projects.path().join("acme");
    let src = project.join("src/main/java/org/acme");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(src.join("Store.java"), STORE).unwrap();
    std::fs::write(src.join("Cache.java"), CACHE).unwrap();
    std::fs::write(src.join("Service.java"), SERVICE).unwrap();
    std::fs::write(project.join("docs/guide.adoc"), GUIDE).unwrap();
    std::fs::write(project.join("README.md"), README).unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "init"]);
    let project = project.canonicalize().unwrap();
    let repo = myceliums_core::analyzer::repo_id_from_path(&project);
    MyceliumsMcp::new()
        .analyze_for_test(AnalyzeParams {
            path: project.to_string_lossy().into_owned(),
            skip_embeddings: Some(true),
            ..Default::default()
        })
        .await
        .expect("analyze");
    let client = connect().await;

    // ── impact_of_change on a method: the docs come last ────────────────
    let impact = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": "Store.open"}),
    )
    .await;
    let section = docs(&impact);
    assert!(
        section.starts_with("Docs mentioning Store.open (5: 4 in docs, 1 in doc comments):\n"),
        "{impact}"
    );
    for line in [
        "README.md\n  :5 section \"Usage\"\n  :9 section \"Notes\"\n  :13 section \"More\"\n",
        "docs/guide.adoc\n  :5 section \"Opening\"\n",
        "src/main/java/org/acme/Service.java\n  :4 doc comment of Service.run\n",
    ] {
        assert!(section.contains(line), "{line:?} missing from {impact}");
    }
    // Five fit: no call for the rest. `evict()` names two methods and is
    // not linked, but it is not `open`.
    assert!(!section.contains("Next (docs)"), "{impact}");
    assert!(!section.contains("Not linked"), "{impact}");

    // ── impact_of_change on the class: its members' docs too ────────────
    let class = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": "Store"}),
    )
    .await;
    let section = docs(&class);
    assert!(
        section.starts_with("Docs mentioning Store (7: 5 in docs, 2 in doc comments):\n"),
        "{class}"
    );
    assert!(
        section.contains("  :5 section \"Opening\" [Store.open]\n"),
        "{class}"
    );
    let next = section
        .lines()
        .find_map(|l| l.strip_prefix("Next (docs): "))
        .unwrap_or_else(|| panic!("no Next (docs): {class}"));
    assert!(
        next.starts_with(&format!(
            "query_knowledge(repo_id={repo:?}, query=\"src/main/java/org/acme/Store.java:Store\") lists every mention"
        )),
        "{next}"
    );
    // The walk's own `Next:` line, if any, is not the docs one.
    assert!(
        class
            .lines()
            .all(|l| !l.starts_with("Next: query_knowledge")),
        "{class}"
    );

    // ── the call the hint names lists every one ─────────────────────────
    let all = call(
        &client,
        "query_knowledge",
        json!({"repo_id": repo, "query": "src/main/java/org/acme/Store.java:Store", "limit": 50}),
    )
    .await;
    for place in [
        "README.md",
        "docs/guide.adoc",
        "src/main/java/org/acme/Service.java",
    ] {
        assert!(all.contains(place), "{place} missing from {all}");
    }

    // ── impact_of_change on a file ───────────────────────────────────────
    let file = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "file_path": "src/main/java/org/acme/Cache.java"}),
    )
    .await;
    let section = docs(&file);
    assert!(
        section.starts_with("Docs mentioning src/main/java/org/acme/Cache.java (2: 1 in docs, 1 in doc comments):\n"),
        "{file}"
    );
    assert!(
        section.contains("  :9 section \"Caching\" [Cache]\n"),
        "{file}"
    );
    assert!(
        section.contains("  :3 doc comment of Store [Cache.get]\n"),
        "{file}"
    );

    // ── find_callers: the same section after the callers ────────────────
    let callers = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "Cache.get"}),
    )
    .await;
    assert!(callers.contains("1 callers of get:\n"), "{callers}");
    let section = docs(&callers);
    assert!(
        section.starts_with("Docs mentioning Cache.get (1: 0 in docs, 1 in doc comments):\n"),
        "{callers}"
    );
    // A name several methods carry is reported as not linked.
    let evict = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "Cache.evict"}),
    )
    .await;
    assert!(!evict.contains("Docs mentioning"), "{evict}");
    assert!(
        evict.ends_with(
            "\n\nNot linked: 1 doc mention(s) of `evict` that several symbols could be, e.g. docs/guide.adoc:13\n"
        ),
        "{evict}"
    );

    // ── trace_flow is unchanged ─────────────────────────────────────────
    let flow = call(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": "Service.run", "to_symbol": "Cache.get"}),
    )
    .await;
    assert!(!flow.contains("Docs mentioning"), "{flow}");
}
