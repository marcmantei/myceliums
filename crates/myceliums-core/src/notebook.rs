//! Jupyter notebook (.ipynb) parser.
//!
//! Parses the JSON structure of `.ipynb` files, extracting:
//! - Code cells: parsed with the appropriate tree-sitter parser based on the
//!   notebook's kernel language metadata.
//! - Markdown cells: emitted as `Document` symbols with their text content.
//!
//! Mixed-language notebooks are supported: the kernel language is used as
//! default, but individual cells can override via cell-level metadata.

use anyhow::{Context, Result};
use myceliums_storage::SymbolKind;
use serde::Deserialize;
use tracing::warn;

use crate::content::parse_content;
use crate::parser::{ParseResult, ParsedSymbol, SourceLanguage, SourceParser};

/// Top-level .ipynb JSON structure (nbformat v4).
#[derive(Deserialize)]
struct Notebook {
    /// Notebook-level metadata (contains kernel info).
    metadata: NotebookMetadata,
    /// The ordered list of cells.
    cells: Vec<Cell>,
}

#[derive(Deserialize)]
struct NotebookMetadata {
    #[serde(default)]
    kernelspec: Option<KernelSpec>,
    #[serde(default)]
    language_info: Option<LanguageInfo>,
}

#[derive(Deserialize)]
struct KernelSpec {
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct LanguageInfo {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct Cell {
    cell_type: String,
    source: CellSource,
    #[serde(default)]
    metadata: Option<CellMetadata>,
}

#[derive(Deserialize)]
struct CellMetadata {
    #[serde(default)]
    language: Option<String>,
}

/// Cell source can be a single string or an array of strings (lines).
#[derive(Deserialize)]
#[serde(untagged)]
enum CellSource {
    Single(String),
    Lines(Vec<String>),
}

impl CellSource {
    fn join(&self) -> String {
        match self {
            CellSource::Single(s) => s.clone(),
            CellSource::Lines(lines) => lines.join(""),
        }
    }
}

/// Resolve a language name string (from notebook metadata) to a `SourceLanguage`.
fn language_from_name(name: &str) -> Option<SourceLanguage> {
    match name.to_lowercase().as_str() {
        "python" | "python3" | "python2" => Some(SourceLanguage::Python),
        "r" => None, // R not yet supported by tree-sitter in this project
        "julia" => None,
        "javascript" | "nodejs" => Some(SourceLanguage::JavaScript),
        "typescript" => Some(SourceLanguage::TypeScript),
        "rust" => Some(SourceLanguage::Rust),
        "go" | "golang" => Some(SourceLanguage::Go),
        "java" => Some(SourceLanguage::Java),
        "scala" => Some(SourceLanguage::Scala),
        "ruby" => Some(SourceLanguage::Ruby),
        "c" => Some(SourceLanguage::C),
        "c++" | "cpp" => Some(SourceLanguage::Cpp),
        "lua" => Some(SourceLanguage::Lua),
        "swift" => Some(SourceLanguage::Swift),
        "kotlin" => Some(SourceLanguage::Kotlin),
        "php" => Some(SourceLanguage::Php),
        "csharp" | "c#" => Some(SourceLanguage::CSharp),
        "elixir" => Some(SourceLanguage::Elixir),
        "sql" => None, // SQL not yet supported
        _ => None,
    }
}

/// Parse a Jupyter notebook file and return combined results from all cells.
///
/// - Code cells are parsed with tree-sitter using the notebook's kernel language.
/// - Markdown cells produce `Document` symbols via the content parser.
///
/// The `line_offset` for each cell's symbols is accumulated so that line numbers
/// in the returned symbols correspond to a virtual concatenation of all cells.
pub fn parse_notebook(source: &str) -> Result<ParseResult> {
    let notebook: Notebook = serde_json::from_str(source).context("Failed to parse .ipynb JSON")?;

    // Determine the default language from notebook metadata.
    let default_lang = notebook
        .metadata
        .language_info
        .as_ref()
        .and_then(|li| li.name.as_deref())
        .or_else(|| {
            notebook
                .metadata
                .kernelspec
                .as_ref()
                .and_then(|ks| ks.language.as_deref())
        })
        .and_then(language_from_name);

    let mut all_symbols: Vec<ParsedSymbol> = Vec::new();
    let mut all_calls = Vec::new();
    let mut all_imports = Vec::new();
    let mut all_rationales = Vec::new();

    // Track a running line offset so symbols have unique, monotonically
    // increasing line numbers across cells.
    let mut line_offset: u32 = 0;
    let mut cell_index: usize = 0;

    for cell in &notebook.cells {
        cell_index += 1;
        let cell_source = cell.source.join();
        if cell_source.trim().is_empty() {
            continue;
        }

        let cell_lines = cell_source.lines().count() as u32;

        match cell.cell_type.as_str() {
            "code" => {
                // Determine language for this cell (cell override or notebook default).
                let cell_lang = cell
                    .metadata
                    .as_ref()
                    .and_then(|m| m.language.as_deref())
                    .and_then(language_from_name)
                    .or(default_lang);

                let lang = match cell_lang {
                    Some(l) => l,
                    None => {
                        warn!(
                            "Skipping code cell {} — unsupported or unknown language",
                            cell_index
                        );
                        line_offset += cell_lines;
                        continue;
                    }
                };

                // Parse the code cell with tree-sitter.
                let result = match SourceParser::new(lang) {
                    Ok(mut parser) => match parser.parse(&cell_source) {
                        Ok(r) => r,
                        Err(e) => {
                            warn!("Failed to parse code cell {}: {}", cell_index, e);
                            line_offset += cell_lines;
                            continue;
                        }
                    },
                    Err(e) => {
                        warn!(
                            "Failed to initialize parser for code cell {}: {}",
                            cell_index, e
                        );
                        line_offset += cell_lines;
                        continue;
                    }
                };

                // Offset line numbers so they are unique across the notebook.
                for mut sym in result.symbols {
                    sym.start_line += line_offset;
                    sym.end_line += line_offset;
                    all_symbols.push(sym);
                }
                for mut call in result.calls {
                    call.line += line_offset;
                    all_calls.push(call);
                }
                for mut rat in result.rationales {
                    rat.line += line_offset;
                    all_rationales.push(rat);
                }
                all_imports.extend(result.imports);
            }
            "markdown" => {
                // Parse markdown cells using the content parser to get
                // Document/Section symbols.
                let result = parse_content(&cell_source, SourceLanguage::Markdown);
                for mut sym in result.symbols {
                    sym.start_line += line_offset;
                    sym.end_line += line_offset;
                    // Tag markdown cell symbols with a cell index parent for context.
                    if sym.kind == SymbolKind::Document {
                        sym.name = format!("cell-{}: {}", cell_index, sym.name);
                    }
                    all_symbols.push(sym);
                }
                all_imports.extend(result.imports);
            }
            // raw cells and other types are ignored
            _ => {}
        }

        line_offset += cell_lines;
    }

    Ok(ParseResult {
        symbols: all_symbols,
        calls: all_calls,
        imports: all_imports,
        rationales: all_rationales,
        aliases: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_notebook(kernel_lang: &str, cells: &[(&str, &str)]) -> String {
        let cells_json: Vec<String> = cells
            .iter()
            .map(|(cell_type, source)| {
                let escaped = source.replace('\\', "\\\\").replace('"', "\\\"");
                let lines: Vec<String> = escaped
                    .split('\n')
                    .map(|l| format!("\"{}\\n\"", l))
                    .collect();
                format!(
                    r#"{{"cell_type": "{}", "source": [{}], "metadata": {{}}}}"#,
                    cell_type,
                    lines.join(", ")
                )
            })
            .collect();

        format!(
            r#"{{
  "metadata": {{
    "kernelspec": {{ "language": "{}" }},
    "language_info": {{ "name": "{}" }}
  }},
  "cells": [{}],
  "nbformat": 4,
  "nbformat_minor": 5
}}"#,
            kernel_lang,
            kernel_lang,
            cells_json.join(",\n    ")
        )
    }

    #[test]
    fn test_parse_python_notebook() {
        let nb = make_notebook(
            "python",
            &[
                ("code", "def hello():\n    pass"),
                ("markdown", "# My Notebook"),
                ("code", "def world():\n    return 42"),
            ],
        );

        let result = parse_notebook(&nb).unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"), "should find hello function");
        assert!(names.contains(&"world"), "should find world function");
        assert!(
            names.iter().any(|n| n.contains("My Notebook")),
            "should find markdown document symbol"
        );
    }

    #[test]
    fn test_parse_empty_cells_skipped() {
        let nb = make_notebook("python", &[("code", ""), ("markdown", "")]);
        let result = parse_notebook(&nb).unwrap();
        assert!(result.symbols.is_empty());
    }

    #[test]
    fn test_parse_unsupported_language() {
        let nb = make_notebook("julia", &[("code", "function hello() end")]);
        let result = parse_notebook(&nb).unwrap();
        // Julia is not supported, so code cells should be skipped
        assert!(result.symbols.is_empty());
    }

    #[test]
    fn test_parse_javascript_notebook() {
        let nb = make_notebook(
            "javascript",
            &[("code", "function greet(name) { return name; }")],
        );
        let result = parse_notebook(&nb).unwrap();
        assert!(result.symbols.iter().any(|s| s.name == "greet"));
    }

    #[test]
    fn test_line_offsets_monotonic() {
        let nb = make_notebook(
            "python",
            &[
                ("code", "def a():\n    pass\n\ndef b():\n    pass"),
                ("code", "def c():\n    pass"),
            ],
        );
        let result = parse_notebook(&nb).unwrap();
        let lines: Vec<u32> = result.symbols.iter().map(|s| s.start_line).collect();
        // Each symbol's start line should be >= the previous
        for i in 1..lines.len() {
            assert!(
                lines[i] >= lines[i - 1],
                "line numbers should be monotonically increasing"
            );
        }
    }

    #[test]
    fn test_invalid_json_returns_error() {
        let result = parse_notebook("not valid json");
        assert!(result.is_err());
    }
}
