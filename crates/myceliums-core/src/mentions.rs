//! Cross-domain mentions extraction.
//!
//! Scans content symbols (emails, documents, sections) for references to
//! code symbols (functions, classes, etc.) and creates `Mentions`
//! relationships with citation metadata.

use crate::llm::LlmProvider;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tracing::info;
use uuid::Uuid;

/// Minimum symbol name length to consider for mention matching.
/// Avoids false positives from very short names like `i`, `x`, `ok`.
const MIN_NAME_LENGTH: usize = 3;

/// Common names to skip — too generic to be meaningful mentions.
const SKIP_NAMES: &[&str] = &[
    "new",
    "get",
    "set",
    "run",
    "main",
    "test",
    "self",
    "this",
    "super",
    "init",
    "None",
    "True",
    "False",
    "null",
    "true",
    "false",
    "undefined",
    "let",
    "var",
    "const",
    "def",
    "fun",
    "pub",
    "use",
    "mod",
    "impl",
    "for",
    "while",
    "loop",
    "break",
    "return",
    "yield",
    "async",
    "await",
    "try",
    "catch",
    "throw",
    "raise",
    "import",
    "from",
    "export",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "type",
    "if",
    "else",
    "elif",
    "match",
    "case",
    "switch",
    "default",
    "and",
    "not",
    "the",
    "was",
    "has",
    "had",
    "are",
    "were",
    "been",
    "str",
    "int",
    "bool",
    "void",
    "any",
    "map",
    "list",
    "vec",
    "err",
    "msg",
    "req",
    "res",
    "ctx",
    "cfg",
    "cmd",
    "arg",
    "opt",
    "len",
    "max",
    "min",
    "sum",
    "add",
    "del",
    "put",
    "pop",
    "log",
    "fmt",
    "std",
    "sys",
    "end",
];

/// A single mention location within a content symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionMatch {
    /// Line number within the content where the mention occurs.
    pub line: u32,
    /// ~80 char context window around the mention.
    pub context: String,
    /// Extraction method: "regex" or "llm".
    pub method: String,
}

/// Metadata for a Mentions relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionMetadata {
    /// All match locations.
    pub matches: Vec<MentionMatch>,
    /// Total occurrence count.
    pub count: usize,
    /// Extraction method used.
    pub extraction_method: String,
}

/// Returns true if the symbol kind is a code symbol (potential mention target).
fn is_code_symbol(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
            | SymbolKind::Variable
            | SymbolKind::Constant
            | SymbolKind::Enum
            | SymbolKind::Module
    )
}

/// Returns true if the symbol kind is a content symbol (potential mention source).
fn is_content_symbol(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Email | SymbolKind::Document | SymbolKind::Section
    )
}

/// Returns true if a name looks like a code identifier (camelCase, snake_case,
/// PascalCase, or contains digits/underscores). Single plain lowercase words
/// like "colors", "location", "value" are too ambiguous and get skipped.
fn looks_like_identifier(name: &str) -> bool {
    // Contains underscore → snake_case (e.g., hash_password)
    if name.contains('_') {
        return true;
    }
    // Contains mixed case → camelCase or PascalCase (e.g., handleAuth, UserModel)
    let has_upper = name.chars().any(|c| c.is_uppercase());
    let has_lower = name.chars().any(|c| c.is_lowercase());
    if has_upper && has_lower {
        return true;
    }
    // ALL_CAPS → constant (e.g., MAX_RETRIES)
    if has_upper && !has_lower && name.len() >= 3 {
        return true;
    }
    // Contains digits → likely an identifier (e.g., auth2, v3)
    if name.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

/// Returns true if the name should be skipped (too short, too common, or
/// looks like a plain English word rather than a code identifier).
fn should_skip_name(name: &str) -> bool {
    name.len() < MIN_NAME_LENGTH || SKIP_NAMES.contains(&name) || !looks_like_identifier(name)
}

/// Extract a ~80-char context window around a match position.
fn extract_context(text: &str, match_start: usize, match_end: usize) -> String {
    let window = 40;
    let raw_start = match_start.saturating_sub(window);
    let raw_end = (match_end + window).min(text.len());

    // Floor to valid char boundary
    let ctx_start = (0..=raw_start)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let ctx_end = (raw_end..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());

    // Snap to word boundaries within the safe range
    let safe_start = text[..ctx_start]
        .rfind(char::is_whitespace)
        .map(|p| {
            // Advance past the whitespace char
            let next = p + text[p..].chars().next().map_or(1, |c| c.len_utf8());
            next.min(ctx_start)
        })
        .unwrap_or(ctx_start);
    let safe_end = text[ctx_end..]
        .find(char::is_whitespace)
        .map(|p| ctx_end + p)
        .unwrap_or(ctx_end);

    let mut ctx = String::new();
    if safe_start > 0 {
        ctx.push_str("...");
    }
    ctx.push_str(text[safe_start..safe_end].trim());
    if safe_end < text.len() {
        ctx.push_str("...");
    }
    ctx
}

/// Find the line number (1-indexed) for a byte offset in text.
fn line_number_at(text: &str, byte_offset: usize) -> u32 {
    text[..byte_offset.min(text.len())].matches('\n').count() as u32 + 1
}

/// Extract mentions of code symbols from content symbols.
///
/// Scans all content symbols (emails, documents, sections) for word-boundary
/// matches of code symbol names. Returns `Mentions` relationships with
/// metadata containing match locations and context.
pub fn extract_mentions(symbols: &[CodeSymbol], repo_id: &str) -> Vec<Relationship> {
    // Separate code symbols (targets) from content symbols (sources)
    let code_symbols: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| is_code_symbol(&s.kind) && !should_skip_name(&s.name))
        .collect();

    let content_symbols: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| is_content_symbol(&s.kind) && !s.content.is_empty())
        .collect();

    if code_symbols.is_empty() || content_symbols.is_empty() {
        return Vec::new();
    }

    // Build name → code symbols lookup (multiple symbols can share a name)
    let mut name_to_symbols: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    for sym in &code_symbols {
        name_to_symbols
            .entry(sym.name.as_str())
            .or_default()
            .push(sym);
    }

    // Deduplicate names and build regex patterns
    let unique_names: Vec<&str> = name_to_symbols.keys().copied().collect();

    // Build a combined regex for all names (much faster than per-name matching)
    // Escape regex special chars in names and join with |
    let escaped_names: Vec<String> = unique_names.iter().map(|n| regex::escape(n)).collect();

    if escaped_names.is_empty() {
        return Vec::new();
    }

    // Build regex in chunks to avoid "too many alternations" issues
    let chunk_size = 200;
    let mut all_relationships: Vec<Relationship> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for chunk in escaped_names.chunks(chunk_size) {
        let pattern = format!(r"\b({})\b", chunk.join("|"));
        let re = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for content_sym in &content_symbols {
            let text = &content_sym.content;

            // Find all matches
            let mut matches_by_name: HashMap<&str, Vec<MentionMatch>> = HashMap::new();

            for m in re.find_iter(text) {
                let matched_name = m.as_str();
                let line = line_number_at(text, m.start());
                let context = extract_context(text, m.start(), m.end());

                matches_by_name
                    .entry(matched_name)
                    .or_default()
                    .push(MentionMatch {
                        line,
                        context,
                        method: "regex".to_string(),
                    });
            }

            // Create Mentions relationships
            for (name, match_locs) in &matches_by_name {
                if let Some(target_syms) = name_to_symbols.get(name) {
                    for target in target_syms {
                        let key = (content_sym.uid.clone(), target.uid.clone());
                        if seen.contains(&key) {
                            continue;
                        }
                        seen.insert(key);

                        let metadata = MentionMetadata {
                            matches: match_locs.clone(),
                            count: match_locs.len(),
                            extraction_method: "regex".to_string(),
                        };

                        all_relationships.push(Relationship {
                            uid: Uuid::new_v4().to_string(),
                            source_uid: content_sym.uid.clone(),
                            target_uid: target.uid.clone(),
                            kind: RelationshipKind::Mentions,
                            repo_id: repo_id.to_string(),
                            metadata: serde_json::to_string(&metadata).unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }

    info!(
        "Extracted {} mentions from {} content symbols referencing {} code symbols",
        all_relationships.len(),
        content_symbols.len(),
        code_symbols.len()
    );

    all_relationships
}

// ── LLM-based extraction ──────────────────────────────────────────────

/// LLM request payload for identifying semantic mentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMentionsRequest {
    /// Content to analyze for mentions.
    pub content: String,
    /// Symbol registry: list of available symbols.
    pub symbols: Vec<SymbolInfo>,
}

/// Information about a single symbol for LLM analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (function, class, etc.).
    pub kind: String,
    /// File path where the symbol is located.
    pub file: String,
}

/// LLM response for identified semantic mentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMentionsResponse {
    /// Identified mentions.
    pub mentions: Vec<LlmMention>,
}

/// A single mention identified by LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMention {
    /// Name of the mentioned symbol.
    pub symbol_name: String,
    /// Confidence score (0.0-1.0).
    pub confidence: f64,
    /// Explanation of why this symbol is mentioned.
    pub reason: String,
}

/// Extract semantic mentions using an LLM provider.
///
/// Analyzes content symbols for implicit references to code symbols.
/// For example, "the authentication handler" might refer to `handleAuth()`.
///
/// # Algorithm
/// 1. Filter symbols: separate code (targets) from content (sources)
/// 2. For each content symbol:
///    - Truncate content to ~4000 chars
///    - Build registry of candidate code symbols (up to 100)
///    - Send to LLM for semantic analysis
///    - Parse response and create Mentions relationships
/// 3. Return relationships with confidence scores and LLM metadata
///
/// # Error Handling
/// Gracefully handles LLM errors without crashing the analysis.
/// Returns empty vec if LLM unavailable or times out.
///
/// # Arguments
/// * `symbols` - All code and content symbols
/// * `llm` - LLM provider instance
/// * `repo_id` - Repository identifier for relationships
/// * `max_content_chars` - Maximum content length (default ~4000)
/// * `max_symbols` - Maximum symbols in registry (default ~100)
/// * `min_confidence` - Minimum confidence threshold (default 0.7)
pub async fn extract_mentions_llm(
    symbols: &[CodeSymbol],
    llm: &dyn LlmProvider,
    repo_id: &str,
    max_content_chars: usize,
    max_symbols: usize,
    min_confidence: f64,
) -> Result<Vec<Relationship>, Box<dyn std::error::Error>> {
    // Separate code symbols (targets) from content symbols (sources)
    let code_symbols: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| is_code_symbol(&s.kind) && !should_skip_name(&s.name))
        .collect();

    let content_symbols: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| is_content_symbol(&s.kind) && !s.content.is_empty())
        .collect();

    if code_symbols.is_empty() || content_symbols.is_empty() {
        return Ok(Vec::new());
    }

    let mut all_relationships: Vec<Relationship> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Process each content symbol
    for content_sym in &content_symbols {
        let text = &content_sym.content;

        // Truncate content to manage token budget
        let truncated_content = if text.len() > max_content_chars {
            format!(
                "{}... [truncated]",
                &text[..max_content_chars.min(text.len())]
            )
        } else {
            text.clone()
        };

        // Build symbol registry (limit to max_symbols)
        let symbol_infos: Vec<SymbolInfo> = code_symbols
            .iter()
            .take(max_symbols)
            .map(|sym| SymbolInfo {
                name: sym.name.clone(),
                kind: format!("{:?}", sym.kind),
                file: sym.file_path.clone(),
            })
            .collect();

        if symbol_infos.is_empty() {
            continue;
        }

        // Build prompt for LLM
        let symbol_list = symbol_infos
            .iter()
            .map(|s| format!("{} ({}) in {}", s.name, s.kind, s.file))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "You are a code analysis expert. Given content and a list of code symbols, identify which symbols are semantically mentioned, even if not explicitly named.\n\nExamples of semantic mentions:\n- \"the auth handler\" → handleAuth\n- \"OAuth implementation\" → oauthProvider\n- \"validation layer\" → validateInput\n\nContent:\n{}\n\nKnown symbols (name | kind | file):\n{}\n\nReturn a JSON object with:\n{{\n  \"mentions\": [\n    {{\n      \"symbol_name\": \"string\",\n      \"confidence\": 0.0-1.0,\n      \"reason\": \"explanation of why this symbol is mentioned\"\n    }}\n  ]\n}}\n\nOnly include high-confidence matches (0.7+). Be conservative.",
            truncated_content, symbol_list
        );

        // Call LLM
        match llm.complete_json(&prompt, 512).await {
            Ok(response) => {
                // Parse response
                if let Ok(resp) = serde_json::from_value::<LlmMentionsResponse>(response) {
                    // Create relationships for high-confidence mentions
                    for mention in resp.mentions {
                        if mention.confidence < min_confidence {
                            continue;
                        }

                        // Find target symbol by name
                        if let Some(target_sym) =
                            code_symbols.iter().find(|s| s.name == mention.symbol_name)
                        {
                            let key = (content_sym.uid.clone(), target_sym.uid.clone());
                            if seen.contains(&key) {
                                continue;
                            }
                            seen.insert(key);

                            // Create LLM metadata
                            let llm_metadata = json!({
                                "method": "llm",
                                "confidence": mention.confidence,
                                "reason": mention.reason,
                                "extraction_timestamp": chrono::Utc::now().to_rfc3339(),
                            });

                            all_relationships.push(Relationship {
                                uid: Uuid::new_v4().to_string(),
                                source_uid: content_sym.uid.clone(),
                                target_uid: target_sym.uid.clone(),
                                kind: RelationshipKind::Mentions,
                                repo_id: repo_id.to_string(),
                                metadata: llm_metadata.to_string(),
                            });
                        }
                    }
                }
            }
            Err(e) => {
                // Log and continue on LLM error
                tracing::debug!(
                    "LLM extraction error for content {}: {}",
                    content_sym.uid,
                    e
                );
            }
        }
    }

    info!(
        "LLM extraction found {} semantic mentions from {} content symbols",
        all_relationships.len(),
        content_symbols.len()
    );

    Ok(all_relationships)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    fn make_code_sym(name: &str, kind: SymbolKind) -> CodeSymbol {
        CodeSymbol {
            uid: format!("code-{}", name),
            name: name.to_string(),
            qualified_name: format!("module::{}", name),
            kind,
            file_path: format!("src/{}.rs", name),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }
    }

    fn make_content_sym(name: &str, kind: SymbolKind, content: &str) -> CodeSymbol {
        CodeSymbol {
            uid: format!("content-{}", name),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind,
            file_path: format!("{}.eml", name),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: content.to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_basic_mention_extraction() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "We need to refactor handleAuth to support OAuth2.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].source_uid, "content-email1");
        assert_eq!(mentions[0].target_uid, "code-handleAuth");
        assert_eq!(mentions[0].kind, RelationshipKind::Mentions);
    }

    #[test]
    fn test_deduplication() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "handleAuth is used here. Also handleAuth is called there. And handleAuth again.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        // Should be 1 relationship with count=3 in metadata
        assert_eq!(mentions.len(), 1);
        let meta: MentionMetadata = serde_json::from_str(&mentions[0].metadata).unwrap();
        assert_eq!(meta.count, 3);
    }

    #[test]
    fn test_short_name_filtered() {
        let symbols = vec![
            make_code_sym("ok", SymbolKind::Function),
            make_content_sym("email1", SymbolKind::Email, "That looks ok to me."),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 0);
    }

    #[test]
    fn test_common_name_filtered() {
        let symbols = vec![
            make_code_sym("new", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "We have a new feature to discuss.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 0);
    }

    #[test]
    fn test_word_boundary_prevents_partial_match() {
        let symbols = vec![
            make_code_sym("handle", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "The handler function processes requests.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        // "handler" should NOT match "handle" (word boundary)
        assert_eq!(mentions.len(), 0);
    }

    #[test]
    fn test_multiple_symbols_mentioned() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_code_sym("validateToken", SymbolKind::Function),
            make_code_sym("UserModel", SymbolKind::Class),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "We need to update handleAuth and validateToken for the new UserModel changes.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 3);
    }

    #[test]
    fn test_metadata_contains_context() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "Line one\nLine two\nWe should refactor handleAuth for OAuth2\nLine four",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 1);
        let meta: MentionMetadata = serde_json::from_str(&mentions[0].metadata).unwrap();
        assert_eq!(meta.matches[0].line, 3);
        assert!(meta.matches[0].context.contains("handleAuth"));
    }

    #[test]
    fn test_no_content_symbols_returns_empty() {
        let symbols = vec![make_code_sym("handleAuth", SymbolKind::Function)];
        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 0);
    }

    #[test]
    fn test_document_symbol_as_source() {
        let symbols = vec![
            make_code_sym("createUser", SymbolKind::Function),
            make_content_sym(
                "doc1",
                SymbolKind::Document,
                "# API Guide\n\nThe createUser function handles registration.",
            ),
        ];

        let mentions = extract_mentions(&symbols, "test-repo");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].source_uid, "content-doc1");
    }

    // LLM-based mentions tests with mock provider

    struct MockLlmProvider {
        response: String,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmProvider for MockLlmProvider {
        async fn complete(&self, _prompt: &str, _max_tokens: u32) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }

        async fn complete_json(
            &self,
            _prompt: &str,
            _max_tokens: u32,
        ) -> anyhow::Result<serde_json::Value> {
            serde_json::from_str(&self.response).context("Failed to parse mock response")
        }
    }

    #[tokio::test]
    async fn test_extract_mentions_llm_semantic_match() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_code_sym("validateToken", SymbolKind::Function),
            make_content_sym(
                "email1",
                SymbolKind::Email,
                "We need to refactor the authentication handler to support OAuth2 token validation.",
            ),
        ];

        let llm_response = r#"{
            "mentions": [
                {
                    "symbol_name": "handleAuth",
                    "confidence": 0.85,
                    "reason": "The text discusses 'the authentication handler', referring to handleAuth"
                },
                {
                    "symbol_name": "validateToken",
                    "confidence": 0.80,
                    "reason": "The text mentions 'token validation', which refers to validateToken"
                }
            ]
        }"#;

        let mock_llm = MockLlmProvider {
            response: llm_response.to_string(),
        };

        let result = extract_mentions_llm(&symbols, &mock_llm, "test-repo", 4000, 100, 0.7).await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        assert_eq!(mentions.len(), 2);

        // Verify first mention
        assert_eq!(mentions[0].source_uid, "content-email1");
        assert_eq!(mentions[0].target_uid, "code-handleAuth");
        assert_eq!(mentions[0].kind, RelationshipKind::Mentions);

        // Verify metadata contains LLM info
        let meta: serde_json::Value = serde_json::from_str(&mentions[0].metadata).unwrap();
        assert_eq!(meta["method"], "llm");
        assert_eq!(meta["confidence"], 0.85);
    }

    #[tokio::test]
    async fn test_extract_mentions_llm_low_confidence_filtered() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym("email1", SymbolKind::Email, "Some content about auth."),
        ];

        let llm_response = r#"{
            "mentions": [
                {
                    "symbol_name": "handleAuth",
                    "confidence": 0.65,
                    "reason": "Maybe related to handleAuth?"
                }
            ]
        }"#;

        let mock_llm = MockLlmProvider {
            response: llm_response.to_string(),
        };

        let result = extract_mentions_llm(
            &symbols,
            &mock_llm,
            "test-repo",
            4000,
            100,
            0.7, // min confidence is 0.7
        )
        .await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        assert_eq!(mentions.len(), 0); // Should be filtered out due to low confidence
    }

    #[tokio::test]
    async fn test_extract_mentions_llm_empty_response() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym("email1", SymbolKind::Email, "Some content."),
        ];

        let llm_response = r#"{
            "mentions": []
        }"#;

        let mock_llm = MockLlmProvider {
            response: llm_response.to_string(),
        };

        let result = extract_mentions_llm(&symbols, &mock_llm, "test-repo", 4000, 100, 0.7).await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        assert_eq!(mentions.len(), 0);
    }

    #[tokio::test]
    async fn test_extract_mentions_llm_no_content_symbols() {
        let symbols = vec![make_code_sym("handleAuth", SymbolKind::Function)];

        let mock_llm = MockLlmProvider {
            response: "{}".to_string(),
        };

        let result = extract_mentions_llm(&symbols, &mock_llm, "test-repo", 4000, 100, 0.7).await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        assert_eq!(mentions.len(), 0); // No content symbols means no mentions
    }

    #[tokio::test]
    async fn test_mentions_deduplication_same_target_ignored() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym("email1", SymbolKind::Email, "handleAuth is mentioned here"),
        ];

        let llm_response = r#"{
            "mentions": [
                {
                    "symbol_name": "handleAuth",
                    "confidence": 0.85,
                    "reason": "Semantic mention of handleAuth"
                }
            ]
        }"#;

        let mock_llm = MockLlmProvider {
            response: llm_response.to_string(),
        };

        let result = extract_mentions_llm(&symbols, &mock_llm, "test-repo", 4000, 100, 0.7).await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        // The mention should be deduplicated if called multiple times (but here it's just called once)
        assert_eq!(mentions.len(), 1);
    }

    #[tokio::test]
    async fn test_mentions_symbol_filtering_only_code_targets() {
        let symbols = vec![
            make_code_sym("handleAuth", SymbolKind::Function),
            make_content_sym("email1", SymbolKind::Email, "content here"),
        ];

        let llm_response = r#"{
            "mentions": [
                {
                    "symbol_name": "email1",
                    "confidence": 0.85,
                    "reason": "Should not match content symbol"
                }
            ]
        }"#;

        let mock_llm = MockLlmProvider {
            response: llm_response.to_string(),
        };

        let result = extract_mentions_llm(&symbols, &mock_llm, "test-repo", 4000, 100, 0.7).await;

        assert!(result.is_ok());
        let mentions = result.unwrap();
        assert_eq!(mentions.len(), 0); // email1 is not a code symbol, so should not match
    }
}
