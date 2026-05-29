//! Line-based parser for content files: Markdown, MDX, and plain text.
//!
//! Extracts headings (H1–H6) as `Section` symbols and inline Markdown links
//! as `ImportInfo` entries. Uses no external parsing crates beyond `regex`,
//! which is already in the workspace dependency tree.

use std::sync::OnceLock;

use myceliums_storage::SymbolKind;
use regex::Regex;

use crate::parser::{ImportInfo, ParseResult, ParsedSymbol, SourceLanguage};

// Compiled regex patterns, initialised once per process.
static HEADING_RE: OnceLock<Regex> = OnceLock::new();
static LINK_RE: OnceLock<Regex> = OnceLock::new();
static MARKUP_RE: OnceLock<Regex> = OnceLock::new();

fn heading_re() -> &'static Regex {
    HEADING_RE.get_or_init(|| Regex::new(r"^(#{1,6})\s+(.+)").expect("valid heading regex"))
}

fn link_re() -> &'static Regex {
    LINK_RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("valid link regex"))
}

/// Strips common inline Markdown markup from a heading text to produce a clean
/// symbol name: removes `**`, `*`, `__`, `_`, backtick spans, and link syntax.
fn strip_inline_markup(text: &str) -> String {
    let markup = MARKUP_RE.get_or_init(|| {
        Regex::new(
            r"\[([^\]]+)\]\([^)]+\)|\*\*([^*]+)\*\*|\*([^*]+)\*|__([^_]+)__|_([^_]+)_|`([^`]+)`",
        )
        .expect("valid markup regex")
    });
    markup
        .replace_all(text, |caps: &regex::Captures| {
            // Return the first non-empty captured group (the visible text).
            for i in 1..=6 {
                if let Some(m) = caps.get(i) {
                    return m.as_str().to_string();
                }
            }
            String::new()
        })
        .trim()
        .to_string()
}

/// Returns true when a line starts a fenced code block (``` or ~~~).
fn is_fence_delimiter(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

/// Parse Markdown or MDX source into symbols (headings) and imports (links).
fn parse_markdown(source: &str) -> ParseResult {
    let mut symbols: Vec<ParsedSymbol> = Vec::new();
    let mut imports: Vec<ImportInfo> = Vec::new();
    let mut in_fence = false;

    // We accumulate open sections as (index-into-symbols, heading-level).
    // When we encounter a heading of equal or higher level we close the previous one.
    let mut open_sections: Vec<(usize, usize)> = Vec::new();

    let lines: Vec<&str> = source.lines().collect();
    let total_lines = lines.len() as u32;

    // First pass: find the first H1 for the Document symbol name.
    let doc_name = lines.iter().find_map(|line| {
        if let Some(caps) = heading_re().captures(line) {
            if caps[1].len() == 1 {
                return Some(strip_inline_markup(&caps[2]));
            }
        }
        None
    });

    // Emit the Document symbol spanning the whole file.
    symbols.push(ParsedSymbol {
        name: doc_name.clone().unwrap_or_else(|| "document".to_string()),
        kind: SymbolKind::Document,
        start_line: 1,
        end_line: total_lines.max(1),
        signature: String::new(),
        content: source.to_string(),
        parent_name: None,
        metadata: None,
    });

    for (idx, line) in lines.iter().enumerate() {
        let line_no = idx as u32 + 1;

        // Toggle fence state.
        if is_fence_delimiter(line) {
            in_fence = !in_fence;
            continue;
        }

        if in_fence {
            continue;
        }

        // Extract heading.
        if let Some(caps) = heading_re().captures(line) {
            let level = caps[1].len();
            let raw_text = caps[2].trim();
            let name = strip_inline_markup(raw_text);
            let signature = format!("{} {}", &caps[1], raw_text);

            // Close any open sections at the same or higher level (lower number = higher).
            while let Some(&(sym_idx, open_level)) = open_sections.last() {
                if open_level >= level {
                    symbols[sym_idx].end_line = line_no.saturating_sub(1).max(1);
                    open_sections.pop();
                } else {
                    break;
                }
            }

            let sym_idx = symbols.len();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Section,
                start_line: line_no,
                end_line: 0, // deferred until section closes
                signature,
                content: String::new(),
                parent_name: None,
                metadata: None,
            });
            open_sections.push((sym_idx, level));
        }

        // Extract Markdown links from any non-fence line.
        for caps in link_re().captures_iter(line) {
            let link_text = caps[1].to_string();
            let target = caps[2].to_string();
            // Skip pure URL anchors and http(s) links — they reference external resources.
            if target.starts_with('#') || target.starts_with("http") {
                continue;
            }
            imports.push(ImportInfo {
                local_name: link_text,
                source_module: target,
                original_name: None,
            });
        }
    }

    // Close all remaining open sections at the last line.
    for (sym_idx, _) in open_sections {
        symbols[sym_idx].end_line = total_lines.max(1);
    }

    ParseResult {
        symbols,
        calls: vec![],
        imports,
        rationales: vec![],
        aliases: vec![],
    }
}

/// Parse plain-text source: emits a single `Document` symbol, no headings/links.
fn parse_plain_text(source: &str) -> ParseResult {
    let line_count = source.lines().count() as u32;
    ParseResult {
        symbols: vec![ParsedSymbol {
            name: "document".to_string(),
            kind: SymbolKind::Document,
            start_line: 1,
            end_line: line_count.max(1),
            signature: String::new(),
            content: source.to_string(),
            parent_name: None,
            metadata: None,
        }],
        calls: vec![],
        imports: vec![],
        rationales: vec![],
        aliases: vec![],
    }
}

/// Parse a content file and return a `ParseResult` matching the contract of `SourceParser`.
///
/// Dispatches to the Markdown/MDX line parser or the plain-text emitter based on `lang`.
/// Must only be called with a content language (`lang.is_content() == true`).
pub fn parse_content(source: &str, lang: SourceLanguage) -> ParseResult {
    match lang {
        SourceLanguage::Markdown | SourceLanguage::Mdx => parse_markdown(source),
        SourceLanguage::PlainText => parse_plain_text(source),
        // PDF content is pre-converted to markdown by the pdf module.
        #[cfg(feature = "pdf")]
        SourceLanguage::Pdf => parse_markdown(source),
        _ => unreachable!("parse_content called with non-content language"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn md(src: &str) -> ParseResult {
        parse_content(src, SourceLanguage::Markdown)
    }

    #[test]
    fn test_markdown_headings_yield_sections() {
        let src =
            "# Title\n\nSome prose.\n\n## Section A\n\nMore prose.\n\n### Subsection\n\nDeep.\n";
        let result = md(src);

        // 1 Document + 3 Section symbols
        let sections: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Section)
            .collect();
        let docs: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Document)
            .collect();

        assert_eq!(docs.len(), 1, "expected 1 Document symbol");
        assert_eq!(sections.len(), 3, "expected 3 Section symbols");
        assert_eq!(docs[0].name, "Title");
        assert_eq!(sections[0].name, "Title");
        assert_eq!(sections[1].name, "Section A");
        assert_eq!(sections[2].name, "Subsection");
    }

    #[test]
    fn test_markdown_links_yield_imports() {
        let src = "See [the guide](../docs/guide.md) and [source](../src/main.rs).\n";
        let result = md(src);
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.imports[0].local_name, "the guide");
        assert_eq!(result.imports[0].source_module, "../docs/guide.md");
        assert_eq!(result.imports[1].local_name, "source");
        assert_eq!(result.imports[1].source_module, "../src/main.rs");
    }

    #[test]
    fn test_plain_text_yields_only_document() {
        let src = "Hello world.\n\nThis is plain text with no headings.\n";
        let result = parse_content(src, SourceLanguage::PlainText);
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].kind, SymbolKind::Document);
        assert!(result.imports.is_empty());
        assert!(result.calls.is_empty());
    }

    #[test]
    fn test_heading_inside_code_fence_skipped() {
        let src = "# Real heading\n\n```rust\n# not a heading\n```\n\n## Also real\n";
        let result = md(src);
        let sections: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Section)
            .collect();
        // Only "Real heading" and "Also real" — the fenced line must be skipped.
        assert_eq!(sections.len(), 2);
        assert!(sections.iter().all(|s| s.name != "not a heading"));
    }

    #[test]
    fn test_inline_markup_stripped_from_heading_name() {
        let src = "## **Bold** `code` heading\n";
        let result = md(src);
        let section = result
            .symbols
            .iter()
            .find(|s| s.kind == SymbolKind::Section)
            .expect("should have a Section");
        assert_eq!(section.name, "Bold code heading");
    }

    #[test]
    fn test_http_links_are_skipped() {
        let src = "Read the [docs](https://example.com) or [local](./other.md).\n";
        let result = md(src);
        // Only the local link should be an import.
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_module, "./other.md");
    }

    #[test]
    fn test_mdx_parsed_same_as_markdown() {
        let src = "# MDX Title\n\n## A section\n";
        let result = parse_content(src, SourceLanguage::Mdx);
        let sections: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Section)
            .collect();
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn test_section_line_numbers() {
        let src = "# H1\n\n## H2\n\n### H3\n";
        let result = md(src);
        let sections: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Section)
            .collect();
        assert_eq!(sections[0].start_line, 1); // # H1
        assert_eq!(sections[1].start_line, 3); // ## H2
        assert_eq!(sections[2].start_line, 5); // ### H3
    }
}
