//! Language-agnostic helpers shared by every extractor: node text,
//! signatures, rationale comments, and child lookup.

use super::{ParsedRationale, RationalePrefix, SourceParser};
use tree_sitter::Node;

impl SourceParser {
    pub(super) fn node_text<'a>(&self, node: Node<'a>, source: &'a [u8]) -> &'a str {
        node.utf8_text(source).unwrap_or("")
    }

    pub(super) fn extract_signature(&self, node: Node, source: &[u8]) -> String {
        let start = node.start_byte();
        let text = &source[start..];
        if let Some(pos) = text.iter().position(|&b| b == b'{' || b == b':') {
            let sig = std::str::from_utf8(&text[..pos]).unwrap_or("");
            sig.trim().to_string()
        } else {
            let first_line = self.node_text(node, source);
            first_line.lines().next().unwrap_or("").trim().to_string()
        }
    }

    /// Known rationale prefixes to scan for in comments.
    const RATIONALE_PREFIXES: &[(&'static str, RationalePrefix)] = &[
        ("NOTE:", RationalePrefix::Note),
        ("HACK:", RationalePrefix::Hack),
        ("WHY:", RationalePrefix::Why),
        ("TODO:", RationalePrefix::Todo),
        ("FIXME:", RationalePrefix::Fixme),
        ("IMPORTANT:", RationalePrefix::Important),
    ];

    /// Walk the AST and extract rationale comments from all comment nodes.
    /// This is language-agnostic: tree-sitter marks comment nodes with kinds
    /// like "comment", "line_comment", "block_comment", etc.
    pub(super) fn extract_rationales(node: Node, source: &[u8]) -> Vec<ParsedRationale> {
        let mut rationales = Vec::new();
        Self::walk_for_rationales(node, source, &mut rationales);
        rationales
    }

    fn walk_for_rationales(node: Node, source: &[u8], out: &mut Vec<ParsedRationale>) {
        let kind = node.kind();
        if kind.contains("comment") {
            let text = node.utf8_text(source).unwrap_or("");
            // A comment node may span multiple lines; check each line.
            for (offset, line) in text.lines().enumerate() {
                // Strip comment delimiters: //, #, /*, *, --, etc.
                let stripped = Self::strip_comment_delimiters(line);
                let trimmed = stripped.trim();
                for (prefix_str, prefix_kind) in Self::RATIONALE_PREFIXES {
                    if let Some(rest) = trimmed.strip_prefix(prefix_str) {
                        out.push(ParsedRationale {
                            prefix: prefix_kind.clone(),
                            text: rest.trim().to_string(),
                            line: node.start_position().row as u32 + offset as u32 + 1,
                        });
                        break; // only match first prefix per line
                    }
                }
            }
        }

        let child_count = node.child_count();
        for i in 0..child_count {
            if let Some(child) = node.child(i) {
                Self::walk_for_rationales(child, source, out);
            }
        }
    }

    /// Strip common comment delimiters to get to the annotation content.
    fn strip_comment_delimiters(line: &str) -> &str {
        let s = line.trim_start();
        // Order matters: try longer prefixes first
        if let Some(rest) = s.strip_prefix("///") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("//") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("/**") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("/*") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("*/") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix('*') {
            return rest;
        }
        if let Some(rest) = s.strip_prefix('#') {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("--") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("{-") {
            return rest;
        }
        if let Some(rest) = s.strip_prefix("-}") {
            return rest;
        }
        s
    }

    /// Helper to find the first child with a given kind
    pub(super) fn find_child_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let result = node.children(&mut cursor).find(|c| c.kind() == kind);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{RationalePrefix, SourceLanguage, SourceParser};

    #[test]
    fn test_extract_rationale_from_typescript() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
// NOTE: This handles edge case with empty arrays
// HACK: Workaround for upstream bug #123
function process(items: string[]) { return items; }

// TODO: Refactor this to use generics
function transform(x: any) { return x; }
"#,
            )
            .unwrap();
        assert_eq!(result.rationales.len(), 3);
        assert_eq!(result.rationales[0].prefix, RationalePrefix::Note);
        assert!(result.rationales[0].text.contains("handles edge case"));
        assert_eq!(result.rationales[1].prefix, RationalePrefix::Hack);
        assert_eq!(result.rationales[2].prefix, RationalePrefix::Todo);
    }

    #[test]
    fn test_extract_rationale_from_rust() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
// WHY: We use a BTreeMap here for deterministic ordering
// IMPORTANT: This must be called before any I/O
fn setup() {}

// FIXME: Handle the error case properly
fn risky() {}
"#,
            )
            .unwrap();
        assert_eq!(result.rationales.len(), 3);
        assert_eq!(result.rationales[0].prefix, RationalePrefix::Why);
        assert_eq!(result.rationales[1].prefix, RationalePrefix::Important);
        assert_eq!(result.rationales[2].prefix, RationalePrefix::Fixme);
    }

    #[test]
    fn test_extract_rationale_from_python() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
# NOTE: Python uses hash comments
# TODO: Add type hints
def greet(name):
    return f"Hello {name}"
"#,
            )
            .unwrap();
        assert_eq!(result.rationales.len(), 2);
        assert_eq!(result.rationales[0].prefix, RationalePrefix::Note);
        assert_eq!(result.rationales[1].prefix, RationalePrefix::Todo);
    }

    #[test]
    fn test_no_rationale_from_regular_comments() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
// This is just a regular comment
// Another comment without a prefix
function foo() {}
"#,
            )
            .unwrap();
        assert_eq!(result.rationales.len(), 0);
    }
}
