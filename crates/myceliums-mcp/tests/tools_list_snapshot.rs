//! Golden snapshot of the `tools/list` surface.
//!
//! Agents see the server only through the tool list: each tool's name,
//! description, and input/output JSON schema. This test serializes that list
//! (sorted by tool name, object keys sorted recursively) and compares it with
//! the committed golden file `tests/snapshots/tools_list.json`, so any change
//! to the agent-visible surface — intended or not — shows up as a diff.
//!
//! To accept an intended change, regenerate the golden file and commit it:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p myceliums-mcp --test tools_list_snapshot
//! ```

use myceliums_mcp::MyceliumsMcp;
use serde_json::Value;
use std::path::PathBuf;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tools_list.json")
}

/// Rebuilds every JSON object with its keys in sorted order, so the snapshot
/// does not depend on serde_json's map ordering or on schema generation order.
fn sort_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, inner) in entries {
                sorted.insert(key, sort_keys(inner));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

fn render_tool_list() -> String {
    let mut tools = MyceliumsMcp::tool_list_for_test();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let value = serde_json::to_value(&tools).expect("tools serialize");
    let mut rendered = serde_json::to_string_pretty(&sort_keys(value)).expect("render");
    rendered.push('\n');
    rendered
}

#[test]
fn tools_list_matches_golden_snapshot() {
    let actual = render_tool_list();
    let path = golden_path();

    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create snapshot dir");
        std::fs::write(&path, &actual).expect("write golden snapshot");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden snapshot {} ({e}); run with UPDATE_SNAPSHOTS=1 to create it",
            path.display()
        )
    });
    if actual != expected {
        let first_diff = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e)
            .map(|i| i + 1)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()) + 1);
        panic!(
            "tools/list differs from {} (first differing line: {first_diff}). \
             If the change is intended, rerun with UPDATE_SNAPSHOTS=1 and commit the file.",
            path.display()
        );
    }
}
