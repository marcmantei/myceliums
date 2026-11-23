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
use myceliums_storage::{DocRef, SymbolKind, SymbolMetadata};
use regex::Regex;
use std::sync::OnceLock;

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

/// Record the doc comment above each of `symbols` in its metadata, with the
/// code references written in it (#180).
pub(crate) fn attach_docs(source: &str, lang: SourceLanguage, symbols: &mut [ParsedSymbol]) {
    let Some(style) = style(lang) else {
        return;
    };
    let lines: Vec<&str> = source.lines().collect();
    for symbol in symbols.iter_mut() {
        if symbol.kind == SymbolKind::Import {
            continue;
        }
        let Some(comment) = leading_comment(&lines, symbol.start_line as usize, style) else {
            continue;
        };
        let refs = if style == Style::CLike {
            doc_refs(&comment, lang)
        } else {
            Vec::new()
        };
        let Some(doc) = joined(&comment) else {
            continue;
        };
        let metadata = symbol.metadata.get_or_insert_with(SymbolMetadata::default);
        metadata.doc = Some(doc);
        metadata.doc_refs = refs;
    }
}

/// The doc comment above the symbol whose first line is `start_line`
/// (1-based), joined into one line, or `None`.
#[cfg(test)]
fn leading_doc(lines: &[&str], start_line: usize, style: Style) -> Option<String> {
    joined(&leading_comment(lines, start_line, style)?)
}

/// The comment lines' text joined into one line, cut to [`MAX_DOC_BYTES`];
/// `None` when there is no text.
fn joined(comment: &[(u32, String)]) -> Option<String> {
    let text = comment
        .iter()
        .flat_map(|(_, l)| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    Some(cut(&text, MAX_DOC_BYTES).to_string())
}

/// The lines of the doc comment above the symbol whose first line is
/// `start_line` (1-based), markers stripped, each with its 1-based line
/// number, top to bottom; `None` if there is none.
fn leading_comment(lines: &[&str], start_line: usize, style: Style) -> Option<Vec<(u32, String)>> {
    // `j` is the number of lines above the one being looked at.
    let mut j = start_line.checked_sub(1)?.min(lines.len());
    let floor = j.saturating_sub(MAX_LINES);
    while j > floor && is_attribute(lines[j - 1].trim(), style) {
        j -= 1;
    }
    let mut collected: Vec<(u32, String)> = Vec::new();
    while j > floor {
        let line = lines[j - 1].trim();
        if line.is_empty() {
            break;
        }
        if let Some(text) = line_comment(line, style) {
            collected.push((j as u32, text.to_string()));
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
        let mut block: Vec<(u32, &str)> = Vec::new();
        let mut k = j;
        let mut opened = false;
        while k > floor {
            let l = lines[k - 1].trim();
            block.push((k as u32, l));
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
        for (n, l) in block {
            collected.push((n, strip_block_line(l, open, close).to_string()));
        }
        break;
    }
    collected.reverse();
    if collected.is_empty() {
        return None;
    }
    Some(collected)
}

// The reference syntaxes of doc comments (#180).
static JAVADOC_LINK: OnceLock<Regex> = OnceLock::new();
static SEE_TAG: OnceLock<Regex> = OnceLock::new();
static CREF: OnceLock<Regex> = OnceLock::new();
static BRACKET_LINK: OnceLock<Regex> = OnceLock::new();

/// The code references in a doc comment's lines: Javadoc/JSDoc/KDoc
/// `{@link X}`, `{@linkplain X label}` and `@see X`; XML-doc `cref="X"`
/// (`<see>`, `<seealso>`, `<exception>`); and, in Kotlin, Rust, Go and Dart,
/// the bracket links `[X]`, `[label][X]` and rustdoc's `[label](path::X)`.
/// Lines inside a fenced code example are skipped. Targets are kept as
/// written; resolving them is `doc_mentions`' job.
fn doc_refs(comment: &[(u32, String)], lang: SourceLanguage) -> Vec<DocRef> {
    let link = JAVADOC_LINK
        .get_or_init(|| Regex::new(r"\{@link(?:plain)?\s+([^\s}]+)").expect("valid regex"));
    let see =
        SEE_TAG.get_or_init(|| Regex::new(r"(?:^|\s)@see\s+([^\s<\x22]+)").expect("valid regex"));
    let cref = CREF.get_or_init(|| Regex::new(r#"\bcref\s*=\s*"([^"]+)""#).expect("valid regex"));
    let bracket = BRACKET_LINK.get_or_init(|| {
        Regex::new(r"\[([^\[\]\n]+)\](?:\[([^\[\]\n]+)\]|\(([^()\n]+)\))?").expect("valid regex")
    });
    let brackets = matches!(
        lang,
        SourceLanguage::Kotlin | SourceLanguage::Rust | SourceLanguage::Go | SourceLanguage::Dart
    );
    let mut refs: Vec<DocRef> = Vec::new();
    let mut push = |target: &str, line: u32| {
        let target = target.trim().trim_matches('`').trim();
        if !target.is_empty() && !refs.iter().any(|r| r.line == line && r.target == target) {
            refs.push(DocRef {
                target: target.to_string(),
                line,
            });
        }
    };
    let mut in_fence = false;
    for (line, text) in comment {
        if text.starts_with("```") || text.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for c in link.captures_iter(text) {
            push(&c[1], *line);
        }
        for c in see.captures_iter(text) {
            push(&c[1], *line);
        }
        for c in cref.captures_iter(text) {
            push(&c[1], *line);
        }
        if brackets {
            for c in bracket.captures_iter(text) {
                if let Some(target) = c.get(2) {
                    push(target.as_str(), *line);
                } else if let Some(path) = c.get(3) {
                    // rustdoc `[label](crate::Foo)`; a URL or file is no code.
                    let path = path.as_str();
                    if !path.contains('/') && !path.contains(".md") && !path.starts_with('#') {
                        push(path, *line);
                    }
                } else {
                    push(&c[1], *line);
                }
            }
        }
    }
    refs
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

    fn refs_of(lang: SourceLanguage, source: &str, name: &str) -> Vec<(String, u32)> {
        let mut parser = SourceParser::new(lang).unwrap();
        let parsed = parser.parse(source).unwrap();
        let symbol = parsed
            .symbols
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol {name}"));
        symbol
            .metadata
            .map(|m| m.doc_refs)
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.target, r.line))
            .collect()
    }

    #[test]
    fn javadoc_links_and_see_tags_are_recorded_with_their_lines() {
        let source = "class Cache {\n    /**\n     * Evicts like {@link Store#evict(String, int)} and\n     * {@linkplain #clear() clears}; see {@code Ignored}.\n     * <pre>{@link InPre}</pre>\n     * @see org.acme.Store\n     * @see <a href=\"https://x\">site</a>\n     */\n    @Override\n    void evict(String key) {}\n}\n";
        assert_eq!(
            refs_of(SourceLanguage::Java, source, "evict"),
            vec![
                ("Store#evict(String,".to_string(), 3),
                ("#clear()".to_string(), 4),
                ("InPre".to_string(), 5),
                ("org.acme.Store".to_string(), 6),
            ]
        );
    }

    #[test]
    fn kdoc_bracket_links_are_recorded() {
        let source = "/**\n * Wraps a [Store] and calls [Store.open] or [the cache][Cache.get].\n * A [link](https://example.com) is no code.\n *\n * ```kotlin\n * val x = list[0]\n * ```\n */\nclass Wrapper\n";
        assert_eq!(
            refs_of(SourceLanguage::Kotlin, source, "Wrapper"),
            vec![
                ("Store".to_string(), 2),
                ("Store.open".to_string(), 2),
                ("Cache.get".to_string(), 2),
            ]
        );
    }

    #[test]
    fn xml_doc_crefs_are_recorded() {
        let source = "namespace Acme {\n    /// <summary>Uses <see cref=\"OrderService.Place(Order)\"/>.</summary>\n    /// <seealso cref=\"T:Acme.Orders.Order\"/>\n    /// <exception cref=\"System.ArgumentException\">bad</exception>\n    public class Checkout {}\n}\n";
        assert_eq!(
            refs_of(SourceLanguage::CSharp, source, "Checkout"),
            vec![
                ("OrderService.Place(Order)".to_string(), 2),
                ("T:Acme.Orders.Order".to_string(), 3),
                ("System.ArgumentException".to_string(), 4),
            ]
        );
    }

    #[test]
    fn rustdoc_intra_doc_links_are_recorded() {
        let source = "/// Opens a [`Store`]; see [`Store::open`] and [the cache](crate::cache::Cache).\n/// A [web link](https://example.com).\npub fn start() {}\n";
        assert_eq!(
            refs_of(SourceLanguage::Rust, source, "start"),
            vec![
                ("Store".to_string(), 1),
                ("Store::open".to_string(), 1),
                ("crate::cache::Cache".to_string(), 1),
            ]
        );
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
