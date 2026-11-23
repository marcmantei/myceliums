//! Golden snapshots of the `tools/list` surface.
//!
//! Agents see the server only through the tool list: each tool's name,
//! description, input/output JSON schema and `_meta`. This test serializes
//! that list (sorted by tool name, object keys sorted recursively) for the
//! default profile (`core`) and for `all`, and compares each with its
//! committed golden file `tests/snapshots/tools_list.<profile>.json`, so any
//! change to the agent-visible surface — intended or not — shows up as a
//! diff.
//!
//! To accept an intended change, regenerate the golden files and commit them:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p myceliums-mcp --test tools_list_snapshot
//! ```

use myceliums_mcp::{MyceliumsMcp, Toolsets};
use rmcp::ServerHandler;
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden_path(profile: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/snapshots/tools_list.{profile}.json"))
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

fn render_tool_list(toolsets: &Toolsets) -> String {
    let mut tools = MyceliumsMcp::tool_list_for_test(toolsets);
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let value = serde_json::to_value(&tools).expect("tools serialize");
    let mut rendered = serde_json::to_string_pretty(&sort_keys(value)).expect("render");
    rendered.push('\n');
    rendered
}

#[test]
fn default_profile_matches_golden_snapshot() {
    check(&Toolsets::default(), "core");
}

#[test]
fn all_profile_matches_golden_snapshot() {
    check(&Toolsets::all(), "all");
}

fn check(toolsets: &Toolsets, profile: &str) {
    let actual = render_tool_list(toolsets);
    let path = golden_path(profile);

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

/// What the default profile adds to every model call (#181): the core tool
/// definitions as Claude Code sends them (name with its `mcp__myceliums__`
/// prefix, description, input schema; compact JSON), measured at 6,697
/// characters, and the server instructions, measured at 1,778. Each budget
/// is that size plus 10 %: a change that grows them past it must cut
/// something else or raise the budget on purpose.
const CORE_DEFINITIONS_BUDGET: usize = 7_366;
const INSTRUCTIONS_BUDGET: usize = 1_955;

#[test]
fn core_definitions_and_instructions_stay_within_budget() {
    let tools = MyceliumsMcp::tool_list_for_test(&Toolsets::default());
    let definitions: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": format!("mcp__myceliums__{}", t.name),
                "description": t.description,
                "input_schema": &*t.input_schema,
            })
        })
        .collect();
    let chars = serde_json::to_string(&definitions)
        .expect("render")
        .chars()
        .count();
    assert!(
        chars <= CORE_DEFINITIONS_BUDGET,
        "core tool definitions are {chars} characters, budget {CORE_DEFINITIONS_BUDGET}"
    );
    let instructions = MyceliumsMcp::new()
        .get_info()
        .instructions
        .expect("server instructions");
    let chars = instructions.chars().count();
    assert!(
        chars <= INSTRUCTIONS_BUDGET,
        "server instructions are {chars} characters, budget {INSTRUCTIONS_BUDGET}"
    );
}
