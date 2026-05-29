//! Tree-sitter-based source code parsing.
//!
//! Supports 23 programming languages. Each file is parsed into
//! [`ParsedSymbol`]s (functions, classes, etc.), [`CallReference`]s, and
//! [`ImportInfo`] records that feed the call-graph resolver.

use anyhow::{Context, Result};
use myceliums_storage::{CodeSymbol, SymbolKind, SymbolMetadata, Visibility};
use tree_sitter::{Language, Node, Parser};
use uuid::Uuid;

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

    fn tree_sitter_language(&self) -> Language {
        match self {
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
            | SourceLanguage::Mbox => {
                unreachable!("content languages do not use tree-sitter")
            }
            #[cfg(feature = "pdf")]
            SourceLanguage::Pdf => {
                unreachable!("PDF content language does not use tree-sitter")
            }
        }
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
        parser
            .set_language(&lang.tree_sitter_language())
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

    fn node_text<'a>(&self, node: Node<'a>, source: &'a [u8]) -> &'a str {
        node.utf8_text(source).unwrap_or("")
    }

    fn extract_signature(&self, node: Node, source: &[u8]) -> String {
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
    fn extract_rationales(node: Node, source: &[u8]) -> Vec<ParsedRationale> {
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

    // --- TypeScript/TSX ---

    fn extract_ts_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            "method_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                    // Recurse into class body with parent set
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ts_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                // Handle: const foo = ..., export const foo = ...
                // Also handle arrow functions: const foo = () => ...
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            let value = child.child_by_field_name("value");
                            let is_arrow = value
                                .map(|v| {
                                    v.kind() == "arrow_function"
                                        || v.kind() == "function_expression"
                                        || v.kind() == "function"
                                })
                                .unwrap_or(false);

                            let sym_kind = if is_arrow {
                                SymbolKind::Function
                            } else if self.node_text(node, source).starts_with("const") {
                                SymbolKind::Constant
                            } else {
                                SymbolKind::Variable
                            };

                            symbols.push(ParsedSymbol {
                                name,
                                kind: sym_kind,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            "export_statement" => {
                // Recurse into exported declarations
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    self.extract_ts_symbols(child, source, symbols, parent);
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_symbols(child, source, symbols, parent);
        }
    }

    fn extract_ts_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_declaration" | "method_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "variable_declarator" => {
                let value = node.child_by_field_name("value");
                let is_fn = value
                    .map(|v| v.kind() == "arrow_function" || v.kind() == "function_expression")
                    .unwrap_or(false);
                if is_fn {
                    node.child_by_field_name("name")
                        .map(|n| self.node_text(n, source).to_string())
                } else {
                    None
                }
            }
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let (callee, chain) = match func_node.kind() {
                    "identifier" => (self.node_text(func_node, source).to_string(), None),
                    "member_expression" => {
                        let chain = self.build_ts_chain(func_node, source);
                        let name = if let Some(prop) = func_node.child_by_field_name("property") {
                            self.node_text(prop, source).to_string()
                        } else {
                            self.node_text(func_node, source).to_string()
                        };
                        (name, chain)
                    }
                    _ => (self.node_text(func_node, source).to_string(), None),
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_calls(child, source, calls, scope);
        }
    }

    /// Build an expression chain from a TypeScript `member_expression` node.
    fn build_ts_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_ts_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    fn collect_ts_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
        match node.kind() {
            "member_expression" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_ts_chain_steps(obj, source, steps);
                }
                if let Some(prop) = node.child_by_field_name("property") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(prop, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_ts_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "this" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            "this" => steps.push(ExpressionStep::This),
            "super" => steps.push(ExpressionStep::Super),
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    fn extract_ts_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "import_statement" {
            let source_node = node.child_by_field_name("source");
            let module = source_node
                .map(|s| {
                    let text = self.node_text(s, source);
                    text.trim_matches('\'').trim_matches('"').to_string()
                })
                .unwrap_or_default();

            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "import_clause" {
                    let clause_cursor = &mut child.walk();
                    for clause_child in child.children(clause_cursor) {
                        match clause_child.kind() {
                            "identifier" => {
                                imports.push(ImportInfo {
                                    local_name: self.node_text(clause_child, source).to_string(),
                                    source_module: module.clone(),
                                    original_name: None,
                                });
                            }
                            "named_imports" => {
                                let spec_cursor = &mut clause_child.walk();
                                for spec in clause_child.children(spec_cursor) {
                                    if spec.kind() == "import_specifier" {
                                        let name_node = spec.child_by_field_name("name");
                                        let alias_node = spec.child_by_field_name("alias");
                                        if let Some(name) = name_node {
                                            let original = self.node_text(name, source).to_string();
                                            let local = alias_node
                                                .map(|a| self.node_text(a, source).to_string())
                                                .unwrap_or_else(|| original.clone());
                                            imports.push(ImportInfo {
                                                local_name: local,
                                                source_module: module.clone(),
                                                original_name: Some(original),
                                            });
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Handle CommonJS require() - for JavaScript files
        // Pattern: const foo = require('module') or var bar = require('./file')
        if node.kind() == "variable_declarator" {
            if let Some(value) = node.child_by_field_name("value") {
                if value.kind() == "call_expression" {
                    if let Some(func) = value.child_by_field_name("function") {
                        if func.kind() == "identifier" && self.node_text(func, source) == "require"
                        {
                            // Extract the module name from arguments
                            let cursor = &mut value.walk();
                            for child in value.children(cursor) {
                                if child.kind() == "arguments" {
                                    let arg_cursor = &mut child.walk();
                                    for arg in child.children(arg_cursor) {
                                        if arg.kind() == "string" {
                                            let module = self.node_text(arg, source);
                                            let module = module
                                                .trim_matches('\'')
                                                .trim_matches('"')
                                                .to_string();

                                            // Extract the local name from the declarator
                                            if let Some(name_node) =
                                                node.child_by_field_name("name")
                                            {
                                                imports.push(ImportInfo {
                                                    local_name: self
                                                        .node_text(name_node, source)
                                                        .to_string(),
                                                    source_module: module,
                                                    original_name: None,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_imports(child, source, imports);
        }
    }

    /// Extract variable aliases from TypeScript/JavaScript assignments.
    ///
    /// Detects patterns like `const handler = authenticate` or `handler = authenticate`
    /// where the RHS is a simple identifier.
    fn extract_ts_aliases(&self, node: Node, source: &[u8], aliases: &mut Vec<(String, String)>) {
        match node.kind() {
            // const/let/var handler = authenticate
            "variable_declarator" => {
                let name_node = node.child_by_field_name("name");
                let value_node = node.child_by_field_name("value");
                if let (Some(name), Some(value)) = (name_node, value_node) {
                    if name.kind() == "identifier" && value.kind() == "identifier" {
                        let local = self.node_text(name, source).to_string();
                        let target = self.node_text(value, source).to_string();
                        aliases.push((local, target));
                    }
                    // handler = obj.method
                    if name.kind() == "identifier" && value.kind() == "member_expression" {
                        if let Some(prop) = value.child_by_field_name("property") {
                            let local = self.node_text(name, source).to_string();
                            let target = self.node_text(prop, source).to_string();
                            aliases.push((local, target));
                        }
                    }
                }
            }
            // handler = authenticate (reassignment)
            "assignment_expression" => {
                let left = node.child_by_field_name("left");
                let right = node.child_by_field_name("right");
                if let (Some(left_node), Some(right_node)) = (left, right) {
                    if left_node.kind() == "identifier" && right_node.kind() == "identifier" {
                        let local = self.node_text(left_node, source).to_string();
                        let target = self.node_text(right_node, source).to_string();
                        aliases.push((local, target));
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_aliases(child, source, aliases);
        }
    }

    // --- Python ---

    fn extract_py_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_py_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_py_metadata(node, source),
                    });
                }
            }
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_py_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_py_metadata(node, source),
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_py_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_symbols(child, source, symbols, parent);
        }
    }

    fn extract_py_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find(':') {
            // Check this is the colon before the body, not a type annotation
            let before_colon = &text[..pos];
            if before_colon.contains("def ") || before_colon.contains("class ") {
                return text[..=pos].trim().to_string();
            }
        }
        text.lines().next().unwrap_or("").trim().to_string()
    }

    fn extract_py_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = if kind == "function_definition" {
            node.child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string())
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let (callee, chain) = match func_node.kind() {
                    "identifier" => (self.node_text(func_node, source).to_string(), None),
                    "attribute" => {
                        let chain = self.build_py_chain(func_node, source);
                        let name = if let Some(attr) = func_node.child_by_field_name("attribute") {
                            self.node_text(attr, source).to_string()
                        } else {
                            self.node_text(func_node, source).to_string()
                        };
                        (name, chain)
                    }
                    _ => (self.node_text(func_node, source).to_string(), None),
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_calls(child, source, calls, scope);
        }
    }

    /// Build an expression chain from a Python `attribute` node.
    fn build_py_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_py_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    fn collect_py_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
        match node.kind() {
            "attribute" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_py_chain_steps(obj, source, steps);
                }
                if let Some(attr) = node.child_by_field_name("attribute") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(attr, source).to_string(),
                    ));
                }
            }
            "call" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_py_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "self" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    fn extract_py_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "import_statement" => {
                // import foo, import foo as bar
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "dotted_name" {
                        let name = self.node_text(child, source).to_string();
                        let local = name.split('.').next_back().unwrap_or(&name).to_string();
                        imports.push(ImportInfo {
                            local_name: local,
                            source_module: name,
                            original_name: None,
                        });
                    } else if child.kind() == "aliased_import" {
                        let name_node = child.child_by_field_name("name");
                        let alias_node = child.child_by_field_name("alias");
                        if let Some(name) = name_node {
                            let original = self.node_text(name, source).to_string();
                            let local = alias_node
                                .map(|a| self.node_text(a, source).to_string())
                                .unwrap_or_else(|| {
                                    original
                                        .split('.')
                                        .next_back()
                                        .unwrap_or(&original)
                                        .to_string()
                                });
                            imports.push(ImportInfo {
                                local_name: local,
                                source_module: original,
                                original_name: None,
                            });
                        }
                    }
                }
            }
            "import_from_statement" => {
                let module_node = node.child_by_field_name("module_name");
                let module = module_node
                    .map(|m| self.node_text(m, source).to_string())
                    .unwrap_or_default();

                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "dotted_name" if Some(child) != module_node => {
                            // Skip the module name itself (already captured)
                            let name = self.node_text(child, source).to_string();
                            imports.push(ImportInfo {
                                local_name: name.clone(),
                                source_module: module.clone(),
                                original_name: Some(name),
                            });
                        }
                        "aliased_import" => {
                            let name_node = child.child_by_field_name("name");
                            let alias_node = child.child_by_field_name("alias");
                            if let Some(name) = name_node {
                                let original = self.node_text(name, source).to_string();
                                let local = alias_node
                                    .map(|a| self.node_text(a, source).to_string())
                                    .unwrap_or_else(|| original.clone());
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: module.clone(),
                                    original_name: Some(original),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_imports(child, source, imports);
        }
    }

    /// Extract variable aliases from Python assignment statements.
    ///
    /// Detects patterns like `handler = authenticate` where the RHS is a simple
    /// identifier, producing an alias pair ("handler", "authenticate").
    fn extract_py_aliases(&self, node: Node, source: &[u8], aliases: &mut Vec<(String, String)>) {
        // Direct assignment node: `x = y`
        if node.kind() == "assignment" {
            self.try_extract_py_alias(node, source, aliases);
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_aliases(child, source, aliases);
        }
    }

    fn try_extract_py_alias(
        &self,
        assign: Node,
        source: &[u8],
        aliases: &mut Vec<(String, String)>,
    ) {
        let left = assign.child_by_field_name("left");
        let right = assign.child_by_field_name("right");

        if let (Some(left_node), Some(right_node)) = (left, right) {
            // identifier = identifier
            if left_node.kind() == "identifier" && right_node.kind() == "identifier" {
                let local = self.node_text(left_node, source).to_string();
                let target = self.node_text(right_node, source).to_string();
                aliases.push((local, target));
            }
            // identifier = module.attribute
            if left_node.kind() == "identifier" && right_node.kind() == "attribute" {
                if let Some(attr) = right_node.child_by_field_name("attribute") {
                    let local = self.node_text(left_node, source).to_string();
                    let target = self.node_text(attr, source).to_string();
                    aliases.push((local, target));
                }
            }
        }
    }

    // --- Go ---

    fn extract_go_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_go_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    // Extract receiver type as parent
                    let receiver_type = node.child_by_field_name("receiver").and_then(|recv| {
                        // receiver is a parameter_list; find the type inside
                        let cursor = &mut recv.walk();
                        for child in recv.children(cursor) {
                            if child.kind() == "parameter_declaration" {
                                if let Some(type_node) = child.child_by_field_name("type") {
                                    let type_text = self.node_text(type_node, source);
                                    // Strip pointer prefix
                                    let cleaned = type_text.trim_start_matches('*');
                                    return Some(cleaned.to_string());
                                }
                            }
                        }
                        None
                    });
                    let parent_name = receiver_type.or_else(|| parent.map(|s| s.to_string()));
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_go_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name,
                        metadata: None,
                    });
                }
            }
            "type_declaration" => {
                // type_declaration contains type_spec children
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "type_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            let type_node = child.child_by_field_name("type");
                            let sym_kind = match type_node.map(|t| t.kind()) {
                                Some("struct_type") => SymbolKind::Class,
                                Some("interface_type") => SymbolKind::Interface,
                                _ => SymbolKind::TypeAlias,
                            };
                            symbols.push(ParsedSymbol {
                                name,
                                kind: sym_kind,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self.extract_go_signature(child, source),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            "const_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "const_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(child, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            "var_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "var_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Variable,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(child, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_symbols(child, source, symbols, parent);
        }
    }

    fn extract_go_signature(&self, node: Node, source: &[u8]) -> String {
        let start = node.start_byte();
        let text = &source[start..];
        if let Some(pos) = text.iter().position(|&b| b == b'{') {
            let sig = std::str::from_utf8(&text[..pos]).unwrap_or("");
            sig.trim().to_string()
        } else {
            let first_line = self.node_text(node, source);
            first_line.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_go_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "method_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "selector_expression" => {
                            if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "go_statement" => {
                // go handleRequest(conn) — extract the call inside
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let callee = match func_node.kind() {
                                "identifier" => self.node_text(func_node, source).to_string(),
                                "selector_expression" => {
                                    if let Some(field) = func_node.child_by_field_name("field") {
                                        self.node_text(field, source).to_string()
                                    } else {
                                        self.node_text(func_node, source).to_string()
                                    }
                                }
                                _ => self.node_text(func_node, source).to_string(),
                            };
                            if let Some(caller) = scope {
                                calls.push(CallReference {
                                    caller_name: caller.to_string(),
                                    callee_name: callee,
                                    line: child.start_position().row as u32 + 1,
                                    chain: None,
                                    file: None,
                                });
                            }
                        }
                    }
                }
                return; // Don't recurse further (already handled the call)
            }
            "defer_statement" => {
                // defer file.Close() — extract the call inside
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let callee = match func_node.kind() {
                                "identifier" => self.node_text(func_node, source).to_string(),
                                "selector_expression" => {
                                    if let Some(field) = func_node.child_by_field_name("field") {
                                        self.node_text(field, source).to_string()
                                    } else {
                                        self.node_text(func_node, source).to_string()
                                    }
                                }
                                _ => self.node_text(func_node, source).to_string(),
                            };
                            if let Some(caller) = scope {
                                calls.push(CallReference {
                                    caller_name: caller.to_string(),
                                    callee_name: callee,
                                    line: child.start_position().row as u32 + 1,
                                    chain: None,
                                    file: None,
                                });
                            }
                        }
                    }
                }
                return; // Don't recurse further
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_calls(child, source, calls, scope);
        }
    }

    fn extract_go_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "import_declaration" {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "import_spec" {
                    self.extract_go_import_spec(child, source, imports);
                } else if child.kind() == "import_spec_list" {
                    let spec_cursor = &mut child.walk();
                    for spec in child.children(spec_cursor) {
                        if spec.kind() == "import_spec" {
                            self.extract_go_import_spec(spec, source, imports);
                        }
                    }
                }
            }
            return;
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_imports(child, source, imports);
        }
    }

    fn extract_go_import_spec(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        // import_spec can have: name (alias), path (the import path string)
        let path_node = node.child_by_field_name("path");
        let name_node = node.child_by_field_name("name");

        if let Some(path) = path_node {
            let module = self.node_text(path, source).trim_matches('"').to_string();

            // Derive the default local name from the last segment of the path
            let default_local = module.rsplit('/').next().unwrap_or(&module).to_string();

            let (local_name, original_name) = if let Some(alias) = name_node {
                let alias_text = self.node_text(alias, source).to_string();
                if alias_text == "." {
                    // Dot import: import . "fmt" — symbols available without qualifier
                    (default_local.clone(), Some(".".to_string()))
                } else {
                    (alias_text, Some(default_local))
                }
            } else {
                (default_local, None)
            };

            imports.push(ImportInfo {
                local_name,
                source_module: module,
                original_name,
            });
        }
    }

    // --- Rust ---

    fn extract_rust_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_item" | "function_signature_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "struct_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "enum_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "trait_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                    // Recurse into trait body for method signatures
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_rust_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "impl_item" => {
                // Extract the type name from the impl block
                let type_name = self.extract_rust_impl_type_name(node, source);
                if let Some(ref type_name) = type_name {
                    // Recurse into impl body with the type name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_rust_symbols(body, source, symbols, Some(type_name));
                        return;
                    }
                }
            }
            "mod_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "const_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Constant,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "static_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Variable,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "type_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "macro_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_symbols(child, source, symbols, parent);
        }
    }

    /// Extract the type name from an impl block.
    /// Handles both `impl Type` and `impl Trait for Type`.
    fn extract_rust_impl_type_name(&self, node: Node, source: &[u8]) -> Option<String> {
        if let Some(type_node) = node.child_by_field_name("type") {
            let text = self.node_text(type_node, source);
            // Strip generic parameters if present (e.g., `Server<T>` -> `Server`)
            let name = text.split('<').next().unwrap_or(text).trim();
            Some(name.to_string())
        } else {
            None
        }
    }

    fn extract_rust_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Rust, the signature is everything before the opening brace
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            // For items without a body (e.g., trait method signatures ending with `;`)
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_rust_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "function_item" {
            node.child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string())
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let (callee, chain) = match func_node.kind() {
                        "identifier" => (self.node_text(func_node, source).to_string(), None),
                        "scoped_identifier" => {
                            // e.g., `Vec::new()` or `std::io::stdin()`
                            let name = if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            };
                            (name, None)
                        }
                        "field_expression" => {
                            // e.g., `self.method()` via call_expression
                            let chain = self.build_rust_chain(func_node, source);
                            let name = if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            };
                            (name, chain)
                        }
                        "generic_function" => {
                            // e.g., `foo::<Type>()` — turbofish
                            if let Some(func_inner) = func_node.child_by_field_name("function") {
                                let (name, chain) = match func_inner.kind() {
                                    "identifier" => {
                                        (self.node_text(func_inner, source).to_string(), None)
                                    }
                                    "scoped_identifier" => {
                                        let n = if let Some(name) =
                                            func_inner.child_by_field_name("name")
                                        {
                                            self.node_text(name, source).to_string()
                                        } else {
                                            self.node_text(func_inner, source).to_string()
                                        };
                                        (n, None)
                                    }
                                    "field_expression" => {
                                        let c = self.build_rust_chain(func_inner, source);
                                        let n = if let Some(field) =
                                            func_inner.child_by_field_name("field")
                                        {
                                            self.node_text(field, source).to_string()
                                        } else {
                                            self.node_text(func_inner, source).to_string()
                                        };
                                        (n, c)
                                    }
                                    _ => (self.node_text(func_inner, source).to_string(), None),
                                };
                                (name, chain)
                            } else {
                                (self.node_text(func_node, source).to_string(), None)
                            }
                        }
                        _ => (self.node_text(func_node, source).to_string(), None),
                    };
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain,
                            file: None,
                        });
                    }
                }
            }
            "macro_invocation" => {
                // e.g., `println!("hello")`, `vec![1, 2, 3]`
                if let Some(macro_node) = node.child_by_field_name("macro") {
                    let macro_name = self.node_text(macro_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: macro_name,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_calls(child, source, calls, scope);
        }
    }

    /// Build an expression chain from a Rust `field_expression` node.
    fn build_rust_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_rust_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    fn collect_rust_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
        match node.kind() {
            "field_expression" => {
                if let Some(obj) = node.child_by_field_name("value") {
                    self.collect_rust_chain_steps(obj, source, steps);
                }
                if let Some(field) = node.child_by_field_name("field") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(field, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_rust_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "self" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            "self" => steps.push(ExpressionStep::This),
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    fn extract_rust_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "use_declaration" => {
                // The use_declaration has an "argument" field containing the use tree
                if let Some(arg) = node.child_by_field_name("argument") {
                    self.extract_rust_use_tree(arg, source, imports, "");
                }
                return; // Don't recurse further for use declarations
            }
            "extern_crate_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let alias = node
                        .child_by_field_name("alias")
                        .map(|a| self.node_text(a, source).to_string());
                    imports.push(ImportInfo {
                        local_name: alias.unwrap_or_else(|| name.clone()),
                        source_module: name,
                        original_name: None,
                    });
                }
                return;
            }
            "mod_item" => {
                // `mod foo;` (no body) is a file-level import
                let has_body = node.child_by_field_name("body").is_some();
                if !has_body {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = self.node_text(name_node, source).to_string();
                        imports.push(ImportInfo {
                            local_name: name.clone(),
                            source_module: name,
                            original_name: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_imports(child, source, imports);
        }
    }

    /// Recursively extract imports from a Rust use tree.
    /// `prefix` accumulates the path prefix (e.g., "std::io").
    fn extract_rust_use_tree(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
        prefix: &str,
    ) {
        match node.kind() {
            "scoped_use_list" => {
                // e.g., `std::{io, fs}` or `std::io::{Read, Write}`
                // The scoped_use_list has a "path" field and a "list" field
                let path_node = node.child_by_field_name("path");
                let list_node = node.child_by_field_name("list");

                let new_prefix = if let Some(path) = path_node {
                    let path_text = self.node_text(path, source);
                    if prefix.is_empty() {
                        path_text.to_string()
                    } else {
                        format!("{}::{}", prefix, path_text)
                    }
                } else {
                    prefix.to_string()
                };

                if let Some(list) = list_node {
                    // list is a use_list containing multiple items
                    let cursor = &mut list.walk();
                    for child in list.children(cursor) {
                        self.extract_rust_use_tree(child, source, imports, &new_prefix);
                    }
                }
            }
            "use_as_clause" => {
                // e.g., `Read as IoRead`
                let path_node = node.child_by_field_name("path");
                let alias_node = node.child_by_field_name("alias");
                if let Some(path) = path_node {
                    let path_text = self.node_text(path, source).to_string();
                    let full_path = if prefix.is_empty() {
                        path_text.clone()
                    } else {
                        format!("{}::{}", prefix, path_text)
                    };
                    let local = alias_node
                        .map(|a| self.node_text(a, source).to_string())
                        .unwrap_or_else(|| {
                            path_text
                                .split("::")
                                .last()
                                .unwrap_or(&path_text)
                                .to_string()
                        });
                    imports.push(ImportInfo {
                        local_name: local,
                        source_module: full_path,
                        original_name: Some(path_text),
                    });
                }
            }
            "scoped_identifier" => {
                // e.g., `std::collections::HashMap`
                let full_text = self.node_text(node, source).to_string();
                let full_path = if prefix.is_empty() {
                    full_text.clone()
                } else {
                    format!("{}::{}", prefix, full_text)
                };
                let local_name = full_text
                    .split("::")
                    .last()
                    .unwrap_or(&full_text)
                    .to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: full_path,
                    original_name: None,
                });
            }
            "identifier" => {
                let name = self.node_text(node, source).to_string();
                let full_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{}::{}", prefix, name)
                };
                imports.push(ImportInfo {
                    local_name: name,
                    source_module: full_path,
                    original_name: None,
                });
            }
            "self" => {
                // `use std::io::{self, Read}` — the `self` imports the parent module
                let local_name = prefix.split("::").last().unwrap_or(prefix).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: prefix.to_string(),
                    original_name: Some("self".to_string()),
                });
            }
            "use_wildcard" => {
                // `use std::io::*` — glob import
                imports.push(ImportInfo {
                    local_name: "*".to_string(),
                    source_module: if prefix.is_empty() {
                        "*".to_string()
                    } else {
                        format!("{}::*", prefix)
                    },
                    original_name: None,
                });
            }
            _ => {
                // For other node types (e.g., `crate`, `super`), treat as identifier
                let text = self.node_text(node, source).to_string();
                if !text.is_empty() && text != "use" && text != "::" && text != ";" {
                    let full_path = if prefix.is_empty() {
                        text.clone()
                    } else {
                        format!("{}::{}", prefix, text)
                    };
                    let local_name = text.split("::").last().unwrap_or(&text).to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: full_path,
                        original_name: None,
                    });
                }
            }
        }
    }

    // --- Java ---

    fn extract_java_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body with interface name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into enum body with enum name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "record_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into record body with record name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "annotation_type_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "constructor_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "field_declaration" => {
                // field_declaration has a "declarator" field containing variable_declarator(s)
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Variable,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            "constant_declaration" => {
                // static final fields sometimes parsed as constant_declaration
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_symbols(child, source, symbols, parent);
        }
    }

    fn extract_java_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_java_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method_declaration" | "constructor_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "method_invocation" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "object_creation_expression" => {
                // new ClassName(args) — extract the type name as callee
                if let Some(type_node) = node.child_by_field_name("type") {
                    let type_name = self.node_text(type_node, source).to_string();
                    // Strip generic parameters (e.g., `ArrayList<String>` -> `ArrayList`)
                    let clean_name = type_name
                        .split('<')
                        .next()
                        .unwrap_or(&type_name)
                        .trim()
                        .to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: clean_name,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "explicit_constructor_invocation" => {
                // super(...) or this(...) calls
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    let child_text = self.node_text(child, source);
                    if child_text == "super" || child_text == "this" {
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: child_text.to_string(),
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                        break;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_calls(child, source, calls, scope);
        }
    }

    fn extract_java_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "import_declaration" => {
                // Get the full import text to detect wildcards and static imports
                let full_text = self.node_text(node, source).to_string();
                let is_static = full_text.contains("static ");

                // Find the scoped_identifier or identifier child
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        let import_path = self.node_text(child, source).to_string();

                        // Check if this is a wildcard import (has asterisk sibling)
                        let is_wildcard = full_text.contains(".*");

                        if is_wildcard {
                            let module_path = import_path.clone();
                            imports.push(ImportInfo {
                                local_name: "*".to_string(),
                                source_module: format!("{}.*", module_path),
                                original_name: None,
                            });
                        } else {
                            // Regular import: `import com.example.MyClass;`
                            let local_name = import_path
                                .rsplit('.')
                                .next()
                                .unwrap_or(&import_path)
                                .to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: import_path,
                                original_name: if is_static {
                                    Some("static".to_string())
                                } else {
                                    None
                                },
                            });
                        }
                        break;
                    }
                }
                return;
            }
            "package_declaration" => {
                // Track the package as module info
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        let package_name = self.node_text(child, source).to_string();
                        imports.push(ImportInfo {
                            local_name: package_name.clone(),
                            source_module: package_name,
                            original_name: Some("package".to_string()),
                        });
                        break;
                    }
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_imports(child, source, imports);
        }
    }

    // --- C# ---

    fn extract_csharp_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" | "struct_declaration" | "record_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with parent set
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_csharp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body for method signatures
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_csharp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "constructor_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Variable,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "delegate_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_csharp_symbols(child, source, symbols, parent);
        }
    }

    fn extract_csharp_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For C#, the signature is everything before the opening brace
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            // For items without a body (e.g., interface method signatures ending with `;`)
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_csharp_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "method_declaration" | "constructor_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "invocation_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "member_access_expression" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "member_binding_expression" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "object_creation_expression" => {
                // new Foo(...) — extract the type name as callee
                if let Some(type_node) = node.child_by_field_name("type") {
                    let callee = self.node_text(type_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_csharp_calls(child, source, calls, scope);
        }
    }

    fn extract_csharp_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "using_directive" {
            // using System.Collections.Generic;
            // using Alias = Some.Namespace;
            let cursor = &mut node.walk();
            let mut module = String::new();
            let mut alias: Option<String> = None;

            for child in node.children(cursor) {
                match child.kind() {
                    "name_equals" => {
                        // This is the alias part: `using Alias = ...`
                        if let Some(name_node) = child.child_by_field_name("name") {
                            alias = Some(self.node_text(name_node, source).to_string());
                        }
                    }
                    "qualified_name" | "identifier" | "alias_qualified_name" => {
                        module = self.node_text(child, source).to_string();
                    }
                    _ => {}
                }
            }

            if !module.is_empty() {
                let local_name = if let Some(ref a) = alias {
                    a.clone()
                } else {
                    // Derive local name from the last segment of the qualified name
                    module.rsplit('.').next().unwrap_or(&module).to_string()
                };
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: alias,
                });
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_csharp_imports(child, source, imports);
        }
    }

    // --- C ---

    fn extract_c_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = self.extract_c_declarator_name(declarator, source) {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Function,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "declaration" => {
                // Check if this is a function forward declaration (has function_declarator)
                // or a variable/constant declaration
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if self.c_has_function_declarator(declarator) {
                        if let Some(name) = self.extract_c_declarator_name(declarator, source) {
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_c_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    // Only emit if this has a body (definition, not just usage)
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "union_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Enum,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "type_definition" => {
                // typedef ... name;
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_c_typedef_name(declarator, source);
                    if let Some(name) = name {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "preproc_def" => {
                // #define NAME value
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Constant,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "preproc_function_def" => {
                // #define NAME(args) body
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Constant,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_symbols(child, source, symbols, parent);
        }
    }

    /// Extract the function name from a C declarator, handling pointer declarators
    /// and function declarators (e.g., `int *foo(int x)` or `void bar(void)`).
    fn extract_c_declarator_name(&self, node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_declarator_name(declarator, source)
                } else {
                    None
                }
            }
            "pointer_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_declarator_name(declarator, source)
                } else {
                    None
                }
            }
            "parenthesized_declarator" => {
                // (*func_ptr)(args) — recurse inside parens
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if let Some(name) = self.extract_c_declarator_name(child, source) {
                        return Some(name);
                    }
                }
                None
            }
            "identifier" => Some(self.node_text(node, source).to_string()),
            _ => None,
        }
    }

    /// Check if a declarator contains a function_declarator node.
    fn c_has_function_declarator(&self, node: Node) -> bool {
        if node.kind() == "function_declarator" {
            return true;
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            if self.c_has_function_declarator(child) {
                return true;
            }
        }
        false
    }

    /// Extract the typedef name from a declarator node.
    fn extract_c_typedef_name(&self, node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "type_identifier" | "identifier" => Some(self.node_text(node, source).to_string()),
            "pointer_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_typedef_name(declarator, source)
                } else {
                    None
                }
            }
            "function_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_typedef_name(declarator, source)
                } else {
                    None
                }
            }
            "parenthesized_declarator" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if let Some(name) = self.extract_c_typedef_name(child, source) {
                        return Some(name);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn extract_c_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_c_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "function_definition" {
            node.child_by_field_name("declarator")
                .and_then(|d| self.extract_c_declarator_name(d, source))
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let callee = match func_node.kind() {
                    "identifier" => self.node_text(func_node, source).to_string(),
                    "field_expression" => {
                        if let Some(field) = func_node.child_by_field_name("field") {
                            self.node_text(field, source).to_string()
                        } else {
                            self.node_text(func_node, source).to_string()
                        }
                    }
                    "parenthesized_expression" => {
                        // Function pointer calls: (*func_ptr)(args)
                        self.node_text(func_node, source).to_string()
                    }
                    _ => self.node_text(func_node, source).to_string(),
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_calls(child, source, calls, scope);
        }
    }

    fn extract_c_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "preproc_include" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path_text = self.node_text(path_node, source).to_string();
                // Strip angle brackets or quotes
                let module = path_text
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim_matches('"')
                    .to_string();
                // Derive local name from filename without extension
                let local_name = module
                    .rsplit('/')
                    .next()
                    .unwrap_or(&module)
                    .split('.')
                    .next()
                    .unwrap_or(&module)
                    .to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: None,
                });
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_imports(child, source, imports);
        }
    }

    // --- C++ ---

    fn extract_cpp_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                // Free functions or method definitions outside class body
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
                        let sym_kind = if parent.is_some() {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        symbols.push(ParsedSymbol {
                            name,
                            kind: sym_kind,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "declaration" if self.cpp_declaration_is_function(node, source) => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
                        let sym_kind = if parent.is_some() {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        symbols.push(ParsedSymbol {
                            name,
                            kind: sym_kind,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "class_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_cpp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into struct body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_cpp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "template_declaration" => {
                // Extract the inner declaration (class, function, etc.) with template info
                let template_sig = self.extract_cpp_template_prefix(node, source);
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "function_definition" => {
                            if let Some(declarator) = child.child_by_field_name("declarator") {
                                let name = self.extract_cpp_declarator_name(declarator, source);
                                if !name.is_empty() {
                                    let sym_kind = if parent.is_some() {
                                        SymbolKind::Method
                                    } else {
                                        SymbolKind::Function
                                    };
                                    symbols.push(ParsedSymbol {
                                        name,
                                        kind: sym_kind,
                                        start_line: node.start_position().row as u32 + 1,
                                        end_line: node.end_position().row as u32 + 1,
                                        signature: format!(
                                            "{} {}",
                                            template_sig,
                                            self.extract_cpp_signature(child, source)
                                        ),
                                        content: self.node_text(node, source).to_string(),
                                        parent_name: parent.map(|s| s.to_string()),
                                        metadata: None,
                                    });
                                }
                            }
                        }
                        "declaration" if self.cpp_declaration_is_function(child, source) => {
                            if let Some(declarator) = child.child_by_field_name("declarator") {
                                let name = self.extract_cpp_declarator_name(declarator, source);
                                if !name.is_empty() {
                                    let sym_kind = if parent.is_some() {
                                        SymbolKind::Method
                                    } else {
                                        SymbolKind::Function
                                    };
                                    symbols.push(ParsedSymbol {
                                        name,
                                        kind: sym_kind,
                                        start_line: node.start_position().row as u32 + 1,
                                        end_line: node.end_position().row as u32 + 1,
                                        signature: format!(
                                            "{} {}",
                                            template_sig,
                                            self.extract_cpp_signature(child, source)
                                        ),
                                        content: self.node_text(node, source).to_string(),
                                        parent_name: parent.map(|s| s.to_string()),
                                        metadata: None,
                                    });
                                }
                            }
                        }
                        "class_specifier" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                let name = self.node_text(name_node, source).to_string();
                                symbols.push(ParsedSymbol {
                                    name: name.clone(),
                                    kind: SymbolKind::Class,
                                    start_line: node.start_position().row as u32 + 1,
                                    end_line: node.end_position().row as u32 + 1,
                                    signature: format!(
                                        "{} {}",
                                        template_sig,
                                        self.extract_cpp_signature(child, source)
                                    ),
                                    content: self.node_text(node, source).to_string(),
                                    parent_name: parent.map(|s| s.to_string()),
                                    metadata: None,
                                });
                                if let Some(body) = child.child_by_field_name("body") {
                                    self.extract_cpp_symbols(body, source, symbols, Some(&name));
                                }
                            }
                        }
                        "struct_specifier" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                let name = self.node_text(name_node, source).to_string();
                                symbols.push(ParsedSymbol {
                                    name: name.clone(),
                                    kind: SymbolKind::Class,
                                    start_line: node.start_position().row as u32 + 1,
                                    end_line: node.end_position().row as u32 + 1,
                                    signature: format!(
                                        "{} {}",
                                        template_sig,
                                        self.extract_cpp_signature(child, source)
                                    ),
                                    content: self.node_text(node, source).to_string(),
                                    parent_name: parent.map(|s| s.to_string()),
                                    metadata: None,
                                });
                                if let Some(body) = child.child_by_field_name("body") {
                                    self.extract_cpp_symbols(body, source, symbols, Some(&name));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                return; // Already handled children
            }
            "type_definition" => {
                // typedef ... name;
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "alias_declaration" => {
                // using Name = Type;
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_cpp_symbols(child, source, symbols, parent);
        }
    }

    fn extract_cpp_declarator_name(&self, node: Node, source: &[u8]) -> String {
        match node.kind() {
            "function_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.extract_cpp_declarator_name(decl, source)
                } else {
                    String::new()
                }
            }
            "qualified_identifier" | "scoped_identifier" => {
                // e.g., ClassName::methodName — extract just the last name
                if let Some(name) = node.child_by_field_name("name") {
                    self.node_text(name, source).to_string()
                } else {
                    self.node_text(node, source).to_string()
                }
            }
            "identifier" | "type_identifier" | "field_identifier" => {
                self.node_text(node, source).to_string()
            }
            "destructor_name" => {
                // ~ClassName
                self.node_text(node, source).to_string()
            }
            "pointer_declarator" | "reference_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.extract_cpp_declarator_name(decl, source)
                } else {
                    String::new()
                }
            }
            "parenthesized_declarator" => {
                // (*funcPtr)(args)
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    let name = self.extract_cpp_declarator_name(child, source);
                    if !name.is_empty() {
                        return name;
                    }
                }
                String::new()
            }
            _ => String::new(),
        }
    }

    fn cpp_declaration_is_function(&self, node: Node, _source: &[u8]) -> bool {
        if let Some(declarator) = node.child_by_field_name("declarator") {
            self.cpp_is_function_declarator(declarator)
        } else {
            false
        }
    }

    fn cpp_is_function_declarator(&self, node: Node) -> bool {
        match node.kind() {
            "function_declarator" => true,
            "pointer_declarator" | "reference_declarator" | "parenthesized_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.cpp_is_function_declarator(decl)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn extract_cpp_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_cpp_template_prefix(&self, node: Node, source: &[u8]) -> String {
        // Extract "template<...>" part
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('>') {
            text[..=pos].trim().to_string()
        } else {
            "template<>".to_string()
        }
    }

    fn extract_cpp_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_definition" => node
                .child_by_field_name("declarator")
                .map(|d| self.extract_cpp_declarator_name(d, source))
                .filter(|n| !n.is_empty()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "field_expression" => {
                            if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "qualified_identifier" | "scoped_identifier" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "template_function" => {
                            // e.g., make_shared<Foo>(...)
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "new_expression" => {
                // new Foo(...) — extract the type name
                if let Some(type_node) = node.child_by_field_name("type") {
                    let callee = self.node_text(type_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_cpp_calls(child, source, calls, scope);
        }
    }

    fn extract_cpp_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "preproc_include" => {
                // #include <header> or #include "header"
                if let Some(path_node) = node.child_by_field_name("path") {
                    let path_text = self.node_text(path_node, source);
                    // Strip quotes/angle brackets
                    let module = path_text
                        .trim_start_matches(['<', '"'])
                        .trim_end_matches(['>', '"'])
                        .to_string();
                    let local_name = module
                        .rsplit('/')
                        .next()
                        .unwrap_or(&module)
                        .trim_end_matches(".h")
                        .trim_end_matches(".hpp")
                        .trim_end_matches(".hxx")
                        .to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: module,
                        original_name: None,
                    });
                }
            }
            "using_declaration" => {
                // using namespace std; or using std::vector;
                let text = self.node_text(node, source).trim().to_string();
                let module = text
                    .trim_start_matches("using")
                    .trim_start_matches("namespace")
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string();
                if !module.is_empty() {
                    let local_name = module.rsplit("::").next().unwrap_or(&module).to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: module,
                        original_name: None,
                    });
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_cpp_imports(child, source, imports);
        }
    }

    // --- Ruby ---

    fn extract_ruby_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ruby_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "module" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into module body with module name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ruby_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "method" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "singleton_method" => {
                // def self.foo — class/static methods
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "assignment" => {
                // Top-level constant assignments like `MAX_SIZE = 100`
                if parent.is_none() {
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        if child.kind() == "constant" {
                            let name = self.node_text(child, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: None,
                                metadata: None,
                            });
                            break;
                        }
                    }
                }
                return; // Don't recurse further
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ruby_symbols(child, source, symbols, parent);
        }
    }

    fn extract_ruby_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // Ruby uses `end` keyword, so signature is the first line
        text.lines().next().unwrap_or("").trim().to_string()
    }

    fn extract_ruby_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method" | "singleton_method" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call" => {
                // obj.method or method(args)
                if let Some(method_node) = node.child_by_field_name("method") {
                    let callee = self.node_text(method_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "method_call" => {
                // method_call is for bare method calls like `puts "hello"`
                if let Some(method_node) = node.child_by_field_name("method") {
                    let callee = self.node_text(method_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "super" => {
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: "super".to_string(),
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
            "yield" => {
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: "yield".to_string(),
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ruby_calls(child, source, calls, scope);
        }
    }

    fn extract_ruby_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        // Ruby imports: require, require_relative, include, extend, prepend
        if node.kind() == "call" || node.kind() == "method_call" {
            let method_node = node.child_by_field_name("method");
            if let Some(method) = method_node {
                let method_name = self.node_text(method, source);
                match method_name {
                    "require" | "require_relative" => {
                        // Extract the argument (string)
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let cursor = &mut args.walk();
                            for arg in args.children(cursor) {
                                if arg.kind() == "string" || arg.kind() == "string_content" {
                                    let module = self.node_text(arg, source);
                                    let module =
                                        module.trim_matches('\'').trim_matches('"').to_string();
                                    if !module.is_empty() {
                                        let local_name = module
                                            .rsplit('/')
                                            .next()
                                            .unwrap_or(&module)
                                            .to_string();
                                        imports.push(ImportInfo {
                                            local_name,
                                            source_module: module,
                                            original_name: Some(method_name.to_string()),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "include" | "extend" | "prepend" => {
                        // Mixin: include ModuleName
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let cursor = &mut args.walk();
                            for arg in args.children(cursor) {
                                if arg.kind() == "constant" || arg.kind() == "scope_resolution" {
                                    let module_name = self.node_text(arg, source).to_string();
                                    imports.push(ImportInfo {
                                        local_name: module_name.clone(),
                                        source_module: module_name,
                                        original_name: Some(method_name.to_string()),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ruby_imports(child, source, imports);
        }
    }

    // --- Kotlin ---

    /// Helper to find the first child with a given kind
    fn find_child_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let result = node.children(&mut cursor).find(|c| c.kind() == kind);
        result
    }

    /// Check if a class_declaration is actually an interface
    fn kotlin_is_interface(&self, node: Node) -> bool {
        let mut cursor = node.walk();
        let result = node.children(&mut cursor).any(|c| c.kind() == "interface");
        result
    }

    fn extract_kotlin_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                // In tree-sitter-kotlin, both classes and interfaces use class_declaration.
                // Interfaces have an "interface" keyword child instead of "class".
                let is_interface = self.kotlin_is_interface(node);

                // The name is a direct `identifier` child (not field-based)
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if is_interface {
                        SymbolKind::Interface
                    } else {
                        SymbolKind::Class
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class_body with class/interface name as parent
                    if let Some(body) = self.find_child_by_kind(node, "class_body") {
                        self.extract_kotlin_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "object_declaration" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class_body with object name as parent
                    if let Some(body) = self.find_child_by_kind(node, "class_body") {
                        self.extract_kotlin_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "companion_object" => {
                // companion object { ... } — use the parent class name or "Companion"
                let companion_name = self
                    .find_child_by_kind(node, "identifier")
                    .map(|n| self.node_text(n, source).to_string())
                    .unwrap_or_else(|| "Companion".to_string());
                let effective_parent = parent.unwrap_or(&companion_name);
                // Recurse into class_body with parent's name so methods appear under the class
                if let Some(body) = self.find_child_by_kind(node, "class_body") {
                    self.extract_kotlin_symbols(body, source, symbols, Some(effective_parent));
                    return;
                }
            }
            "function_declaration" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // val/var declarations — the name is in a variable_declaration child
                if let Some(var_decl) = self.find_child_by_kind(node, "variable_declaration") {
                    if let Some(name_node) = self.find_child_by_kind(var_decl, "identifier") {
                        let name = self.node_text(name_node, source).to_string();
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Variable,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self
                                .node_text(node, source)
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_string(),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
                return; // Don't recurse further
            }
            "enum_entry" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_kotlin_symbols(child, source, symbols, parent);
        }
    }

    fn extract_kotlin_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_kotlin_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_declaration" => self
                .find_child_by_kind(node, "identifier")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(first_child) = node.child(0) {
                let callee = match first_child.kind() {
                    "simple_identifier" | "identifier" => {
                        self.node_text(first_child, source).to_string()
                    }
                    "navigation_expression" => {
                        // obj.method() — extract the last simple_identifier
                        let nav_cursor = &mut first_child.walk();
                        let mut last_name = self.node_text(first_child, source).to_string();
                        for nav_child in first_child.children(nav_cursor) {
                            if nav_child.kind() == "simple_identifier" {
                                last_name = self.node_text(nav_child, source).to_string();
                            }
                        }
                        last_name
                    }
                    _ => self.node_text(first_child, source).to_string(),
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_kotlin_calls(child, source, calls, scope);
        }
    }

    fn extract_kotlin_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "import" => {
                // import kotlinx.coroutines.flow.Flow
                // import java.util.UUID as JavaUUID
                let cursor = &mut node.walk();
                let mut import_path = String::new();
                let mut alias: Option<String> = None;

                for child in node.children(cursor) {
                    if child.kind() == "qualified_identifier" {
                        import_path = self.node_text(child, source).to_string();
                    }
                    // `as` keyword followed by an identifier for aliased imports
                    if child.kind() == "identifier" {
                        // This is the alias after `as`
                        alias = Some(self.node_text(child, source).to_string());
                    }
                }

                if !import_path.is_empty() {
                    let is_wildcard =
                        import_path.ends_with(".*") || self.node_text(node, source).contains(".*");
                    if is_wildcard {
                        imports.push(ImportInfo {
                            local_name: "*".to_string(),
                            source_module: import_path,
                            original_name: None,
                        });
                    } else {
                        let local_name = if let Some(ref a) = alias {
                            a.clone()
                        } else {
                            import_path
                                .rsplit('.')
                                .next()
                                .unwrap_or(&import_path)
                                .to_string()
                        };
                        imports.push(ImportInfo {
                            local_name,
                            source_module: import_path,
                            original_name: alias,
                        });
                    }
                }
                return;
            }
            "package_header" => {
                // package com.example.mypackage
                if let Some(qi) = self.find_child_by_kind(node, "qualified_identifier") {
                    let package_name = self.node_text(qi, source).to_string();
                    imports.push(ImportInfo {
                        local_name: package_name.clone(),
                        source_module: package_name,
                        original_name: Some("package".to_string()),
                    });
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_kotlin_imports(child, source, imports);
        }
    }

    // --- Swift ---

    /// Determine the Swift declaration kind from the `declaration_kind` field.
    /// In tree-sitter-swift, class/struct/enum/extension all use `class_declaration`
    /// with different `declaration_kind` values.
    fn swift_declaration_kind<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
        node.child_by_field_name("declaration_kind")
            .map(move |dk| self.node_text(dk, source))
    }

    fn extract_swift_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                // tree-sitter-swift uses class_declaration for class, struct, enum, and extension
                let decl_kind = self.swift_declaration_kind(node, source).unwrap_or("class");
                match decl_kind {
                    "class" | "struct" => {
                        if let Some(name_node) = node.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Class,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_swift_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            if let Some(body) = node.child_by_field_name("body") {
                                self.extract_swift_symbols(body, source, symbols, Some(&name));
                                return;
                            }
                        }
                    }
                    "enum" => {
                        if let Some(name_node) = node.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Enum,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_swift_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            if let Some(body) = node.child_by_field_name("body") {
                                self.extract_swift_symbols(body, source, symbols, Some(&name));
                                return;
                            }
                        }
                    }
                    "extension" => {
                        // extension TypeName { ... } — extract the type being extended
                        // name field contains the user_type being extended
                        let ext_name = node
                            .child_by_field_name("name")
                            .map(|n| self.node_text(n, source).to_string());
                        let effective_parent = ext_name.as_deref().or(parent);
                        if let Some(body) = node.child_by_field_name("body") {
                            self.extract_swift_symbols(body, source, symbols, effective_parent);
                            return;
                        }
                    }
                    _ => {}
                }
            }
            "protocol_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_swift_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "protocol_function_declaration" => {
                // Protocol method declarations (no body)
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // var/let declarations — name is in a "pattern" child with field name "name"
                if let Some(pattern_node) = node.child_by_field_name("name") {
                    // pattern_node is a `pattern` node containing a `simple_identifier`
                    let name = self.node_text(pattern_node, source).to_string();
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Variable,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self
                                .node_text(node, source)
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_string(),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
                return; // Don't recurse further
            }
            "typealias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_swift_symbols(child, source, symbols, parent);
        }
    }

    fn extract_swift_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_swift_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            // In tree-sitter-swift, call_expression has the callee as first child
            // and call_suffix as second child
            if let Some(first_child) = node.child(0) {
                let callee = match first_child.kind() {
                    "simple_identifier" => self.node_text(first_child, source).to_string(),
                    "navigation_expression" => {
                        // obj.method() — extract the suffix's simple_identifier
                        if let Some(suffix) = first_child.child_by_field_name("suffix") {
                            // navigation_suffix contains a simple_identifier with field "suffix"
                            if let Some(id) = suffix.child_by_field_name("suffix") {
                                self.node_text(id, source).to_string()
                            } else {
                                // Fallback: last simple_identifier in the navigation expression
                                let nav_cursor = &mut first_child.walk();
                                let mut last_name = self.node_text(first_child, source).to_string();
                                for nav_child in first_child.children(nav_cursor) {
                                    if nav_child.kind() == "simple_identifier" {
                                        last_name = self.node_text(nav_child, source).to_string();
                                    }
                                }
                                last_name
                            }
                        } else {
                            self.node_text(first_child, source).to_string()
                        }
                    }
                    _ => self.node_text(first_child, source).to_string(),
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_swift_calls(child, source, calls, scope);
        }
    }

    fn extract_swift_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "import_declaration" {
            // import Foundation
            // import UIKit.UIViewController
            // The import_declaration has an `identifier` child with the module path
            let cursor = &mut node.walk();
            let mut module = String::new();
            for child in node.children(cursor) {
                if child.kind() == "identifier" {
                    module = self.node_text(child, source).to_string();
                }
            }
            if !module.is_empty() {
                let local_name = module.rsplit('.').next().unwrap_or(&module).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: None,
                });
            }
            return;
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_swift_imports(child, source, imports);
        }
    }

    // --- PHP ---

    fn extract_php_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "trait_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // Extract property names from property elements
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "property_element" {
                        let inner_cursor = &mut child.walk();
                        for inner in child.children(inner_cursor) {
                            if inner.kind() == "variable_name" {
                                let name = self.node_text(inner, source).to_string();
                                // Strip leading $ from PHP variable names
                                let name = name.trim_start_matches('$').to_string();
                                symbols.push(ParsedSymbol {
                                    name,
                                    kind: SymbolKind::Variable,
                                    start_line: node.start_position().row as u32 + 1,
                                    end_line: node.end_position().row as u32 + 1,
                                    signature: self
                                        .node_text(node, source)
                                        .lines()
                                        .next()
                                        .unwrap_or("")
                                        .trim()
                                        .to_string(),
                                    content: self.node_text(node, source).to_string(),
                                    parent_name: parent.map(|s| s.to_string()),
                                    metadata: None,
                                });
                            }
                        }
                    }
                }
                return; // Don't recurse further
            }
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into namespace body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "const_declaration" => {
                // const FOO = 1; or class const FOO = 1;
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "const_element" {
                        // The name is a child node with kind "name"
                        if let Some(name_node) = self.find_child_by_kind(child, "name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_symbols(child, source, symbols, parent);
        }
    }

    fn extract_php_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_php_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "method_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "function_call_expression" => {
                // Regular function calls: foo(), \Namespace\foo()
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "name" | "qualified_name" => self.node_text(func_node, source).to_string(),
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    // Extract just the function name from qualified names
                    let callee = callee.rsplit('\\').next().unwrap_or(&callee).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "member_call_expression" => {
                // $obj->method()
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "scoped_call_expression" => {
                // ClassName::staticMethod()
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "object_creation_expression" => {
                // new Foo()
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "name" || child.kind() == "qualified_name" {
                        let class_name = self.node_text(child, source).to_string();
                        let class_name = class_name
                            .rsplit('\\')
                            .next()
                            .unwrap_or(&class_name)
                            .to_string();
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: class_name,
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                        break;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_calls(child, source, calls, scope);
        }
    }

    fn extract_php_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "namespace_use_declaration" => {
                // use App\Models\User;
                // use App\Models\User as UserModel;
                // use App\Models\{User, Post};
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "namespace_use_clause" {
                        let path = self.node_text(child, source).to_string();
                        // Check for alias (as)
                        let (source_module, local_name) =
                            if let Some(alias_node) = child.child_by_field_name("alias") {
                                let alias = self.node_text(alias_node, source).to_string();
                                // Get the qualified name (everything before " as ")
                                if let Some(name_node) = child.child(0) {
                                    let module = self.node_text(name_node, source).to_string();
                                    (module, alias)
                                } else {
                                    (path.clone(), path)
                                }
                            } else {
                                let local = path.rsplit('\\').next().unwrap_or(&path).to_string();
                                (path, local)
                            };
                        imports.push(ImportInfo {
                            local_name,
                            source_module,
                            original_name: None,
                        });
                    }
                    if child.kind() == "namespace_use_group" {
                        // Grouped use: use App\Models\{User, Post};
                        // Find prefix from the namespace_use_declaration
                        let prefix = node
                            .child(1)
                            .map(|n| self.node_text(n, source).to_string())
                            .unwrap_or_default();
                        let group_cursor = &mut child.walk();
                        for clause in child.children(group_cursor) {
                            if clause.kind() == "namespace_use_clause" {
                                let name = self.node_text(clause, source).to_string();
                                let full_path = if prefix.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{}\\{}", prefix, name)
                                };
                                let local = name.rsplit('\\').next().unwrap_or(&name).to_string();
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: full_path,
                                    original_name: None,
                                });
                            }
                        }
                    }
                }
                return;
            }
            "namespace_definition" => {
                // namespace App\Models;
                if let Some(name_node) = node.child_by_field_name("name") {
                    let ns_name = self.node_text(name_node, source).to_string();
                    imports.push(ImportInfo {
                        local_name: ns_name.clone(),
                        source_module: ns_name,
                        original_name: Some("namespace".to_string()),
                    });
                }
            }
            "expression_statement" => {
                // require/include/require_once/include_once calls
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "function_call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let func_name = self.node_text(func_node, source);
                            if matches!(
                                func_name,
                                "require" | "include" | "require_once" | "include_once"
                            ) {
                                if let Some(args) = child.child_by_field_name("arguments") {
                                    let arg_text = self.node_text(args, source);
                                    let module = arg_text
                                        .trim_matches(|c| c == '(' || c == ')')
                                        .trim()
                                        .trim_matches('\'')
                                        .trim_matches('"')
                                        .to_string();
                                    if !module.is_empty() {
                                        let local_name = module
                                            .rsplit('/')
                                            .next()
                                            .unwrap_or(&module)
                                            .to_string();
                                        imports.push(ImportInfo {
                                            local_name,
                                            source_module: module,
                                            original_name: Some(func_name.to_string()),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    // require_expression, include_expression etc. are keyword-based
                    if matches!(
                        child.kind(),
                        "require_expression"
                            | "include_expression"
                            | "require_once_expression"
                            | "include_once_expression"
                    ) {
                        // The second child is typically the path expression
                        if let Some(path_node) = child.child(1) {
                            let module = self
                                .node_text(path_node, source)
                                .trim_matches('\'')
                                .trim_matches('"')
                                .to_string();
                            if !module.is_empty() {
                                let kind_name = child.kind().replace("_expression", "");
                                let local_name =
                                    module.rsplit('/').next().unwrap_or(&module).to_string();
                                imports.push(ImportInfo {
                                    local_name,
                                    source_module: module,
                                    original_name: Some(kind_name),
                                });
                            }
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_imports(child, source, imports);
        }
    }

    // ─── Lua ───────────────────────────────────────────────────────────

    fn extract_lua_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if name.contains(':') {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "local_function" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_definition_statement" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if name.contains(':') {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "local_variable_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator"
                        || child.kind() == "identifier"
                        || child.kind() == "name"
                    {
                        let name = self.node_text(child, source).to_string();
                        if !name.is_empty() {
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Variable,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            "assignment_statement" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_list" || child.kind() == "variable" {
                        let name = self.node_text(child, source).to_string();
                        if !name.is_empty() {
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Variable,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(node, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_lua_symbols(child, source, symbols, parent);
        }
    }

    fn extract_lua_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('\n') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_lua_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "function_declaration" | "local_function" | "function_definition_statement" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "function_call" {
            if let Some(name_node) = node.child_by_field_name("name") {
                let callee = self.node_text(name_node, source).to_string();
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            } else if let Some(method_node) = node.child_by_field_name("method") {
                let callee = self.node_text(method_node, source).to_string();
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            } else if let Some(first_child) = node.child(0) {
                let callee = self.node_text(first_child, source).to_string();
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_lua_calls(child, source, calls, scope);
        }
    }

    fn extract_lua_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "function_call" {
            let func_text = if let Some(name_node) = node.child_by_field_name("name") {
                Some(self.node_text(name_node, source).to_string())
            } else {
                node.child(0)
                    .map(|first_child| self.node_text(first_child, source).to_string())
            };

            if let Some(func_name) = func_text {
                if func_name == "require" {
                    if let Some(args) = node.child_by_field_name("arguments") {
                        let arg_text = self.node_text(args, source);
                        let module = arg_text
                            .trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
                            .trim_matches('\'')
                            .trim_matches('"')
                            .to_string();
                        if !module.is_empty() {
                            let local_name =
                                module.rsplit('.').next().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: Some("require".to_string()),
                            });
                        }
                    } else {
                        let cursor = &mut node.walk();
                        for child in node.children(cursor) {
                            if child.kind() == "string" || child.kind() == "string_literal" {
                                let module = self
                                    .node_text(child, source)
                                    .trim_matches('\'')
                                    .trim_matches('"')
                                    .to_string();
                                if !module.is_empty() {
                                    let local_name =
                                        module.rsplit('.').next().unwrap_or(&module).to_string();
                                    imports.push(ImportInfo {
                                        local_name,
                                        source_module: module,
                                        original_name: Some("require".to_string()),
                                    });
                                }
                                break;
                            }
                        }
                    }
                    return;
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_lua_imports(child, source, imports);
        }
    }

    // ─── Zig ───────────────────────────────────────────────────────────

    fn extract_zig_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "FnProto" | "fn_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_zig_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "ContainerDecl" | "struct_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = {
                        let text = self.node_text(node, source);
                        if text.starts_with("enum") {
                            SymbolKind::Enum
                        } else {
                            SymbolKind::Class
                        }
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_zig_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_zig_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "VarDecl" | "variable_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let text = self.node_text(node, source);
                    let sym_kind = if text.trim_start().starts_with("const") {
                        let cursor = &mut node.walk();
                        let mut is_container = false;
                        for child in node.children(cursor) {
                            if matches!(child.kind(), "ContainerDecl" | "struct_declaration") {
                                is_container = true;
                                break;
                            }
                        }
                        if is_container {
                            SymbolKind::Class
                        } else {
                            SymbolKind::Constant
                        }
                    } else {
                        SymbolKind::Variable
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_zig_symbols(child, source, symbols, parent);
        }
    }

    fn extract_zig_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_zig_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "FnProto" | "fn_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" | "FnCallExpr" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = self.node_text(func_node, source).to_string();
                    let callee_short = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee_short,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(first_child) = node.child(0) {
                    let callee = self.node_text(first_child, source).to_string();
                    let callee_short = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee_short,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "builtin_call_expr" | "BuiltinCallExpr" => {
                let text = self.node_text(node, source);
                if let Some(paren_pos) = text.find('(') {
                    let builtin_name = text[..paren_pos].trim().to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: builtin_name,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_zig_calls(child, source, calls, scope);
        }
    }

    fn extract_zig_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "VarDecl" | "variable_declaration" => {
                let text = self.node_text(node, source);
                if text.contains("@import") {
                    let local_name = if let Some(name_node) = node.child_by_field_name("name") {
                        self.node_text(name_node, source).to_string()
                    } else {
                        String::new()
                    };

                    if let Some(start) = text.find("@import(") {
                        let after_import = &text[start + 8..];
                        let module = after_import
                            .trim_start_matches(['"', '\''])
                            .split(['"', '\''])
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !module.is_empty() && !local_name.is_empty() {
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: Some("@import".to_string()),
                            });
                        }
                    }
                    return;
                }
                if text.contains("@cImport") {
                    let local_name = if let Some(name_node) = node.child_by_field_name("name") {
                        self.node_text(name_node, source).to_string()
                    } else {
                        String::new()
                    };

                    let mut search = text;
                    while let Some(start) = search.find("@cInclude(") {
                        let after = &search[start + 10..];
                        let header = after
                            .trim_start_matches(['"', '\''])
                            .split(['"', '\''])
                            .next()
                            .unwrap_or("");
                        if !header.is_empty() {
                            imports.push(ImportInfo {
                                local_name: local_name.clone(),
                                source_module: header.to_string(),
                                original_name: Some("@cImport".to_string()),
                            });
                        }
                        search = after;
                    }
                    return;
                }
            }
            "builtin_call_expr" | "BuiltinCallExpr" => {
                let text = self.node_text(node, source);
                if text.starts_with("@import(") {
                    let module = text
                        .trim_start_matches("@import(")
                        .trim_end_matches(')')
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    if !module.is_empty() {
                        let local_name = module.rsplit('.').next().unwrap_or(&module).to_string();
                        imports.push(ImportInfo {
                            local_name,
                            source_module: module,
                            original_name: Some("@import".to_string()),
                        });
                    }
                    return;
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_zig_imports(child, source, imports);
        }
    }

    // --- PowerShell ---

    fn extract_powershell_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_powershell_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_statement" | "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "class_statement" | "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_powershell_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_statement" | "enum_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_symbols(child, source, symbols, parent);
        }
    }

    fn extract_powershell_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "function_statement" | "function_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "command_expression" | "command" => {
                // PowerShell command calls like: Get-Process, Invoke-WebRequest
                if let Some(cmd_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(cmd_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(cmd_node) = node.child(0) {
                    // Fallback: first child is the command name
                    let callee = self.node_text(cmd_node, source).to_string();
                    if !callee.is_empty() {
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: callee,
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                    }
                }
            }
            "command_invocation_expression" => {
                if let Some(cmd_node) = node.child(0) {
                    let callee = self.node_text(cmd_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "invocation_expression" => {
                // Method-style calls: [System.IO.File]::ReadAllText() or $obj.Method()
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(member_node) = node.child_by_field_name("member") {
                    let callee = self.node_text(member_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_calls(child, source, calls, scope);
        }
    }

    fn extract_powershell_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "import_statement" => {
                if let Some(module_node) = node.child_by_field_name("module_name") {
                    let module = self.node_text(module_node, source).to_string();
                    if !module.is_empty() {
                        imports.push(ImportInfo {
                            local_name: module.clone(),
                            source_module: module,
                            original_name: None,
                        });
                    }
                }
                return;
            }
            "using_statement" => {
                // using module ModuleName; or using namespace System.IO;
                let text = self.node_text(node, source).to_string();
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() >= 3 {
                    let module = parts[2].trim_end_matches(';').to_string();
                    if !module.is_empty() {
                        let local_name =
                            module.split('.').next_back().unwrap_or(&module).to_string();
                        imports.push(ImportInfo {
                            local_name,
                            source_module: module,
                            original_name: Some(parts[1].to_string()),
                        });
                    }
                }
                return;
            }
            "command_expression" | "command" => {
                // Import-Module ModuleName
                if let Some(cmd_node) = node.child(0) {
                    let cmd_name = self.node_text(cmd_node, source);
                    if cmd_name == "Import-Module" {
                        // The module name is typically the second child or argument
                        if let Some(arg_node) = node.child(1) {
                            let module = self.node_text(arg_node, source).to_string();
                            let module = module.trim().to_string();
                            if !module.is_empty() {
                                imports.push(ImportInfo {
                                    local_name: module.clone(),
                                    source_module: module,
                                    original_name: None,
                                });
                            }
                        }
                        return;
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_imports(child, source, imports);
        }
    }

    // --- Objective-C ---

    fn extract_objc_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_objc_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_interface" | "interface_declaration" => {
                // @interface ClassName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    // Some grammars don't have a body field; recurse children
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "class_implementation" | "implementation_definition" => {
                // @implementation ClassName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into implementation body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "protocol_declaration" => {
                // @protocol ProtocolName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into protocol body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "method_declaration" | "method_definition" => {
                // - (void)methodName or + (id)classMethod
                if let Some(name_node) = node.child_by_field_name("selector") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_definition" | "declaration" => {
                // C-style function definitions in ObjC files
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if declarator.kind() == "function_declarator" {
                        if let Some(name_node) = declarator.child_by_field_name("declarator") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_objc_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_objc_symbols(child, source, symbols, parent);
        }
    }

    fn extract_objc_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method_declaration" | "method_definition" => node
                .child_by_field_name("selector")
                .or_else(|| node.child_by_field_name("name"))
                .map(|n| self.node_text(n, source).to_string()),
            "function_definition" => node
                .child_by_field_name("declarator")
                .and_then(|d| d.child_by_field_name("declarator"))
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "message_expression" | "message_send" => {
                // ObjC-style: [obj method] or [obj method:arg]
                if let Some(selector_node) = node.child_by_field_name("selector") {
                    let callee = self.node_text(selector_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "call_expression" => {
                // C-style function calls: NSLog(@"hello"), dispatch_async(...)
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = self.node_text(func_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_objc_calls(child, source, calls, scope);
        }
    }

    fn extract_objc_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "preproc_import" | "preproc_include" => {
                // #import <Foundation/Foundation.h> or #import "MyHeader.h"
                // #include <stdio.h> or #include "MyHeader.h"
                let text = self.node_text(node, source);
                if let Some(start) = text.find('<').or_else(|| text.find('"')) {
                    if let Some(end) = text[start + 1..]
                        .find('>')
                        .or_else(|| text[start + 1..].find('"'))
                    {
                        let module = text[start + 1..start + 1 + end].to_string();
                        if !module.is_empty() {
                            let local_name =
                                module.split('/').next_back().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_objc_imports(child, source, imports);
        }
    }

    // ── Elixir ──────────────────────────────────────────────────────────

    fn extract_elixir_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Elixir, signature is up to "do" keyword or first line
        if let Some(pos) = text.find(" do") {
            text[..pos + 3].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_elixir_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();

        if kind == "call" {
            // In Elixir tree-sitter, most definitions are `call` nodes.
            // The first child (target) tells us what kind of definition it is.
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "defmodule" => {
                        // defmodule MyModule do ... end
                        if let Some(arg) = node.child(1) {
                            let name = self.node_text(arg, source).to_string();
                            let name = name.trim().to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Module,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_elixir_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            // Recurse into the module body with module name as parent
                            let cursor = &mut node.walk();
                            for child in node.children(cursor) {
                                self.extract_elixir_symbols(child, source, symbols, Some(&name));
                            }
                            return;
                        }
                    }
                    "def" | "defp" | "defmacro" | "defmacrop" => {
                        // def function_name(args) do ... end
                        if let Some(arg) = node.child(1) {
                            let arg_text = self.node_text(arg, source).to_string();
                            let fn_name = arg_text
                                .split('(')
                                .next()
                                .unwrap_or(&arg_text)
                                .trim()
                                .to_string();
                            symbols.push(ParsedSymbol {
                                name: fn_name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_elixir_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                        return; // Don't recurse into function bodies for symbols
                    }
                    "defstruct" => {
                        // defstruct [:field1, :field2]
                        symbols.push(ParsedSymbol {
                            name: parent.unwrap_or("anonymous").to_string(),
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_elixir_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_symbols(child, source, symbols, parent);
        }
    }

    fn extract_elixir_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "def" | "defp" | "defmacro" | "defmacrop" => node.child(1).map(|arg| {
                        let arg_text = self.node_text(arg, source).to_string();
                        arg_text
                            .split('(')
                            .next()
                            .unwrap_or(&arg_text)
                            .trim()
                            .to_string()
                    }),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source).to_string();
                // Skip definition keywords and import keywords
                match target_text.as_str() {
                    "defmodule" | "def" | "defp" | "defmacro" | "defmacrop" | "defstruct"
                    | "use" | "import" | "alias" | "require" => {}
                    _ => {
                        // This is a regular function call
                        let callee = target_text
                            .rsplit('.')
                            .next()
                            .unwrap_or(&target_text)
                            .to_string();
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: callee,
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                    }
                }
            }
        }

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_calls(child, source, calls, scope);
        }
    }

    fn extract_elixir_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "use" | "import" | "alias" | "require" => {
                        // use Phoenix.Controller
                        // import Ecto.Query
                        // alias MyApp.Accounts.User
                        // require Logger
                        if let Some(arg) = node.child(1) {
                            let module = self.node_text(arg, source).to_string();
                            let module = module.trim().to_string();
                            let local_name =
                                module.rsplit('.').next().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: None,
                            });
                        }
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_imports(child, source, imports);
        }
    }

    // ── Scala ───────────────────────────────────────────────────────────

    fn extract_scala_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Scala, signature is up to the opening brace or `=`
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else if let Some(pos) = text.find('=') {
            text[..pos + 1].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_scala_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "object_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "trait_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "val_definition" | "var_definition" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    let name = self.node_text(pattern, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Variable,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_symbols(child, source, symbols, parent);
        }
    }

    fn extract_scala_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let callee_text = self.node_text(func_node, source).to_string();
                let callee = callee_text
                    .rsplit('.')
                    .next()
                    .unwrap_or(&callee_text)
                    .to_string();
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_calls(child, source, calls, scope);
        }
    }

    fn extract_scala_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        if node.kind() == "import_declaration" {
            let text = self.node_text(node, source).to_string();
            let path = text.trim_start_matches("import").trim();

            if path.contains('{') {
                // Grouped imports: import foo.{Bar, Baz}
                if let Some(brace_start) = path.find('{') {
                    let prefix = path[..brace_start].trim_end_matches('.').to_string();
                    let inner = &path[brace_start + 1..];
                    let inner = inner.trim_end_matches('}');
                    for item in inner.split(',') {
                        let item = item.trim();
                        if item.is_empty() {
                            continue;
                        }
                        // Handle rename: Bar => RenamedBar
                        if item.contains("=>") {
                            let parts: Vec<&str> = item.split("=>").collect();
                            if parts.len() == 2 {
                                let original = parts[0].trim().to_string();
                                let local = parts[1].trim().to_string();
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: format!("{}.{}", prefix, original),
                                    original_name: Some(original),
                                });
                            }
                        } else {
                            let full_path = format!("{}.{}", prefix, item);
                            imports.push(ImportInfo {
                                local_name: item.to_string(),
                                source_module: full_path,
                                original_name: None,
                            });
                        }
                    }
                }
            } else {
                // Simple import
                let local_name = path.rsplit('.').next().unwrap_or(path).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: path.to_string(),
                    original_name: None,
                });
            }
            return;
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_imports(child, source, imports);
        }
    }

    // ── Vue / Svelte script block extraction ─────────────────────────

    /// Extract the content of the first `<script>` block from a Vue or Svelte file.
    /// Falls back to the full source if no script block is found.
    fn extract_script_block(source: &str) -> String {
        // Match <script ...> (with optional attributes like lang="ts", setup, etc.)
        let lower = source.to_lowercase();
        if let Some(tag_start) = lower.find("<script") {
            // Find the closing > of the opening tag
            if let Some(content_start) = source[tag_start..].find('>') {
                let content_start = tag_start + content_start + 1;
                // Find the closing </script>
                if let Some(end) = lower[content_start..].find("</script>") {
                    return source[content_start..content_start + end].to_string();
                }
            }
        }
        // No script block found — return empty string (will produce no symbols)
        String::new()
    }

    // ── Dart ─────────────────────────────────────────────────────────

    fn extract_dart_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "mixin_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "extension_declaration" => {
                // extension Foo on Bar { ... }
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_signature" | "method_signature" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_body" => {
                // Skip — the parent function_signature already captured this
            }
            _ => {
                // Check for top-level or class-level function declarations
                // Dart grammar may represent them differently
                if kind == "declaration" || kind == "function_declaration" {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = self.node_text(name_node, source).to_string();
                        let sym_kind = if parent.is_some() {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        symbols.push(ParsedSymbol {
                            name,
                            kind: sym_kind,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_dart_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_symbols(child, source, symbols, parent);
        }
    }

    fn extract_dart_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_signature" | "method_signature" | "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        // Dart function invocations
        if kind == "selector" || kind == "argument_part" {
            // handled by parent
        }

        // Match function calls: identifier(...) or expr.identifier(...)
        if kind == "identifier" {
            if let Some(parent_node) = node.parent() {
                let parent_kind = parent_node.kind();
                if parent_kind == "assignable_expression"
                    || parent_kind == "primary"
                    || parent_kind == "function_expression_body"
                {
                    // Check if followed by argument_part
                    if let Some(next) = parent_node.next_sibling() {
                        if next.kind() == "selector" || next.kind() == "argument_part" {
                            let callee = self.node_text(node, source).to_string();
                            if let Some(caller) = scope {
                                calls.push(CallReference {
                                    caller_name: caller.to_string(),
                                    callee_name: callee,
                                    line: node.start_position().row as u32 + 1,
                                    chain: None,
                                    file: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_calls(child, source, calls, scope);
        }
    }

    fn extract_dart_imports(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        let kind = node.kind();

        if kind == "import_or_export" || kind == "import_specification" || kind == "library_import"
        {
            let text = self.node_text(node, source).to_string();
            if text.starts_with("import") {
                // import 'package:foo/bar.dart' as baz;
                // import 'package:foo/bar.dart' show Baz;
                // import 'package:foo/bar.dart';
                if let Some(start) = text.find('\'').or_else(|| text.find('"')) {
                    let quote = text.as_bytes()[start] as char;
                    if let Some(end) = text[start + 1..].find(quote) {
                        let module = text[start + 1..start + 1 + end].to_string();
                        let local = module
                            .rsplit('/')
                            .next()
                            .unwrap_or(&module)
                            .trim_end_matches(".dart")
                            .to_string();

                        // Check for 'as' alias
                        let alias = if let Some(as_pos) = text.find(" as ") {
                            let rest = text[as_pos + 4..].trim().trim_end_matches(';');
                            Some(rest.to_string())
                        } else {
                            None
                        };

                        imports.push(ImportInfo {
                            local_name: alias.unwrap_or_else(|| local.clone()),
                            source_module: module,
                            original_name: None,
                        });
                    }
                }
            }
            return;
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_imports(child, source, imports);
        }
    }

    fn extract_dart_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    // --- Metadata extraction helpers ---

    /// Extract Python metadata: decorators, return type, superclasses.
    fn extract_py_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Decorators: check if the parent is a decorated_definition
        if let Some(parent) = node.parent() {
            if parent.kind() == "decorated_definition" {
                let cursor = &mut parent.walk();
                for child in parent.children(cursor) {
                    if child.kind() == "decorator" {
                        let text = self.node_text(child, source).trim().to_string();
                        meta.decorators.push(text);
                    }
                }
            }
        }

        // Return type annotation
        if node.kind() == "function_definition" {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                let cleaned = text.strip_prefix("->").map(|s| s.trim()).unwrap_or(&text);
                if !cleaned.is_empty() {
                    meta.return_type = Some(cleaned.to_string());
                }
            }
        }

        // Superclasses for class_definition
        if node.kind() == "class_definition" {
            if let Some(args) = node.child_by_field_name("superclasses") {
                let cursor = &mut args.walk();
                for child in args.children(cursor) {
                    let ck = child.kind();
                    if ck == "identifier" || ck == "attribute" || ck == "subscript" {
                        meta.superclasses
                            .push(self.node_text(child, source).to_string());
                    }
                }
            }
        }

        if meta.decorators.is_empty() && meta.return_type.is_none() && meta.superclasses.is_empty()
        {
            None
        } else {
            Some(meta)
        }
    }

    /// Extract Rust metadata: visibility, attributes (decorators), return type.
    fn extract_rust_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Visibility modifier
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            if child.kind() == "visibility_modifier" {
                let vis_text = self.node_text(child, source).trim().to_string();
                meta.visibility = Some(if vis_text.starts_with("pub(crate)") {
                    Visibility::Internal
                } else if vis_text.starts_with("pub") {
                    Visibility::Public
                } else {
                    Visibility::Private
                });
                break;
            }
        }

        // Attributes (decorators): look at preceding siblings
        if let Some(parent) = node.parent() {
            let cursor2 = &mut parent.walk();
            let children: Vec<_> = parent.children(cursor2).collect();
            let mut idx = children.len();
            for (i, child) in children.iter().enumerate() {
                if child.id() == node.id() {
                    idx = i;
                    break;
                }
            }
            for i in (0..idx).rev() {
                if children[i].kind() == "attribute_item" {
                    let text = self.node_text(children[i], source).trim().to_string();
                    meta.decorators.push(text);
                } else if children[i].kind() != "line_comment"
                    && children[i].kind() != "block_comment"
                {
                    break;
                }
            }
            meta.decorators.reverse();
        }

        // Return type
        if node.kind() == "function_item" || node.kind() == "function_signature_item" {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                if !text.is_empty() {
                    meta.return_type = Some(text);
                }
            }
        }

        // Type parameters
        if let Some(tp) = node.child_by_field_name("type_parameters") {
            let text = self.node_text(tp, source).trim().to_string();
            if !text.is_empty() {
                meta.type_params.push(text);
            }
        }

        if meta.visibility.is_none()
            && meta.decorators.is_empty()
            && meta.return_type.is_none()
            && meta.type_params.is_empty()
        {
            None
        } else {
            Some(meta)
        }
    }

    /// Extract TypeScript/JavaScript metadata: decorators, return type, extends/implements.
    fn extract_ts_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Decorators: look at preceding siblings
        if let Some(parent) = node.parent() {
            let cursor = &mut parent.walk();
            let children: Vec<_> = parent.children(cursor).collect();
            let mut idx = children.len();
            for (i, child) in children.iter().enumerate() {
                if child.id() == node.id() {
                    idx = i;
                    break;
                }
            }
            for i in (0..idx).rev() {
                if children[i].kind() == "decorator" {
                    let text = self.node_text(children[i], source).trim().to_string();
                    meta.decorators.push(text);
                } else if children[i].kind() != "comment" {
                    break;
                }
            }
            meta.decorators.reverse();
        }

        // Return type annotation
        if node.kind() == "function_declaration"
            || node.kind() == "method_definition"
            || node.kind() == "arrow_function"
        {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                let cleaned = text.strip_prefix(':').map(|s| s.trim()).unwrap_or(&text);
                if !cleaned.is_empty() {
                    meta.return_type = Some(cleaned.to_string());
                }
            }
        }

        // Extends / implements for class_declaration
        if node.kind() == "class_declaration" {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "class_heritage" {
                    let cursor2 = &mut child.walk();
                    for hchild in child.children(cursor2) {
                        if hchild.kind() == "extends_clause" || hchild.kind() == "implements_clause"
                        {
                            let cursor3 = &mut hchild.walk();
                            for gchild in hchild.children(cursor3) {
                                let gk = gchild.kind();
                                if gk == "identifier"
                                    || gk == "nested_identifier"
                                    || gk == "generic_type"
                                    || gk == "type_identifier"
                                {
                                    meta.superclasses
                                        .push(self.node_text(gchild, source).to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Type parameters
        if let Some(tp) = node.child_by_field_name("type_parameters") {
            let text = self.node_text(tp, source).trim().to_string();
            if !text.is_empty() {
                meta.type_params.push(text);
            }
        }

        if meta.decorators.is_empty()
            && meta.return_type.is_none()
            && meta.superclasses.is_empty()
            && meta.type_params.is_empty()
        {
            None
        } else {
            Some(meta)
        }
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

    #[test]
    fn test_parse_typescript_function() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function greet(name: string): string { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_typescript_class() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id: string) { return null; }
    deleteUser(id: string) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_typescript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("const add = (a: number, b: number) => a + b;")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_python_function() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse("def greet(name: str) -> str:\n    return f'Hello {name}'")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_python_class() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
class UserService:
    def get_user(self, id: str):
        return None
    def delete_user(self, id: str):
        pass
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"get_user"));
        assert!(names.contains(&"delete_user"));
    }

    #[test]
    fn test_call_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    #[test]
    fn test_import_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_parse_javascript_function() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse("function greet(name) { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_javascript_class() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id) { return null; }
    deleteUser(id) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_javascript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser.parse("const add = (a, b) => a + b;").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_import_extraction_js_esm() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user.js';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_import_extraction_js_commonjs() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
const { UserService } = require('./services/user.js');
const axios = require('axios');
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "UserService" || i.local_name == "axios"));
    }

    // --- Rust tests ---

    #[test]
    fn test_parse_rust_function() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("fn process(input: &str) -> Result<String> { Ok(input.to_string()) }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "process");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_rust_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("struct Server { port: u16, host: String }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Server");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_rust_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("enum Status { Active, Inactive, Error(String) }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Status");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_rust_trait() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
trait Handler {
    fn handle(&self, req: Request) -> Response;
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Handler"));
        assert!(names.contains(&"handle"));
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "Handler")
                .unwrap()
                .kind,
            SymbolKind::Interface
        );
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "handle")
                .unwrap()
                .kind,
            SymbolKind::Method
        );
    }

    #[test]
    fn test_parse_rust_impl_block() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
struct Server { port: u16 }

impl Server {
    fn new(port: u16) -> Self { Server { port } }
    fn start(&self) -> bool { true }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"new"));
        assert!(names.contains(&"start"));
        // Methods inside impl should have parent_name set
        let new_sym = result.symbols.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new_sym.kind, SymbolKind::Method);
        assert_eq!(new_sym.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_rust_trait_impl() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
impl Handler for Server {
    fn handle(&self) -> bool { true }
}
"#,
            )
            .unwrap();
        let handle = result.symbols.iter().find(|s| s.name == "handle").unwrap();
        assert_eq!(handle.kind, SymbolKind::Method);
        assert_eq!(handle.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_rust_const_static() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
const MAX_SIZE: usize = 1024;
static COUNTER: u32 = 0;
"#,
            )
            .unwrap();
        let max_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MAX_SIZE")
            .unwrap();
        assert_eq!(max_sym.kind, SymbolKind::Constant);
        let counter_sym = result.symbols.iter().find(|s| s.name == "COUNTER").unwrap();
        assert_eq!(counter_sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn test_parse_rust_type_alias() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("type Result<T> = std::result::Result<T, Error>;")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Result");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_rust_macro_definition() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
macro_rules! my_macro {
    ($x:expr) => { println!("{}", $x) };
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "my_macro");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_rust_module() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("mod utils { fn helper() {} }").unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"utils"));
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "utils")
                .unwrap()
                .kind,
            SymbolKind::Module
        );
    }

    #[test]
    fn test_call_extraction_rust() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
fn main() {
    let result = process(&input);
    println!("done");
    let v = Vec::new();
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "process"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
        assert!(result.calls.iter().any(|c| c.callee_name == "new"));
    }

    #[test]
    fn test_import_extraction_rust() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
use std::collections::HashMap;
use std::{io, fs};
use crate::parser::SourceLanguage;
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "HashMap"));
        assert!(result.imports.iter().any(|i| i.local_name == "io"));
        assert!(result.imports.iter().any(|i| i.local_name == "fs"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "SourceLanguage"));
    }

    #[test]
    fn test_import_extraction_rust_mod_declaration() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("mod parser;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "parser");
    }

    #[test]
    fn test_call_extraction_js() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    // --- Go tests ---

    #[test]
    fn test_parse_go_function() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func greet(name string) string {
    return "Hello " + name
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_go_struct_and_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type Server struct {
    Host string
    Port int
}

func (s *Server) Start() error {
    return nil
}

func (s Server) Address() string {
    return s.Host
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"Start"));
        assert!(names.contains(&"Address"));

        let server_sym = result.symbols.iter().find(|s| s.name == "Server").unwrap();
        assert_eq!(server_sym.kind, SymbolKind::Class);

        let start_sym = result.symbols.iter().find(|s| s.name == "Start").unwrap();
        assert_eq!(start_sym.kind, SymbolKind::Method);
        assert_eq!(start_sym.parent_name.as_deref(), Some("Server"));

        let addr_sym = result.symbols.iter().find(|s| s.name == "Address").unwrap();
        assert_eq!(addr_sym.kind, SymbolKind::Method);
        assert_eq!(addr_sym.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_go_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type Handler interface {
    ServeHTTP(w ResponseWriter, r *Request)
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Handler");
        assert_eq!(result.symbols[0].kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_go_const_and_var() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

const MaxRetries = 3

var DefaultTimeout = 30
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MaxRetries"));
        assert!(names.contains(&"DefaultTimeout"));

        let const_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MaxRetries")
            .unwrap();
        assert_eq!(const_sym.kind, SymbolKind::Constant);

        let var_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "DefaultTimeout")
            .unwrap();
        assert_eq!(var_sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn test_parse_go_type_alias() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type UserID string
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "UserID");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_go() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func main() {
    result := processData(input)
    fmt.Println(result)
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Println"));
    }

    #[test]
    fn test_call_extraction_go_goroutine_and_defer() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func serve() {
    go handleRequest(conn)
    defer file.Close()
}
"#,
            )
            .unwrap();
        assert!(result
            .calls
            .iter()
            .any(|c| c.callee_name == "handleRequest"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Close"));
    }

    #[test]
    fn test_import_extraction_go() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

import (
    "fmt"
    "net/http"
    h "net/http"
)
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "fmt"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "http" && i.source_module == "net/http"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "h" && i.source_module == "net/http"));
    }

    #[test]
    fn test_import_extraction_go_single() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

import "fmt"
"#,
            )
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "fmt");
        assert_eq!(result.imports[0].source_module, "fmt");
    }

    // --- Java tests ---

    #[test]
    fn test_parse_java_class() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class UserService {
    public String getUser(String id) {
        return null;
    }
    public void deleteUser(String id) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "UserService")
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let method_sym = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(method_sym.kind, SymbolKind::Method);
        assert_eq!(method_sym.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_java_constructor() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class Server {
    private int port;

    public Server(int port) {
        this.port = port;
    }

    public void start() {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"Server")); // constructor
        assert!(names.contains(&"start"));
        assert!(names.contains(&"port"));

        let constructor = result
            .symbols
            .iter()
            .find(|s| s.name == "Server" && s.kind == SymbolKind::Method)
            .unwrap();
        assert_eq!(constructor.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_java_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public interface Repository {
    Object findById(String id);
    void save(Object entity);
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let iface = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);

        let method = result
            .symbols
            .iter()
            .find(|s| s.name == "findById")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.parent_name.as_deref(), Some("Repository"));
    }

    #[test]
    fn test_parse_java_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
enum Status {
    ACTIVE,
    INACTIVE,
    ERROR
}
"#,
            )
            .unwrap();
        assert_eq!(
            result.symbols.iter().filter(|s| s.name == "Status").count(),
            1
        );
        let status = result.symbols.iter().find(|s| s.name == "Status").unwrap();
        assert_eq!(status.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_java_record() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
record Point(int x, int y) {
    public double distance() {
        return Math.sqrt(x * x + y * y);
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"distance"));

        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_java_field() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class Config {
    private String host;
    private int port;
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"host"));
        assert!(names.contains(&"port"));

        let host = result.symbols.iter().find(|s| s.name == "host").unwrap();
        assert_eq!(host.kind, SymbolKind::Variable);
        assert_eq!(host.parent_name.as_deref(), Some("Config"));
    }

    #[test]
    fn test_call_extraction_java() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class App {
    public void main() {
        String result = processData(input);
        System.out.println(result);
        List<String> items = new ArrayList<>();
    }
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
        assert!(result.calls.iter().any(|c| c.callee_name == "ArrayList"));
    }

    #[test]
    fn test_import_extraction_java() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
package com.example.app;

import java.util.List;
import java.util.Map;
import java.io.*;
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "List"));
        assert!(result.imports.iter().any(|i| i.local_name == "Map"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "*" && i.source_module.contains("java.io")));
        // Package declaration tracked
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "com.example.app"
                && i.original_name.as_deref() == Some("package")));
    }

    #[test]
    fn test_import_extraction_java_static() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
import static java.lang.Math.PI;
"#,
            )
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "PI");
        assert_eq!(result.imports[0].original_name.as_deref(), Some("static"));
    }

    // --- C# tests ---

    #[test]
    fn test_parse_csharp_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public class UserService
{
    public string Name { get; set; }

    public UserService(string name)
    {
        Name = name;
    }

    public string GetUser(string id)
    {
        return null;
    }

    public void DeleteUser(string id)
    {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"GetUser"));
        assert!(names.contains(&"DeleteUser"));
        assert!(names.contains(&"Name"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "UserService" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let get_user = result.symbols.iter().find(|s| s.name == "GetUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));

        let name_prop = result.symbols.iter().find(|s| s.name == "Name").unwrap();
        assert_eq!(name_prop.kind, SymbolKind::Variable);
        assert_eq!(name_prop.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_csharp_interface() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public interface IHandler
{
    void Handle(Request req);
    string GetName();
}
"#,
            )
            .unwrap();
        let handler = result
            .symbols
            .iter()
            .find(|s| s.name == "IHandler")
            .unwrap();
        assert_eq!(handler.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_csharp_enum() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public enum Status
{
    Active,
    Inactive,
    Error
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Status");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_csharp_struct() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public struct Point
{
    public int X { get; set; }
    public int Y { get; set; }
}
"#,
            )
            .unwrap();
        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_csharp_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
namespace MyApp.Services
{
    public class Foo { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MyApp.Services"));
        let ns = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Services")
            .unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_csharp_delegate() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse("public delegate void EventHandler(object sender, EventArgs e);")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "EventHandler");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_csharp() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public class App
{
    public void Run()
    {
        var result = ProcessData(input);
        Console.WriteLine(result);
        var svc = new UserService();
    }
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "ProcessData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "WriteLine"));
        assert!(result.calls.iter().any(|c| c.callee_name == "UserService"));
    }

    #[test]
    fn test_import_extraction_csharp() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
using System;
using System.Collections.Generic;
using System.Linq;
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "System" && i.source_module == "System"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "Generic" && i.source_module == "System.Collections.Generic"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "Linq" && i.source_module == "System.Linq"));
    }

    #[test]
    fn test_parse_csharp_record() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse("public record Person(string Name, int Age);")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Person");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }

    // --- C tests ---

    #[test]
    fn test_parse_c_function() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse("int add(int a, int b) { return a + b; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_c_struct() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
struct Node {
    int value;
    struct Node *next;
};
"#,
            )
            .unwrap();
        let node_sym = result.symbols.iter().find(|s| s.name == "Node").unwrap();
        assert_eq!(node_sym.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_c_enum() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
enum Color {
    RED,
    GREEN,
    BLUE
};
"#,
            )
            .unwrap();
        let color = result.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_c_union() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
union Data {
    int i;
    float f;
    char c;
};
"#,
            )
            .unwrap();
        let data = result.symbols.iter().find(|s| s.name == "Data").unwrap();
        assert_eq!(data.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_c_typedef() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser.parse("typedef unsigned long ulong;").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "ulong");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_c_macro() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
#define MAX_SIZE 1024
#define SQUARE(x) ((x) * (x))
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MAX_SIZE"));
        assert!(names.contains(&"SQUARE"));
        for sym in &result.symbols {
            assert_eq!(sym.kind, SymbolKind::Constant);
        }
    }

    #[test]
    fn test_parse_c_forward_declaration() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser.parse("int init(struct Config *config);").unwrap();
        let init_sym = result.symbols.iter().find(|s| s.name == "init").unwrap();
        assert_eq!(init_sym.kind, SymbolKind::Function);
    }

    #[test]
    fn test_call_extraction_c() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
void process(int x) {
    int result = add(x, 1);
    printf("Result: %d\n", result);
    free(ptr);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "add"));
        assert!(result.calls.iter().any(|c| c.callee_name == "printf"));
        assert!(result.calls.iter().any(|c| c.callee_name == "free"));
        for call in &result.calls {
            assert_eq!(call.caller_name, "process");
        }
    }

    #[test]
    fn test_import_extraction_c() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
#include <stdio.h>
#include <stdlib.h>
#include "myheader.h"
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.source_module == "stdio.h"));
        assert!(result.imports.iter().any(|i| i.source_module == "stdlib.h"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "myheader.h"));
        // Local names derived from filenames without extension
        assert!(result.imports.iter().any(|i| i.local_name == "stdio"));
        assert!(result.imports.iter().any(|i| i.local_name == "myheader"));
    }

    #[test]
    fn test_parse_c_pointer_return_function() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse("void *allocate(size_t size) { return malloc(size); }")
            .unwrap();
        let alloc = result
            .symbols
            .iter()
            .find(|s| s.name == "allocate")
            .unwrap();
        assert_eq!(alloc.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_c_extension() {
        assert_eq!(SourceLanguage::from_extension("c"), Some(SourceLanguage::C));
        assert_eq!(SourceLanguage::from_extension("h"), Some(SourceLanguage::C));
    }

    // --- C++ tests ---

    #[test]
    fn test_parse_cpp_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
class Dog {
public:
    Dog(const std::string& name) : name_(name) {}

    std::string speak() const {
        return "Woof!";
    }

    void fetch(const std::string& item) {
        std::cout << item << std::endl;
    }

private:
    std::string name_;
};
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Dog"));
        assert!(names.contains(&"speak"));
        assert!(names.contains(&"fetch"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "Dog" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let speak = result.symbols.iter().find(|s| s.name == "speak").unwrap();
        assert_eq!(speak.kind, SymbolKind::Method);
        assert_eq!(speak.parent_name.as_deref(), Some("Dog"));
    }

    #[test]
    fn test_parse_cpp_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
struct Point {
    double x;
    double y;

    double distance() const {
        return 0.0;
    }
};
"#,
            )
            .unwrap();
        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);

        let distance = result
            .symbols
            .iter()
            .find(|s| s.name == "distance")
            .unwrap();
        assert_eq!(distance.kind, SymbolKind::Method);
        assert_eq!(distance.parent_name.as_deref(), Some("Point"));
    }

    #[test]
    fn test_parse_cpp_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
enum class Color {
    Red,
    Green,
    Blue
};
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Color");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_cpp_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
namespace myapp {
    void helper() {}
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"myapp"));
        let ns = result.symbols.iter().find(|s| s.name == "myapp").unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_cpp_free_function() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
void greet(const std::string& name) {
    std::cout << "Hello, " << name << std::endl;
}
"#,
            )
            .unwrap();
        let greet = result.symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(greet.kind, SymbolKind::Function);
        assert_eq!(greet.parent_name, None);
    }

    #[test]
    fn test_parse_cpp_template_function() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
template<typename T>
T max_value(T a, T b) {
    return (a > b) ? a : b;
}
"#,
            )
            .unwrap();
        let max_val = result
            .symbols
            .iter()
            .find(|s| s.name == "max_value")
            .unwrap();
        assert_eq!(max_val.kind, SymbolKind::Function);
        assert!(max_val.signature.contains("template"));
    }

    #[test]
    fn test_parse_cpp_template_class() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
template<typename T>
class Container {
public:
    void add(const T& item) {}
    size_t size() const { return 0; }
};
"#,
            )
            .unwrap();
        let container = result
            .symbols
            .iter()
            .find(|s| s.name == "Container")
            .unwrap();
        assert_eq!(container.kind, SymbolKind::Class);
        assert!(container.signature.contains("template"));
    }

    #[test]
    fn test_parse_cpp_typedef_and_using() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
typedef unsigned long ulong;
using StringVec = std::vector<std::string>;
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"ulong"));
        assert!(names.contains(&"StringVec"));

        let ulong_sym = result.symbols.iter().find(|s| s.name == "ulong").unwrap();
        assert_eq!(ulong_sym.kind, SymbolKind::TypeAlias);

        let sv = result
            .symbols
            .iter()
            .find(|s| s.name == "StringVec")
            .unwrap();
        assert_eq!(sv.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_cpp() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
void doWork() {
    greet("world");
    dog.fetch("ball");
    auto p = new Point{1.0, 2.0};
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "greet"));
        assert!(result.calls.iter().any(|c| c.callee_name == "fetch"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Point"));
    }

    #[test]
    fn test_import_extraction_cpp() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
#include <iostream>
#include <string>
#include "myheader.h"
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.source_module == "iostream"));
        assert!(result.imports.iter().any(|i| i.source_module == "string"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "myheader.h" && i.local_name == "myheader"));
    }

    #[test]
    fn test_cpp_extension_mapping() {
        assert_eq!(
            SourceLanguage::from_extension("cpp"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("cc"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("cxx"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("hpp"),
            Some(SourceLanguage::Cpp)
        );
    }
    // --- Ruby tests ---

    #[test]
    fn test_parse_ruby_class_and_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class User
  def initialize(name)
    @name = name
  end

  def greet
    puts "Hello"
  end

  def self.create(name)
    new(name)
  end
end
"#,
            )
            .unwrap();

        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"initialize"));
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"create"));

        let user = result.symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.kind, SymbolKind::Class);

        let init = result
            .symbols
            .iter()
            .find(|s| s.name == "initialize")
            .unwrap();
        assert_eq!(init.kind, SymbolKind::Method);
        assert_eq!(init.parent_name.as_deref(), Some("User"));

        let create = result.symbols.iter().find(|s| s.name == "create").unwrap();
        assert_eq!(create.kind, SymbolKind::Method);
        assert_eq!(create.parent_name.as_deref(), Some("User"));
    }

    #[test]
    fn test_parse_ruby_module() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
module Serializable
  def to_json
    "json"
  end
end
"#,
            )
            .unwrap();

        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Serializable"));
        assert!(names.contains(&"to_json"));

        let m = result
            .symbols
            .iter()
            .find(|s| s.name == "Serializable")
            .unwrap();
        assert_eq!(m.kind, SymbolKind::Module);

        let method = result.symbols.iter().find(|s| s.name == "to_json").unwrap();
        assert_eq!(method.parent_name.as_deref(), Some("Serializable"));
    }

    #[test]
    fn test_parse_ruby_constant() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser.parse("MAX_RETRIES = 3").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "MAX_RETRIES");
        assert_eq!(result.symbols[0].kind, SymbolKind::Constant);
    }

    #[test]
    fn test_parse_ruby_calls() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class Foo
  def bar
    puts "hello"
    baz.quux
  end
end
"#,
            )
            .unwrap();

        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"puts"));
        assert!(callees.contains(&"quux"));
    }

    #[test]
    fn test_parse_ruby_super_and_yield() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class Child < Parent
  def initialize
    super
  end

  def action
    yield
  end
end
"#,
            )
            .unwrap();

        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"super"));
        assert!(callees.contains(&"yield"));
    }

    #[test]
    fn test_parse_ruby_imports() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
require 'json'
require_relative 'helpers/utils'
"#,
            )
            .unwrap();

        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "json" && i.original_name.as_deref() == Some("require")));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "helpers/utils"
                && i.original_name.as_deref() == Some("require_relative")));
    }

    #[test]
    fn test_parse_ruby_mixins() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class User
  include Serializable
  extend ClassMethods
  prepend Logging
end
"#,
            )
            .unwrap();

        let import_names: Vec<&str> = result
            .imports
            .iter()
            .map(|i| i.local_name.as_str())
            .collect();
        assert!(import_names.contains(&"Serializable"));
        assert!(import_names.contains(&"ClassMethods"));
        assert!(import_names.contains(&"Logging"));
    }

    // --- Kotlin tests ---

    #[test]
    fn test_parse_kotlin_data_class() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse("data class User(val id: String, val name: String)")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "User");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_kotlin_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    fun getUser(id: String): User? {
        return null
    }
    fun deleteUser(id: String) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_kotlin_object() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
object AppConfig {
    val version = "1.0.0"
    fun getEnvironment(): String {
        return "development"
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"AppConfig"));
        assert!(names.contains(&"getEnvironment"));

        let config = result
            .symbols
            .iter()
            .find(|s| s.name == "AppConfig")
            .unwrap();
        assert_eq!(config.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_kotlin_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
interface Repository<T> {
    fun findById(id: String): T?
    fun save(entity: T): T
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let repo = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(repo.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_kotlin_companion_object() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    companion object {
        fun create(): UserService {
            return UserService()
        }
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"create"));

        let create = result.symbols.iter().find(|s| s.name == "create").unwrap();
        assert_eq!(create.kind, SymbolKind::Method);
        assert_eq!(create.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_kotlin_top_level_function() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
fun main() {
    println("Hello")
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "main");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
        assert_eq!(result.symbols[0].parent_name, None);
    }

    #[test]
    fn test_call_extraction_kotlin() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
fun main() {
    val result = processData(input)
    println(result)
    config.getEnvironment()
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
    }

    #[test]
    fn test_import_extraction_kotlin() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
package com.example.myapp

import kotlinx.coroutines.flow.Flow
import java.util.UUID as JavaUUID
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "com.example.myapp"
                && i.original_name.as_deref() == Some("package")));
        assert!(result.imports.iter().any(|i| i.local_name == "Flow"));
    }

    #[test]
    fn test_parse_kotlin_extension() {
        let lang = SourceLanguage::from_extension("kt");
        assert_eq!(lang, Some(SourceLanguage::Kotlin));
        let lang = SourceLanguage::from_extension("kts");
        assert_eq!(lang, Some(SourceLanguage::Kotlin));
    }

    // --- Swift tests ---

    #[test]
    fn test_parse_swift_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    func getUser(id: String) -> String {
        return ""
    }
    func deleteUser(id: String) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_swift_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
struct Point {
    var x: Int
    var y: Int
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"x"));
        assert!(names.contains(&"y"));

        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);

        let x = result.symbols.iter().find(|s| s.name == "x").unwrap();
        assert_eq!(x.kind, SymbolKind::Variable);
        assert_eq!(x.parent_name.as_deref(), Some("Point"));
    }

    #[test]
    fn test_parse_swift_protocol() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
protocol Repository {
    func findById(id: String) -> Any?
    func save(entity: Any) -> Any
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let repo = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(repo.kind, SymbolKind::Interface);

        let find = result
            .symbols
            .iter()
            .find(|s| s.name == "findById")
            .unwrap();
        assert_eq!(find.kind, SymbolKind::Method);
        assert_eq!(find.parent_name.as_deref(), Some("Repository"));
    }

    #[test]
    fn test_parse_swift_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
enum Color {
    case red
    case green
    case blue
}
"#,
            )
            .unwrap();
        let color = result.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_swift_typealias() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser.parse("typealias UserID = String").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "UserID");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_swift_extension() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
extension UserService {
    func getAllUsers() -> [User] {
        return []
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"getAllUsers"));

        let get_all = result
            .symbols
            .iter()
            .find(|s| s.name == "getAllUsers")
            .unwrap();
        assert_eq!(get_all.kind, SymbolKind::Method);
        assert_eq!(get_all.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_swift_top_level_function() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
func main() {
    print("Hello")
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "main");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
        assert_eq!(result.symbols[0].parent_name, None);
    }

    #[test]
    fn test_call_extraction_swift() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
func main() {
    let result = processData(input)
    print(result)
    service.getUser(id: "1")
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "print"));
        assert!(result.calls.iter().any(|c| c.callee_name == "getUser"));
    }

    #[test]
    fn test_import_extraction_swift() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
import Foundation
import UIKit
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "Foundation"));
        assert!(result.imports.iter().any(|i| i.source_module == "UIKit"));
    }

    #[test]
    fn test_parse_swift_file_extension() {
        let lang = SourceLanguage::from_extension("swift");
        assert_eq!(lang, Some(SourceLanguage::Swift));
    }

    // --- PHP tests ---

    #[test]
    fn test_parse_php_extension() {
        let lang = SourceLanguage::from_extension("php");
        assert_eq!(lang, Some(SourceLanguage::Php));
    }

    #[test]
    fn test_parse_php_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
class UserService {
    public string $name;

    public function getUser(string $id): ?User {
        return null;
    }

    public function deleteUser(string $id): void {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"UserService"),
            "missing UserService, got: {:?}",
            names
        );
        assert!(
            names.contains(&"getUser"),
            "missing getUser, got: {:?}",
            names
        );
        assert!(
            names.contains(&"deleteUser"),
            "missing deleteUser, got: {:?}",
            names
        );
        assert!(
            names.contains(&"name"),
            "missing property name, got: {:?}",
            names
        );

        // Methods should have UserService as parent
        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_php_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
interface Cacheable {
    public function cacheKey(): string;
}
"#,
            )
            .unwrap();
        let iface = result
            .symbols
            .iter()
            .find(|s| s.name == "Cacheable")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_php_trait() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
trait Timestamps {
    public function createdAt(): string {
        return $this->created_at;
    }
}
"#,
            )
            .unwrap();
        let trait_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "Timestamps")
            .unwrap();
        assert_eq!(trait_sym.kind, SymbolKind::Interface);
        let method = result
            .symbols
            .iter()
            .find(|s| s.name == "createdAt")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.parent_name.as_deref(), Some("Timestamps"));
    }

    #[test]
    fn test_parse_php_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
enum Status: string {
    case Active = 'active';
    case Inactive = 'inactive';
}
"#,
            )
            .unwrap();
        let status = result.symbols.iter().find(|s| s.name == "Status").unwrap();
        assert_eq!(status.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_php_function() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
function findUser(string $name): ?User {
    return null;
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "findUser");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_php_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
namespace App\Models;

class User {}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"App\\Models"),
            "missing namespace, got: {:?}",
            names
        );
        let ns = result
            .symbols
            .iter()
            .find(|s| s.name == "App\\Models")
            .unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_php_const() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
const MAX_RETRIES = 3;
"#,
            )
            .unwrap();
        let c = result
            .symbols
            .iter()
            .find(|s| s.name == "MAX_RETRIES")
            .unwrap();
        assert_eq!(c.kind, SymbolKind::Constant);
    }

    #[test]
    fn test_call_extraction_php() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
function main() {
    $user = new User("Alice", 30);
    $user->greet();
    User::create("Bob", 25);
    processData($user);
}
"#,
            )
            .unwrap();
        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"User"),
            "missing new User, got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"greet"),
            "missing ->greet(), got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"create"),
            "missing ::create(), got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"processData"),
            "missing processData(), got: {:?}",
            callees
        );
    }

    #[test]
    fn test_import_extraction_php() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
namespace App\Models;

use App\Contracts\Renderable;
use App\Services\Logger as AppLogger;
"#,
            )
            .unwrap();
        // Namespace import
        assert!(
            result.imports.iter().any(|i| i.local_name == "App\\Models"
                && i.original_name.as_deref() == Some("namespace")),
            "missing namespace import, got: {:?}",
            result.imports
        );
        // Use statements
        assert!(
            result.imports.iter().any(|i| i.local_name == "Renderable"),
            "missing Renderable import, got: {:?}",
            result.imports
        );
        assert!(
            result.imports.iter().any(|i| i.local_name == "AppLogger"),
            "missing AppLogger import, got: {:?}",
            result.imports
        );
    }

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

    #[test]
    fn test_simple_call_has_no_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { bar(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "bar");
        assert!(call.is_some(), "should detect the bar() call");
        assert!(
            call.unwrap().chain.is_none(),
            "simple call should have no chain"
        );
    }

    #[test]
    fn test_ts_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { obj.method(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "method");
        assert!(call.is_some(), "should detect obj.method() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("method call should have a chain");
        assert_eq!(chain[0], ExpressionStep::Ident("obj".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("method".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
    }

    #[test]
    fn test_py_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser.parse("def foo():\n    obj.method()\n").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "method");
        assert!(call.is_some(), "should detect obj.method() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("method call should have a chain");
        assert_eq!(chain[0], ExpressionStep::Ident("obj".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("method".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
    }

    #[test]
    fn test_rust_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("fn foo() { obj.method(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "method");
        assert!(call.is_some(), "should detect obj.method() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("method call should have a chain");
        assert_eq!(chain[0], ExpressionStep::Ident("obj".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("method".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
    }

    #[test]
    fn test_ts_chained_calls() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { a.b().c(); }").unwrap();
        // The outer call is c(), with the receiver being a.b()
        let call = result.calls.iter().find(|c| c.callee_name == "c");
        assert!(call.is_some(), "should detect .c() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("chained call should have a chain");
        // a.b().c() => [Ident("a"), Field("b"), Call, Field("c"), Call]
        assert_eq!(chain[0], ExpressionStep::Ident("a".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("b".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
        assert_eq!(chain[3], ExpressionStep::Field("c".to_string()));
        assert_eq!(chain[4], ExpressionStep::Call);
    }

    // --- Metadata extraction tests ---

    #[test]
    fn test_python_decorated_function_has_decorators() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
@app.route("/api")
@login_required
def handle_request():
    pass
"#,
            )
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "handle_request")
            .expect("should find handle_request");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.decorators.is_empty(),
            "should have at least one decorator, got {:?}",
            meta.decorators
        );
    }

    #[test]
    fn test_python_class_with_superclasses() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
class MyView(BaseView, Mixin):
    pass
"#,
            )
            .unwrap();
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "MyView")
            .expect("should find MyView");
        let meta = cls.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.superclasses.is_empty(),
            "should have superclasses, got {:?}",
            meta.superclasses
        );
    }

    #[test]
    fn test_python_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
def greet(name: str) -> str:
    return f"Hello {name}"
"#,
            )
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "greet")
            .expect("should find greet");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
        assert_eq!(meta.return_type.as_deref().unwrap(), "str");
    }

    #[test]
    fn test_rust_pub_function_has_visibility() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("pub fn serve(port: u16) -> Result<()> { Ok(()) }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "serve")
            .expect("should find serve");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert_eq!(meta.visibility, Some(Visibility::Public));
    }

    #[test]
    fn test_rust_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("fn compute(x: i32) -> Vec<String> { vec![] }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "compute")
            .expect("should find compute");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
    }

    #[test]
    fn test_typescript_class_extends_has_superclasses() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class Animal {
    name: string;
}

class Dog extends Animal {
    bark() {}
}
"#,
            )
            .unwrap();
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "Dog")
            .expect("should find Dog");
        let meta = cls.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.superclasses.is_empty(),
            "should have superclasses from extends"
        );
    }

    #[test]
    fn test_typescript_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function add(a: number, b: number): number { return a + b; }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "add")
            .expect("should find add");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
    }

    #[test]
    fn test_python_alias_extraction() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
handler = authenticate
callback = module.process
x = 42
"#,
            )
            .unwrap();
        // Should find 2 aliases: handler→authenticate, callback→process
        assert_eq!(result.aliases.len(), 2);
        assert!(result
            .aliases
            .contains(&("handler".to_string(), "authenticate".to_string())));
        assert!(result
            .aliases
            .contains(&("callback".to_string(), "process".to_string())));
    }

    #[test]
    fn test_typescript_alias_extraction() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
const handler = authenticate;
let cb = service.validate;
handler = reassigned;
"#,
            )
            .unwrap();
        // Should find: handler→authenticate, cb→validate, handler→reassigned
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "authenticate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "cb" && t == "validate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "reassigned"));
    }

    #[test]
    fn test_alias_resolves_call() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
def authenticate(user):
    pass

def main():
    handler = authenticate
    handler()
"#,
            )
            .unwrap();
        // Aliases collected
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "authenticate"));
        // Call to handler() exists
        assert!(result.calls.iter().any(|c| c.callee_name == "handler"));
    }
}
