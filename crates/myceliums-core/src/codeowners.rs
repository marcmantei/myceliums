//! CODEOWNERS file parsing and ownership resolution.
//!
//! Parses GitHub-style CODEOWNERS files and matches symbols to their owners
//! using glob-like pattern matching (last match wins).

use myceliums_storage::CodeSymbol;
use serde::Serialize;
use std::collections::BTreeSet;

/// A single CODEOWNERS rule.
#[derive(Debug, Clone, Serialize)]
pub struct OwnershipEntry {
    pub pattern: String,
    pub owners: Vec<String>,
}

/// Ownership for a specific file.
#[derive(Debug, Clone, Serialize)]
pub struct FileOwnership {
    pub file_path: String,
    pub owners: Vec<String>,
}

/// Summary of ownership across the codebase.
#[derive(Debug, Clone, Serialize)]
pub struct OwnershipReport {
    pub owned_files: Vec<FileOwnership>,
    pub unowned_files: Vec<String>,
    pub total_rules: usize,
}

/// Parse a CODEOWNERS file into ownership entries.
///
/// Skips comments (lines starting with `#`) and blank lines.
/// Each rule is `<pattern> <owner1> <owner2> ...`.
pub fn parse_codeowners(content: &str) -> Vec<OwnershipEntry> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            Some(OwnershipEntry {
                pattern: parts[0].to_string(),
                owners: parts[1..].iter().map(|s| s.to_string()).collect(),
            })
        })
        .collect()
}

/// Match symbols to owners using CODEOWNERS rules.
///
/// Follows GitHub convention: last matching rule wins.
pub fn compute_ownership(symbols: &[CodeSymbol], entries: &[OwnershipEntry]) -> OwnershipReport {
    let unique_files: BTreeSet<&str> = symbols.iter().map(|s| s.file_path.as_str()).collect();
    let mut owned_files = Vec::new();
    let mut unowned_files = Vec::new();

    for file in unique_files {
        let mut matched_owners: Option<Vec<String>> = None;
        // Iterate in order — last match wins
        for entry in entries {
            if matches_pattern(&entry.pattern, file) {
                matched_owners = Some(entry.owners.clone());
            }
        }
        match matched_owners {
            Some(owners) => owned_files.push(FileOwnership {
                file_path: file.to_string(),
                owners,
            }),
            None => unowned_files.push(file.to_string()),
        }
    }

    OwnershipReport {
        owned_files,
        unowned_files,
        total_rules: entries.len(),
    }
}

/// Simple glob pattern matching for CODEOWNERS patterns.
fn matches_pattern(pattern: &str, path: &str) -> bool {
    // Directory pattern: "src/auth/" matches anything under src/auth/
    if pattern.ends_with('/') {
        return path.starts_with(pattern);
    }

    // Extension wildcard: "*.rs" matches any .rs file
    if pattern.starts_with("*.") {
        let ext = &pattern[1..];
        return path.ends_with(ext);
    }

    // Double-star prefix: "**/foo" matches foo anywhere in path
    if let Some(suffix) = pattern.strip_prefix("**/") {
        return path.ends_with(suffix) || path.contains(&format!("/{}", suffix));
    }

    // Exact match or prefix match
    path == pattern || path.starts_with(&format!("{}/", pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn make_symbol(uid: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: uid.to_string(),
            qualified_name: uid.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_parse_codeowners_basic() {
        let content = "*.rs @rustteam\nsrc/auth/ @authteam @security";
        let entries = parse_codeowners(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].pattern, "*.rs");
        assert_eq!(entries[0].owners, vec!["@rustteam"]);
        assert_eq!(entries[1].owners, vec!["@authteam", "@security"]);
    }

    #[test]
    fn test_parse_skips_comments() {
        let content = "# This is a comment\n\n*.rs @rustteam\n# Another comment";
        let entries = parse_codeowners(content);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_last_match_wins() {
        let entries = parse_codeowners("*.rs @team1\nsrc/auth/ @team2");
        let symbols = vec![make_symbol("login", "src/auth/login.rs")];

        let report = compute_ownership(&symbols, &entries);
        assert_eq!(report.owned_files.len(), 1);
        // src/auth/ matches last, so @team2 wins
        assert_eq!(report.owned_files[0].owners, vec!["@team2"]);
    }

    #[test]
    fn test_unowned_files() {
        let entries = parse_codeowners("*.py @pythonteam");
        let symbols = vec![make_symbol("main", "src/main.rs")];

        let report = compute_ownership(&symbols, &entries);
        assert_eq!(report.owned_files.len(), 0);
        assert_eq!(report.unowned_files.len(), 1);
    }

    #[test]
    fn test_directory_pattern() {
        let entries = parse_codeowners("src/auth/ @authteam");
        let symbols = vec![
            make_symbol("login", "src/auth/login.rs"),
            make_symbol("main", "src/main.rs"),
        ];

        let report = compute_ownership(&symbols, &entries);
        assert_eq!(report.owned_files.len(), 1);
        assert_eq!(report.owned_files[0].file_path, "src/auth/login.rs");
        assert_eq!(report.unowned_files.len(), 1);
    }

    #[test]
    fn test_empty_codeowners() {
        let entries = parse_codeowners("");
        let symbols = vec![make_symbol("main", "src/main.rs")];
        let report = compute_ownership(&symbols, &entries);
        assert!(report.owned_files.is_empty());
        assert_eq!(report.unowned_files.len(), 1);
    }
}
