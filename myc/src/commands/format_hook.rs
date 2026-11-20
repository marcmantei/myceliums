//! `myc format-hook`.

use anyhow::Result;

/// Output a JSON hookSpecificOutput for Claude Code PostToolUse hooks.
fn hook_post_tool_message(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"{}\"}}}}",
        escaped
    );
}

pub(crate) async fn cmd_format_hook() -> Result<()> {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);

    let tool_name = json["tool_name"].as_str().unwrap_or("");
    let tool_input = &json["tool_input"];
    let tool_response = &json["tool_response"];

    let short_name = tool_name
        .strip_prefix("mcp__myceliums__")
        .unwrap_or(tool_name);

    let result_text = hook_response_text(tool_response);
    let msg = format_hook_message(short_name, tool_input, &result_text);
    if !msg.is_empty() {
        hook_post_tool_message(&msg);
    }

    Ok(())
}

/// The text of an MCP tool response as the hook receives it: the first text
/// block. Text tools send plain text (#97); servers before 0.4 wrapped it as
/// `{"text": "..."}` JSON, which is still unwrapped.
fn hook_response_text(tool_response: &serde_json::Value) -> String {
    let block = match tool_response {
        serde_json::Value::String(s) => s.as_str(),
        other => other
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|block| block["text"].as_str())
            .unwrap_or(""),
    };
    match serde_json::from_str::<serde_json::Value>(block) {
        Ok(v) if v["text"].is_string() => v["text"].as_str().unwrap_or_default().to_string(),
        _ => block.to_string(),
    }
}

fn format_hook_message(
    short_name: &str,
    tool_input: &serde_json::Value,
    result_text: &str,
) -> String {
    fn truncate(s: &str, max: usize) -> String {
        myceliums_core::text::snippet(s, max)
    }

    // The first line that says something: past the `Index:` freshness line,
    // the keyword-fallback note of search_code, and untrusted-block markers.
    let first_line = result_text
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with("Index:")
                && !l.starts_with("Keyword search (")
                && !l.starts_with("<<<UNTRUSTED ")
                && !l.starts_with(">>>END ")
        })
        .unwrap_or("");
    // Structured tools (find_callers, ...) send JSON.
    let json: serde_json::Value =
        serde_json::from_str(result_text).unwrap_or(serde_json::Value::Null);

    match short_name {
        "find_callers" | "find_callees" | "get_callers" | "get_callees" => {
            let what = if short_name.ends_with("callers") {
                "callers"
            } else {
                "callees"
            };
            let name = tool_input["symbol_name"].as_str().unwrap_or("?");
            match json["total_count"].as_u64() {
                Some(n) => format!("[myceliums] {n} {what} of {}", truncate(name, 40)),
                None => format!("[myceliums] {what} of {}", truncate(name, 40)),
            }
        }
        "trace_flow" => {
            if first_line.starts_with("Shortest") || first_line.starts_with("No call chain") {
                format!("[myceliums] {}", truncate(first_line, 100))
            } else {
                "[myceliums] Call chain traced".to_string()
            }
        }
        "search_code" | "context_search" | "search_documents" | "semantic_search" => {
            let query = tool_input["query"].as_str().unwrap_or("?");
            // First line is like "Found 15 results for "query":"
            if first_line.starts_with("Found") || first_line.starts_with("No results") {
                format!("[myceliums] {}", first_line)
            } else {
                format!("[myceliums] Search: \"{}\"", truncate(query, 40))
            }
        }
        "hybrid_search" => {
            if first_line.starts_with("Found") || first_line.starts_with("No results") {
                format!("[myceliums] {}", first_line)
            } else {
                let query = tool_input["query"].as_str().unwrap_or("?");
                format!("[myceliums] Hybrid search: \"{}\"", truncate(query, 40))
            }
        }
        "explain_symbol" | "symbol_context" => {
            // First line is like "Symbol: AppInner (Function)"
            if first_line.starts_with("Symbol:") {
                format!("[myceliums] {}", first_line)
            } else {
                let name = tool_input["symbol_name"].as_str().unwrap_or("?");
                format!("[myceliums] Symbol: {}", name)
            }
        }
        "analyze" => {
            // First line is like "Analysis complete (cached):"
            if first_line.starts_with("Analysis") {
                format!("[myceliums] {}", first_line.trim_end_matches(':'))
            } else {
                "[myceliums] Analysis complete".to_string()
            }
        }
        "impact_of_change" | "detect_impact" => {
            if first_line.starts_with("Impact") {
                format!("[myceliums] {}", first_line.trim_end_matches(':'))
            } else {
                "[myceliums] Impact analysis complete".to_string()
            }
        }
        "cypher_query" => {
            let query = tool_input["query"].as_str().unwrap_or("?");
            format!("[myceliums] Cypher: {}", truncate(query, 50))
        }
        "get_processes" => {
            if first_line.starts_with("Found") || first_line.starts_with("No ") {
                format!("[myceliums] {}", first_line)
            } else {
                "[myceliums] Processes loaded".to_string()
            }
        }
        "rename_symbol" => {
            if first_line.starts_with("Rename:") {
                format!("[myceliums] {}", first_line)
            } else {
                let from = tool_input["symbol_name"].as_str().unwrap_or("?");
                let to = tool_input["new_name"].as_str().unwrap_or("?");
                format!("[myceliums] Rename: {} -> {}", from, to)
            }
        }
        _ => format!("[myceliums] {}", short_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_message_truncation_is_char_safe() {
        let input = serde_json::json!({ "query": "ä".repeat(100) });
        let result = "😀".repeat(500);
        // Must not panic on multi-byte content.
        let _ = format_hook_message("context_search", &input, &result);
    }

    #[test]
    fn hook_reads_plain_text_and_skips_the_index_line() {
        let response = serde_json::json!([{
            "type": "text",
            "text": "Index: commit 1a2b3c4 = HEAD, up to date.\n\n<<<UNTRUSTED ab12 search results\nFound 3 results for \"parse\":\n>>>END ab12"
        }]);
        let text = hook_response_text(&response);
        let input = serde_json::json!({ "query": "parse" });
        assert_eq!(
            format_hook_message("search_code", &input, &text),
            "[myceliums] Found 3 results for \"parse\":"
        );
        // Servers before 0.4 wrapped the text in JSON.
        let legacy =
            serde_json::json!([{"type": "text", "text": "{\"text\":\"Symbol: f (Function)\"}"}]);
        assert_eq!(
            format_hook_message("symbol_context", &input, &hook_response_text(&legacy)),
            "[myceliums] Symbol: f (Function)"
        );
        let callers =
            serde_json::json!([{"type": "text", "text": "{\"total_count\":4,\"callers\":[]}"}]);
        let input = serde_json::json!({ "symbol_name": "parse" });
        assert_eq!(
            format_hook_message("find_callers", &input, &hook_response_text(&callers)),
            "[myceliums] 4 callers of parse"
        );
    }
}
