//! Doc comments written directly above a symbol (#157).
//!
//! Tree-sitter keeps comments out of a declaration's node, so a symbol's
//! content never holds the `///`, `/** */` or `#` lines that explain it — the
//! best natural-language description of the code an embedding model could
//! get. This pass reads them from the source: going up from the line above the
//! symbol, past attribute and decorator lines, it collects the contiguous
//! comment lines (or one block comment) and stops at a blank line or code, so
//! a licence header or a comment that belongs to the previous declaration is
//! not taken. Markers are stripped and the text is joined into one line.
//!
//! Python docstrings live inside the body and are taken from there by the
//! embedding template; a `#` comment above a `def` is collected here.

use super::{ParsedSymbol, SourceLanguage};
use myceliums_storage::{SymbolKind, SymbolMetadata};

/// Most bytes of doc text kept per symbol.
pub const MAX_DOC_BYTES: usize = 1_000;

/// Most source lines looked at above a symbol.
const MAX_LINES: usize = 80;

/// The comment syntax of a language family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// `//` and `/* */` (with `///`, `//!`, `/** */` doc variants).
    CLike,
    /// `#`, plus `<# #>` blocks in PowerShell.
    Hash,
    /// `--` and `--[[ ]]`.
    Lua,
}

fn style(lang: SourceLanguage) -> Option<Style> {
    use SourceLanguage as L;
    Some(match lang {
        L::TypeScript
        | L::Tsx
        | L::JavaScript
        | L::Go
        | L::Rust
        | L::Java
        | L::CSharp
        | L::C
        | L::Cpp
        | L::Kotlin
        | L::Swift
        | L::Php
        | L::Scala
        | L::ObjectiveC
        | L::Dart
        | L::Zig
        | L::Vue
        | L::Svelte => Style::CLike,
        L::Python | L::Ruby | L::Elixir | L::PowerShell => Style::Hash,
        L::Lua => Style::Lua,
        _ => return None,
    })
}

/// Record the doc comment above each of `symbols` in its metadata.
pub(crate) fn attach_docs(source: &str, lang: SourceLanguage, symbols: &mut [ParsedSymbol]) {
    let Some(style) = style(lang) else {
        return;
    };
    let lines: Vec<&str> = source.lines().collect();
    for symbol in symbols.iter_mut() {
        if symbol.kind == SymbolKind::Import {
            continue;
        }
        if let Some(doc) = leading_doc(&lines, symbol.start_line as usize, style) {
            symbol
                .metadata
                .get_or_insert_with(SymbolMetadata::default)
                .doc = Some(doc);
        }
    }
}

/// The doc comment above the symbol whose first line is `start_line`
/// (1-based), or `None`.
fn leading_doc(lines: &[&str], start_line: usize, style: Style) -> Option<String> {
    // `j` is the number of lines above the one being looked at.
    let mut j = start_line.checked_sub(1)?.min(lines.len());
    let floor = j.saturating_sub(MAX_LINES);
    while j > floor && is_attribute(lines[j - 1].trim(), style) {
        j -= 1;
    }
    let mut collected: Vec<String> = Vec::new();
    while j > floor {
        let line = lines[j - 1].trim();
        if line.is_empty() {
            break;
        }
        if let Some(text) = line_comment(line, style) {
            collected.push(text.to_string());
            j -= 1;
            continue;
        }
        let Some((open, close)) = block_delimiters(style) else {
            break;
        };
        if !line.ends_with(close) || !collected.is_empty() {
            break;
        }
        // A block comment ending right above: collect it up to its opening.
        let mut block: Vec<&str> = Vec::new();
        let mut k = j;
        let mut opened = false;
        while k > floor {
            let l = lines[k - 1].trim();
            block.push(l);
            k -= 1;
            if let Some(at) = l.find(open) {
                // Code before the opening: a trailing comment of a statement,
                // not a doc.
                if !l[..at].trim().is_empty() {
                    return None;
                }
                opened = true;
                break;
            }
        }
        if !opened {
            return None;
        }
        for l in block {
            collected.push(strip_block_line(l, open, close).to_string());
        }
        break;
    }
    collected.reverse();
    let text = collected
        .iter()
        .flat_map(|l| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    Some(cut(&text, MAX_DOC_BYTES).to_string())
}

/// Decorator and attribute lines that may sit between a doc comment and the
/// declaration (`@Override`, `#[derive(..)]`, `[HttpGet]`).
fn is_attribute(line: &str, style: Style) -> bool {
    match style {
        Style::CLike => {
            line.starts_with('@')
                || line.starts_with("#[")
                || (line.starts_with('[') && line.ends_with(']'))
        }
        Style::Hash => line.starts_with('@'),
        Style::Lua => false,
    }
}

/// The text of a line comment, markers removed, or `None` for other lines.
fn line_comment(line: &str, style: Style) -> Option<&str> {
    match style {
        Style::CLike => {
            let rest = line.strip_prefix("//")?;
            Some(rest.trim_start_matches(['/', '!']).trim())
        }
        Style::Hash => {
            // `#[` is a Rust-style attribute or Elixir sigil, `#!` a shebang.
            if line.starts_with("#[") || line.starts_with("#!") || line.starts_with("<#") {
                return None;
            }
            let rest = line.strip_prefix('#')?;
            Some(rest.trim_start_matches('#').trim())
        }
        Style::Lua => {
            if line.starts_with("--[[") {
                return None;
            }
            Some(line.strip_prefix("--")?.trim_start_matches('-').trim())
        }
    }
}

fn block_delimiters(style: Style) -> Option<(&'static str, &'static str)> {
    match style {
        Style::CLike => Some(("/*", "*/")),
        Style::Hash => Some(("<#", "#>")),
        Style::Lua => Some(("--[[", "]]")),
    }
}

/// One line of a block comment without its delimiters or `*` gutter.
fn strip_block_line<'a>(line: &'a str, open: &str, close: &str) -> &'a str {
    let mut l = line;
    if let Some(i) = l.find(open) {
        l = &l[i + open.len()..];
    }
    if let Some(i) = l.rfind(close) {
        l = &l[..i];
    }
    l.trim().trim_start_matches('*').trim()
}

fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SourceParser;

    fn docs(lang: SourceLanguage, source: &str) -> Vec<(String, Option<String>)> {
        let mut parser = SourceParser::new(lang).unwrap();
        let parsed = parser.parse(source).unwrap();
        parsed
            .symbols
            .into_iter()
            .map(|s| (s.name, s.metadata.and_then(|m| m.doc)))
            .collect()
    }

    fn doc_of(found: &[(String, Option<String>)], name: &str) -> Option<String> {
        found
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no symbol {name} in {found:?}"))
            .1
            .clone()
    }

    #[test]
    fn go_doc_comments_are_attached_and_stop_at_blank_lines() {
        let source = "package gin\n\n// Licence header.\n\n// Next should be used only inside middleware.\n// It executes the pending handlers.\nfunc (c *Context) Next() {\n}\n\nfunc undocumented() {}\n";
        let found = docs(SourceLanguage::Go, source);
        assert_eq!(
            doc_of(&found, "Next").as_deref(),
            Some("Next should be used only inside middleware. It executes the pending handlers.")
        );
        assert_eq!(doc_of(&found, "undocumented"), None);
    }

    #[test]
    fn jsdoc_blocks_lose_their_gutter() {
        let source = "/**\n * Create a CORS middleware.\n * @param options the allowed origins\n */\nexport const cors = (options) => {\n  return options\n}\n";
        let found = docs(SourceLanguage::TypeScript, source);
        assert_eq!(
            doc_of(&found, "cors").as_deref(),
            Some("Create a CORS middleware. @param options the allowed origins")
        );
    }

    #[test]
    fn rust_doc_comments_skip_attributes() {
        let source =
            "/// A point in space.\n#[derive(Debug)]\npub struct Point {\n    x: i32,\n}\n";
        let found = docs(SourceLanguage::Rust, source);
        assert_eq!(
            doc_of(&found, "Point").as_deref(),
            Some("A point in space.")
        );
    }

    #[test]
    fn python_hash_comments_above_a_def_are_collected() {
        let source = "# Look up a user.\n@cached\ndef get_user(uid):\n    return uid\n";
        let found = docs(SourceLanguage::Python, source);
        assert_eq!(
            doc_of(&found, "get_user").as_deref(),
            Some("Look up a user.")
        );
    }

    #[test]
    fn a_statement_or_its_trailing_comment_is_not_a_doc() {
        assert_eq!(
            leading_doc(&["let x = 1;", "fn f() {}"], 2, Style::CLike),
            None
        );
        assert_eq!(
            leading_doc(&["let x = 1; /* trailing */", "fn f() {}"], 2, Style::CLike),
            None
        );
    }

    #[test]
    fn malformed_input_never_panics() {
        for source in ["*/\nfn f() {}", "/*", "", "//", "#[\n", "]]\n--[["] {
            let lines: Vec<&str> = source.lines().collect();
            for start in 0..4 {
                for style in [Style::CLike, Style::Hash, Style::Lua] {
                    let _ = leading_doc(&lines, start, style);
                }
            }
        }
    }

    #[test]
    fn long_docs_are_cut_on_a_char_boundary() {
        let line = format!("// {}", "ü".repeat(MAX_DOC_BYTES));
        let lines = [line.as_str(), "fn f() {}"];
        let doc = leading_doc(&lines, 2, Style::CLike).unwrap();
        assert!(doc.len() <= MAX_DOC_BYTES);
        assert!(doc.chars().all(|c| c == 'ü'));
    }
}
