//! Symbol renaming with call-graph-aware reference tracking.
//!
//! [`RenamePlan`] finds all references to a symbol — the definition itself,
//! call sites, imports, and text occurrences — and produces a set of edits
//! that can be previewed or applied to disk.

use anyhow::Result;
use myceliums_storage::models::{CodeSymbol, Relationship, RelationshipKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

        // Edit the symbol definition
        for (i, line) in target.content.lines().enumerate() {
            let replaced = re.replace_all(line, new_name).to_string();
            edits.push(RenameEdit {
                file_path: target.file_path.clone(),
                line: target.start_line + i as u32,
                old_text: line.to_string(),
                new_text: replaced,
            });
        }

        // Find callers and importers
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

        // Scan all other symbols for text references
        let already_scanned: std::collections::HashSet<&str> = {
            let mut set = std::collections::HashSet::new();
            set.insert(target.uid.as_str());
            for uid in &referencing_uids {
                set.insert(uid);
            }
            set
        };

        for sym in symbols {
            if already_scanned.contains(sym.uid.as_str()) {
                continue;
            }
            for (i, line) in sym.content.lines().enumerate() {
                if re.is_match(line) {
                    let replaced = re.replace_all(line, new_name).to_string();
                    edits.push(RenameEdit {
                        file_path: sym.file_path.clone(),
                        line: sym.start_line + i as u32,
                        old_text: line.to_string(),
                        new_text: replaced,
                    });
                }
            }
        }

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
        assert!(
            !main_edits.is_empty(),
            "Should have edits for the call site"
        );
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
}
