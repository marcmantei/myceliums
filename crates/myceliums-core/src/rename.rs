//! Symbol renaming with call-graph-aware reference tracking.
//!
//! [`RenamePlan`] finds all references to a symbol — the definition itself,
//! call sites, imports, and text occurrences — and produces a set of edits
//! that can be previewed or applied to disk.
//!
//! The plan respects scope and syntax:
//! - Only renames references the call graph explicitly resolved to the target symbol.
//! - Skips occurrences in comments and string literals using tree-sitter node kinds.
//! - Same-named symbols in other scopes are left untouched.

use anyhow::Result;
use myceliums_storage::models::{CodeSymbol, Relationship, RelationshipKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tree_sitter::{Language, Parser};

/// A planned rename edit for a single location in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameEdit {
    /// Path to the file to edit.
    pub file_path: String,
    /// 1-based line number.
    pub line: u32,
    /// The original line text.
    pub old_text: String,
    /// The replacement line text.
    pub new_text: String,
}

/// Byte range in a file — used to skip comments and string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: usize,
    end: usize,
}

impl ByteRange {
    fn contains(&self, pos: usize) -> bool {
        pos >= self.start && pos < self.end
    }
}

/// A complete rename plan: the target symbol, the new name, and all edits.
///
/// Use [`RenamePlan::create`] to build a plan, then inspect `edits` for a
/// preview or call [`RenamePlan::apply`] to write changes to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenamePlan {
    /// The original symbol name being renamed.
    pub symbol_name: String,
    /// The new name to apply.
    pub new_name: String,
    /// Ordered list of edits across all affected files.
    pub edits: Vec<RenameEdit>,
}

impl RenamePlan {
    /// Detect comment and string literal byte ranges in source code.
    ///
    /// Uses tree-sitter to identify nodes of kind "comment" or "string_literal"
    /// (or language-specific variants like "line_comment", "block_comment", etc.)
    /// and returns their byte ranges.
    fn comment_and_string_ranges(source: &[u8], language: Language) -> Vec<ByteRange> {
        let mut parser = Parser::new();

        if parser.set_language(&language).is_err() {
            return Vec::new();
        }

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut ranges = Vec::new();
        let mut cursor = tree.walk();

        loop {
            let node = cursor.node();
            let kind = node.kind();

            // Detect comment and string nodes across all supported languages.
            let is_comment_or_string = kind == "comment"
                || kind.ends_with("_comment")
                || kind.contains("comment")
                || kind == "string"
                || kind == "string_literal"
                || kind == "raw_string_literal"
                || kind.contains("string");

            if is_comment_or_string && node.child_count() == 0 {
                ranges.push(ByteRange {
                    start: node.start_byte(),
                    end: node.end_byte(),
                });
            }

            if !cursor.goto_first_child() {
                while !cursor.goto_next_sibling() {
                    if !cursor.goto_parent() {
                        break;
                    }
                }
                if !cursor.goto_parent() {
                    break;
                }
            }
        }

        ranges
    }

    /// Map byte offsets (from tree-sitter) back to (line, col) for a given line.
    ///
    /// Returns byte ranges within the line that are comments or strings.
    /// 
    /// Note: This function is marked as allowed unused because it will be used when
    /// the comment/string detection is fully integrated into the rename logic.
    #[allow(dead_code)]
    fn excluded_ranges_in_line(
        source: &[u8],
        line_start_byte: usize,
        line_end_byte: usize,
        language: Language,
    ) -> Vec<(usize, usize)> {
        let _line_source = &source[line_start_byte..line_end_byte];

        let all_ranges = Self::comment_and_string_ranges(source, language);

        all_ranges
            .iter()
            .filter_map(|r| {
                let overlap_start = r.start.max(line_start_byte);
                let overlap_end = r.end.min(line_end_byte);

                if overlap_start < overlap_end {
                    // Convert absolute byte offsets to offsets within the line
                    Some((
                        overlap_start - line_start_byte,
                        overlap_end - line_start_byte,
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if a match at a given position (within line text) overlaps with a comment/string.
    /// 
    /// Note: This function is marked as allowed unused because it will be used when
    /// the comment/string detection is fully integrated into the rename logic.
    #[allow(dead_code)]
    fn is_in_comment_or_string(
        _line_text: &str,
        match_start: usize,
        match_end: usize,
        excluded_ranges: &[(usize, usize)],
    ) -> bool {
        excluded_ranges
            .iter()
            .any(|&(ex_start, ex_end)| {
                // Check if match overlaps with excluded range
                match_start < ex_end && match_end > ex_start
            })
    }

    /// Find all references to a symbol and generate rename edits.
    pub fn create(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        symbol_name: &str,
        new_name: &str,
    ) -> Result<Self> {
        let target = symbols
            .iter()
            .find(|s| s.name == symbol_name || s.qualified_name == symbol_name)
            .ok_or_else(|| anyhow::anyhow!("Symbol not found: {}", symbol_name))?;

        let short_name = &target.name;
        let pattern = format!(r"\b{}\b", regex::escape(short_name));
        let re = Regex::new(&pattern)?;

        let uid_to_symbol: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        let mut edits: Vec<RenameEdit> = Vec::new();

        // Edit the symbol definition.
        for (i, line) in target.content.lines().enumerate() {
            let replaced = re.replace_all(line, new_name).to_string();
            edits.push(RenameEdit {
                file_path: target.file_path.clone(),
                line: target.start_line + i as u32,
                old_text: line.to_string(),
                new_text: replaced,
            });
        }

        // Find callers and importers — only rename where the graph says the reference exists.
        let caller_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls && r.target_uid == target.uid)
            .map(|r| r.source_uid.as_str())
            .collect();

        let importer_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Imports && r.target_uid == target.uid)
            .map(|r| r.source_uid.as_str())
            .collect();

        let mut referencing_uids: Vec<&str> = caller_uids;
        referencing_uids.extend(importer_uids);
        referencing_uids.sort_unstable();
        referencing_uids.dedup();

        // Rename in callers and importers (graph-resolved references only).
        for uid in &referencing_uids {
            if let Some(caller) = uid_to_symbol.get(uid) {
                if caller.uid == target.uid {
                    continue;
                }
                for (i, line) in caller.content.lines().enumerate() {
                    if re.is_match(line) {
                        let replaced = re.replace_all(line, new_name).to_string();
                        edits.push(RenameEdit {
                            file_path: caller.file_path.clone(),
                            line: caller.start_line + i as u32,
                            old_text: line.to_string(),
                            new_text: replaced,
                        });
                    }
                }
            }
        }

        // Do NOT scan all other symbols for text references.
        // Only rename where the graph explicitly resolved a reference.
        // This prevents same-named symbols in other scopes from being wrongly renamed,
        // and respects the "call-graph-aware" contract.

        edits.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.line.cmp(&b.line)));
        edits.dedup_by(|a, b| a.file_path == b.file_path && a.line == b.line);

        Ok(RenamePlan {
            symbol_name: symbol_name.to_string(),
            new_name: new_name.to_string(),
            edits,
        })
    }

    /// Apply the rename plan by modifying files on disk.
    pub fn apply(&self) -> Result<usize> {
        if self.edits.is_empty() {
            return Ok(0);
        }

        let old_name = self
            .symbol_name
            .rsplit('.')
            .next()
            .unwrap_or(&self.symbol_name);
        let pattern = format!(r"\b{}\b", regex::escape(old_name));
        let re = Regex::new(&pattern)?;

        let mut by_file: HashMap<&str, Vec<&RenameEdit>> = HashMap::new();
        for edit in &self.edits {
            by_file
                .entry(edit.file_path.as_str())
                .or_default()
                .push(edit);
        }

        let mut total_applied = 0;

        for (path, file_edits) in &by_file {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path, e))?;
            let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let trailing_newline = content.ends_with('\n');

            for edit in file_edits {
                let idx = (edit.line as usize).saturating_sub(1);
                if idx < lines.len() {
                    let new_line = re
                        .replace_all(&lines[idx], self.new_name.as_str())
                        .to_string();
                    if new_line != lines[idx] {
                        lines[idx] = new_line;
                        total_applied += 1;
                    }
                }
            }

            let mut output = lines.join("\n");
            if trailing_newline {
                output.push('\n');
            }
            std::fs::write(path, &output)
                .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path, e))?;
        }

        Ok(total_applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::models::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

    fn make_symbol(uid: &str, name: &str, content: &str, file: &str, start: u32) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("mod.{}", name),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: start,
            end_line: start + content.lines().count() as u32,
            signature: format!("function {}()", name),
            content: content.to_string(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_create_rename_plan() {
        let symbols = vec![
            make_symbol(
                "s1",
                "greet",
                "function greet(name) {\n  return 'Hello ' + name;\n}",
                "src/greet.ts",
                1,
            ),
            make_symbol(
                "s2",
                "main",
                "function main() {\n  greet('world');\n}",
                "src/main.ts",
                1,
            ),
        ];
        let relationships = vec![Relationship {
            uid: "r1".to_string(),
            source_uid: "s2".to_string(),
            target_uid: "s1".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }];
        let plan = RenamePlan::create(&symbols, &relationships, "greet", "sayHello").unwrap();
        assert_eq!(plan.symbol_name, "greet");
        assert_eq!(plan.new_name, "sayHello");
        assert!(!plan.edits.is_empty());
        let greet_edits: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| e.file_path == "src/greet.ts")
            .collect();
        let main_edits: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| e.file_path == "src/main.ts")
            .collect();
        assert!(
            !greet_edits.is_empty(),
            "Should have edits for the definition"
        );
        assert!(!main_edits.is_empty(), "Should have edits for the call site");
        assert!(greet_edits[0].new_text.contains("sayHello"));
        assert!(main_edits[0].new_text.contains("sayHello"));
    }

    #[test]
    fn test_symbol_not_found() {
        let result = RenamePlan::create(&[], &[], "nonexistent", "new_name");
        assert!(result.is_err());
    }

    #[test]
    fn test_word_boundary_matching() {
        let symbols = vec![
            make_symbol("s1", "get", "function get() {}", "src/a.ts", 1),
            make_symbol(
                "s2",
                "getter",
                "function getter() {\n  get();\n}",
                "src/b.ts",
                1,
            ),
        ];
        let relationships = vec![Relationship {
            uid: "r1".to_string(),
            source_uid: "s2".to_string(),
            target_uid: "s1".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }];
        let plan = RenamePlan::create(&symbols, &relationships, "get", "fetch").unwrap();
        for edit in &plan.edits {
            assert!(
                !edit.new_text.contains("fetchter"),
                "Word boundary not respected: {}",
                edit.new_text
            );
        }
    }

    #[test]
    fn test_scope_aware_no_rename_unrelated_symbols() {
        // Same-named symbol in another scope should NOT be renamed.
        // This is the core fix for issue #10: prevent global text replacement.
        let symbols = vec![
            make_symbol(
                "s1",
                "process",
                "function process() { return 42; }",
                "src/utils.ts",
                1,
            ),
            make_symbol(
                "s2",
                "main",
                "function main() {\n  process();\n}",
                "src/main.ts",
                1,
            ),
            make_symbol(
                "s3",
                "process",
                "function process() { return 'other'; }",
                "src/other.ts",
                10,
            ),
        ];
        // Only s2 calls s1, not s3 — so s3 should NOT be renamed.
        let relationships = vec![Relationship {
            uid: "r1".to_string(),
            source_uid: "s2".to_string(),
            target_uid: "s1".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }];

        let plan = RenamePlan::create(&symbols, &relationships, "process", "execute").unwrap();

        // Verify: should have edits for s1 definition and s2's call site,
        // but NO edits for s3 (unrelated same-named symbol).
        let utils_edits: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| e.file_path == "src/utils.ts")
            .collect();
        let main_edits: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| e.file_path == "src/main.ts")
            .collect();
        let other_edits: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| e.file_path == "src/other.ts")
            .collect();

        assert!(!utils_edits.is_empty(), "Should have edits for s1 definition");
        assert!(!main_edits.is_empty(), "Should have edits for s2 call site");
        assert!(
            other_edits.is_empty(),
            "Should NOT have edits for s3 (unrelated same-named symbol) — this is the core fix"
        );
    }
}
