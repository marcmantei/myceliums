//! Tree-sitter-based source code parsing.
//!
//! Supports 23 programming languages. Each file is parsed into
//! [`ParsedSymbol`]s (functions, classes, etc.), [`CallReference`]s, and
//! [`ImportInfo`] records that feed the call-graph resolver.
//!
//! Layout: this file holds the public types, the language→grammar table
//! (`SourceLanguage::tree_sitter_language`) and the per-language dispatch
//! in [`SourceParser::parse`]. Each submodule adds the `extract_*` methods for
//! one language family to `SourceParser`, with its tests alongside; `common`
//! holds the helpers they share.

use anyhow::{Context, Result};
use myceliums_storage::{CodeSymbol, SymbolKind, SymbolMetadata};
use tree_sitter::{Language, Parser};
use uuid::Uuid;

mod c;
mod common;
mod cpp;
mod dart;
mod dotnet;
mod elixir;
mod go;
mod jvm;
mod lua;
mod php;
mod python;
mod ruby;
mod rust;
mod swift;
mod typescript;
mod zig;

/// A supported source language for tree-sitter parsing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceLanguage {
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Rust,
    Java,
    CSharp,
    C,
    Cpp,
    Ruby,
    Kotlin,
    Swift,
    Php,
    Lua,
    Zig,
    PowerShell,
    Elixir,
    Scala,
    ObjectiveC,
    Dart,
    Vue,
    Svelte,
    Markdown,
    Mdx,
    PlainText,
    Jupyter,
    #[cfg(feature = "pdf")]
    Pdf,
    Email,
    Mbox,
}

impl SourceLanguage {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "ts" => Some(SourceLanguage::TypeScript),
            "tsx" => Some(SourceLanguage::Tsx),
            "js" => Some(SourceLanguage::JavaScript),
            "py" => Some(SourceLanguage::Python),
            "go" => Some(SourceLanguage::Go),
            "rs" => Some(SourceLanguage::Rust),
            "java" => Some(SourceLanguage::Java),
            "cs" => Some(SourceLanguage::CSharp),
            "c" | "h" => Some(SourceLanguage::C),
            "cpp" | "cc" | "cxx" | "hpp" => Some(SourceLanguage::Cpp),
            "rb" => Some(SourceLanguage::Ruby),
            "kt" | "kts" => Some(SourceLanguage::Kotlin),
            "swift" => Some(SourceLanguage::Swift),
            "php" => Some(SourceLanguage::Php),
            "lua" => Some(SourceLanguage::Lua),
            "zig" => Some(SourceLanguage::Zig),
            "ps1" => Some(SourceLanguage::PowerShell),
            "ex" | "exs" => Some(SourceLanguage::Elixir),
            "scala" => Some(SourceLanguage::Scala),
            "m" => Some(SourceLanguage::ObjectiveC),
            "dart" => Some(SourceLanguage::Dart),
            "vue" => Some(SourceLanguage::Vue),
            "svelte" => Some(SourceLanguage::Svelte),
            "md" | "markdown" => Some(SourceLanguage::Markdown),
            "mdx" => Some(SourceLanguage::Mdx),
            "txt" => Some(SourceLanguage::PlainText),
            "ipynb" => Some(SourceLanguage::Jupyter),
            #[cfg(feature = "pdf")]
            "pdf" => Some(SourceLanguage::Pdf),
            "eml" => Some(SourceLanguage::Email),
            "mbox" => Some(SourceLanguage::Mbox),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            SourceLanguage::TypeScript => "typescript",
            SourceLanguage::Tsx => "tsx",
            SourceLanguage::JavaScript => "javascript",
            SourceLanguage::Python => "python",
            SourceLanguage::Go => "go",
            SourceLanguage::Rust => "rust",
            SourceLanguage::Java => "java",
            SourceLanguage::CSharp => "csharp",
            SourceLanguage::C => "c",
            SourceLanguage::Cpp => "cpp",
            SourceLanguage::Ruby => "ruby",
            SourceLanguage::Kotlin => "kotlin",
            SourceLanguage::Swift => "swift",
            SourceLanguage::Php => "php",
            SourceLanguage::Lua => "lua",
            SourceLanguage::Zig => "zig",
            SourceLanguage::PowerShell => "powershell",
            SourceLanguage::Elixir => "elixir",
            SourceLanguage::Scala => "scala",
            SourceLanguage::ObjectiveC => "objective-c",
            SourceLanguage::Dart => "dart",
            SourceLanguage::Vue => "vue",
            SourceLanguage::Svelte => "svelte",
            SourceLanguage::Markdown => "markdown",
            SourceLanguage::Mdx => "mdx",
            SourceLanguage::PlainText => "plaintext",
            SourceLanguage::Jupyter => "jupyter",
            #[cfg(feature = "pdf")]
            SourceLanguage::Pdf => "pdf",
            SourceLanguage::Email => "email",
            SourceLanguage::Mbox => "mbox",
        }
    }

    /// Returns true for content files that bypass tree-sitter parsing.
    pub fn is_content(&self) -> bool {
        #[cfg(feature = "pdf")]
        if matches!(self, Self::Pdf) {
            return true;
        }
        matches!(
            self,
            Self::Markdown | Self::Mdx | Self::PlainText | Self::Email | Self::Mbox
        )
    }

    /// The tree-sitter grammar for this language, or `None` for languages
    /// that are not parsed with tree-sitter (content files, notebooks).
    fn tree_sitter_language(&self) -> Option<Language> {
        Some(match self {
            SourceLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SourceLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SourceLanguage::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            SourceLanguage::Python => tree_sitter_python::LANGUAGE.into(),
            SourceLanguage::Go => tree_sitter_go::LANGUAGE.into(),
            SourceLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
            SourceLanguage::Java => tree_sitter_java::LANGUAGE.into(),
            SourceLanguage::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            SourceLanguage::C => tree_sitter_c::LANGUAGE.into(),
            SourceLanguage::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            SourceLanguage::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            SourceLanguage::Kotlin => tree_sitter_kotlin::LANGUAGE.into(),
            SourceLanguage::Swift => tree_sitter_swift::LANGUAGE.into(),
            SourceLanguage::Php => tree_sitter_php::LANGUAGE_PHP_ONLY.into(),
            SourceLanguage::Lua => tree_sitter_lua::LANGUAGE.into(),
            SourceLanguage::Zig => tree_sitter_zig::LANGUAGE.into(),
            SourceLanguage::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
            SourceLanguage::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            SourceLanguage::Scala => tree_sitter_scala::LANGUAGE.into(),
            SourceLanguage::ObjectiveC => tree_sitter_objc::LANGUAGE.into(),
            SourceLanguage::Dart => tree_sitter_dart::LANGUAGE.into(),
            // Vue and Svelte extract <script> blocks and parse as TypeScript
            SourceLanguage::Vue | SourceLanguage::Svelte => {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
            SourceLanguage::Markdown
            | SourceLanguage::Mdx
            | SourceLanguage::PlainText
            | SourceLanguage::Jupyter
            | SourceLanguage::Email
            | SourceLanguage::Mbox => return None,
            #[cfg(feature = "pdf")]
            SourceLanguage::Pdf => return None,
        })
    }
}

/// Tree-sitter parser wrapper for a specific [`SourceLanguage`].
pub struct SourceParser {
    parser: Parser,
    lang: SourceLanguage,
}

/// A symbol extracted from source code (function, class, method, etc.).
#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    /// Symbol name (e.g. `"authenticate"`).
    pub name: String,
    /// Symbol kind (Function, Class, etc.).
    pub kind: SymbolKind,
    /// 1-based start line in the source file.
    pub start_line: u32,
    /// 1-based end line in the source file.
    pub end_line: u32,
    /// Signature text (e.g. `"fn authenticate(user: &str) -> bool"`).
    pub signature: String,
    /// Full source text of the symbol body.
    pub content: String,
    /// Parent symbol name (e.g. class name for a method).
    pub parent_name: Option<String>,
    /// Optional metadata (decorators, return type, superclasses, etc.).
    pub metadata: Option<SymbolMetadata>,
}

/// A single step in an expression chain leading to a call.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionStep {
    /// Variable or function name (e.g. `foo`).
    Ident(String),
    /// Member/field access (e.g. `.bar`).
    Field(String),
    /// Function call `()`.
    Call,
    /// Array index `[]`.
    Index,
    /// `self` / `this` keyword.
    This,
    /// `super` keyword.
    Super,
    /// Constructor call (e.g. `new Foo`).
    New(String),
}

/// A full expression chain with source location.
#[derive(Debug, Clone)]
pub struct CallChain {
    /// Ordered steps from receiver to final call.
    pub steps: Vec<ExpressionStep>,
    /// Line number where the chain occurs.
    pub line: u32,
}

/// A detected function call from caller to callee.
#[derive(Debug, Clone)]
pub struct CallReference {
    /// Name of the calling function.
    pub caller_name: String,
    /// Name of the called function.
    pub callee_name: String,
    /// Line number where the call occurs.
    pub line: u32,
    /// Optional expression chain for method calls.
    pub chain: Option<Vec<ExpressionStep>>,
    /// File path where the call occurs (set during analysis accumulation).
    pub file: Option<String>,
}

/// An import statement linking a local name to a source module.
#[derive(Debug, Clone)]
pub struct ImportInfo {
    /// Local name used in the importing file.
    pub local_name: String,
    /// Module or path being imported from.
    pub source_module: String,
    /// Original exported name if the import is aliased.
    pub original_name: Option<String>,
}

/// The prefix tag on a rationale comment (e.g. NOTE, HACK, TODO).
#[derive(Debug, Clone, PartialEq)]
pub enum RationalePrefix {
    Note,
    Hack,
    Why,
    Todo,
    Fixme,
    Important,
}

impl std::fmt::Display for RationalePrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Note => write!(f, "NOTE"),
            Self::Hack => write!(f, "HACK"),
            Self::Why => write!(f, "WHY"),
            Self::Todo => write!(f, "TODO"),
            Self::Fixme => write!(f, "FIXME"),
            Self::Important => write!(f, "IMPORTANT"),
        }
    }
}

/// A design-rationale comment extracted from source code.
#[derive(Debug, Clone)]
pub struct ParsedRationale {
    /// The prefix tag (NOTE, HACK, TODO, etc.).
    pub prefix: RationalePrefix,
    /// The comment text (after the prefix).
    pub text: String,
    /// 1-based line number where the comment starts.
    pub line: u32,
}

/// The result of parsing a single source file.
#[derive(Debug)]
pub struct ParseResult {
    /// Symbols found in the file.
    pub symbols: Vec<ParsedSymbol>,
    /// Call references found in the file.
    pub calls: Vec<CallReference>,
    /// Import statements found in the file.
    pub imports: Vec<ImportInfo>,
    /// Rationale comments found in the file.
    pub rationales: Vec<ParsedRationale>,
    /// Variable aliases discovered during parsing (local_name, target_name).
    /// Used by the SSA resolver to track assignments like `handler = authenticate`.
    pub aliases: Vec<(String, String)>,
}

impl SourceParser {
    pub fn new(lang: SourceLanguage) -> Result<Self> {
        let mut parser = Parser::new();
        let language = lang.tree_sitter_language().with_context(|| {
            format!(
                "{} is not parsed with tree-sitter; use the content/notebook parsers",
                lang.name()
            )
        })?;
        parser
            .set_language(&language)
            .context("Failed to set language")?;
        Ok(Self { parser, lang })
    }

    /// Parse using the DSL engine if rules are available for this language.
    /// Falls back to the hand-coded path if no DSL rules exist.
    pub fn parse_with_dsl(&mut self, source: &str) -> Result<ParseResult> {
        if let Some(rules) = crate::dsl::rules_for(self.lang) {
            let effective_source: String;
            let parse_source = if matches!(self.lang, SourceLanguage::Vue | SourceLanguage::Svelte)
            {
                effective_source = Self::extract_script_block(source);
                effective_source.as_str()
            } else {
                source
            };

            let tree = self
                .parser
                .parse(parse_source, None)
                .context("Failed to parse source")?;
            let root = tree.root_node();
            let source_bytes = parse_source.as_bytes();

            let engine = crate::dsl::DslEngine::new(&rules, source_bytes);
            let (symbols, calls, imports) = engine.extract(root);

            // Add chain: None and file: None to DSL-produced calls
            let calls = calls
                .into_iter()
                .map(|c| CallReference {
                    caller_name: c.caller_name,
                    callee_name: c.callee_name,
                    line: c.line,
                    chain: c.chain,
                    file: None,
                })
                .collect();

            let rationales = Self::extract_rationales(root, source_bytes);

            return Ok(ParseResult {
                symbols,
                calls,
                imports,
                rationales,
                aliases: vec![],
            });
        }

        // Fall back to hand-coded path
        self.parse(source)
    }

    pub fn parse(&mut self, source: &str) -> Result<ParseResult> {
        // For Vue/Svelte, extract the <script> block and parse as TypeScript
        let effective_source: String;
        let parse_source = if matches!(self.lang, SourceLanguage::Vue | SourceLanguage::Svelte) {
            effective_source = Self::extract_script_block(source);
            effective_source.as_str()
        } else {
            source
        };

        let tree = self
            .parser
            .parse(parse_source, None)
            .context("Failed to parse source")?;
        let root = tree.root_node();
        let source_bytes = parse_source.as_bytes();

        let mut symbols = Vec::new();
        let mut calls = Vec::new();
        let mut imports = Vec::new();
        let mut aliases = Vec::new();

        match self.lang {
            SourceLanguage::TypeScript
            | SourceLanguage::Tsx
            | SourceLanguage::JavaScript
            | SourceLanguage::Vue
            | SourceLanguage::Svelte => {
                self.extract_ts_symbols(root, source_bytes, &mut symbols, None);
                self.extract_ts_calls(root, source_bytes, &mut calls, None);
                self.extract_ts_imports(root, source_bytes, &mut imports);
                self.extract_ts_aliases(root, source_bytes, &mut aliases);
            }
            SourceLanguage::Python => {
                self.extract_py_symbols(root, source_bytes, &mut symbols, None);
                self.extract_py_calls(root, source_bytes, &mut calls, None);
                self.extract_py_imports(root, source_bytes, &mut imports);
                self.extract_py_aliases(root, source_bytes, &mut aliases);
            }
            SourceLanguage::Go => {
                self.extract_go_symbols(root, source_bytes, &mut symbols, None);
                self.extract_go_calls(root, source_bytes, &mut calls, None);
                self.extract_go_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Rust => {
                self.extract_rust_symbols(root, source_bytes, &mut symbols, None);
                self.extract_rust_calls(root, source_bytes, &mut calls, None);
                self.extract_rust_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Java => {
                self.extract_java_symbols(root, source_bytes, &mut symbols, None);
                self.extract_java_calls(root, source_bytes, &mut calls, None);
                self.extract_java_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::CSharp => {
                self.extract_csharp_symbols(root, source_bytes, &mut symbols, None);
                self.extract_csharp_calls(root, source_bytes, &mut calls, None);
                self.extract_csharp_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::C => {
                self.extract_c_symbols(root, source_bytes, &mut symbols, None);
                self.extract_c_calls(root, source_bytes, &mut calls, None);
                self.extract_c_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Cpp => {
                self.extract_cpp_symbols(root, source_bytes, &mut symbols, None);
                self.extract_cpp_calls(root, source_bytes, &mut calls, None);
                self.extract_cpp_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Ruby => {
                self.extract_ruby_symbols(root, source_bytes, &mut symbols, None);
                self.extract_ruby_calls(root, source_bytes, &mut calls, None);
                self.extract_ruby_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Kotlin => {
                self.extract_kotlin_symbols(root, source_bytes, &mut symbols, None);
                self.extract_kotlin_calls(root, source_bytes, &mut calls, None);
                self.extract_kotlin_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Swift => {
                self.extract_swift_symbols(root, source_bytes, &mut symbols, None);
                self.extract_swift_calls(root, source_bytes, &mut calls, None);
                self.extract_swift_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Php => {
                self.extract_php_symbols(root, source_bytes, &mut symbols, None);
                self.extract_php_calls(root, source_bytes, &mut calls, None);
                self.extract_php_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Lua => {
                self.extract_lua_symbols(root, source_bytes, &mut symbols, None);
                self.extract_lua_calls(root, source_bytes, &mut calls, None);
                self.extract_lua_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Zig => {
                self.extract_zig_symbols(root, source_bytes, &mut symbols, None);
                self.extract_zig_calls(root, source_bytes, &mut calls, None);
                self.extract_zig_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::PowerShell => {
                self.extract_powershell_symbols(root, source_bytes, &mut symbols, None);
                self.extract_powershell_calls(root, source_bytes, &mut calls, None);
                self.extract_powershell_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Elixir => {
                self.extract_elixir_symbols(root, source_bytes, &mut symbols, None);
                self.extract_elixir_calls(root, source_bytes, &mut calls, None);
                self.extract_elixir_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Scala => {
                self.extract_scala_symbols(root, source_bytes, &mut symbols, None);
                self.extract_scala_calls(root, source_bytes, &mut calls, None);
                self.extract_scala_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::ObjectiveC => {
                self.extract_objc_symbols(root, source_bytes, &mut symbols, None);
                self.extract_objc_calls(root, source_bytes, &mut calls, None);
                self.extract_objc_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Dart => {
                self.extract_dart_symbols(root, source_bytes, &mut symbols, None);
                self.extract_dart_calls(root, source_bytes, &mut calls, None);
                self.extract_dart_imports(root, source_bytes, &mut imports);
            }
            SourceLanguage::Markdown
            | SourceLanguage::Mdx
            | SourceLanguage::PlainText
            | SourceLanguage::Email
            | SourceLanguage::Mbox => {
                unreachable!("content languages bypass SourceParser::parse()")
            }
            SourceLanguage::Jupyter => {
                unreachable!("Jupyter notebooks bypass SourceParser::parse()")
            }
            #[cfg(feature = "pdf")]
            SourceLanguage::Pdf => {
                unreachable!("PDF content language bypasses SourceParser::parse()")
            }
        }

        // Extract rationale comments from the AST (language-agnostic)
        let rationales = Self::extract_rationales(root, source_bytes);

        Ok(ParseResult {
            symbols,
            calls,
            imports,
            rationales,
            aliases,
        })
    }
}

/// Convert parsed symbols into storage-ready [`CodeSymbol`] records.
pub fn to_code_symbols(parsed: &[ParsedSymbol], file_path: &str, repo_id: &str) -> Vec<CodeSymbol> {
    parsed
        .iter()
        .map(|p| {
            let qualified_name = match &p.parent_name {
                Some(parent) => format!("{}.{}", parent, p.name),
                None => p.name.clone(),
            };
            CodeSymbol {
                uid: Uuid::new_v4().to_string(),
                name: p.name.clone(),
                qualified_name,
                kind: p.kind.clone(),
                file_path: file_path.to_string(),
                start_line: p.start_line,
                end_line: p.end_line,
                signature: p.signature.clone(),
                content: p.content.clone(),
                repo_id: repo_id.to_string(),
                metadata: p
                    .metadata
                    .as_ref()
                    .and_then(|m| serde_json::to_string(m).ok()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "pdf")]
    #[test]
    fn test_pdf_extension_recognized() {
        let lang = SourceLanguage::from_extension("pdf");
        assert_eq!(lang, Some(SourceLanguage::Pdf));
        assert!(lang.unwrap().is_content());
        assert_eq!(lang.unwrap().name(), "pdf");
    }

    #[cfg(not(feature = "pdf"))]
    #[test]
    fn test_pdf_extension_not_recognized_without_feature() {
        assert_eq!(SourceLanguage::from_extension("pdf"), None);
    }
}
