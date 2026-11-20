//! Tree-sitter-based source code parsing.
//!
//! Supports 23 programming languages. Each file is parsed into
//! [`ParsedSymbol`]s (functions, classes, etc.), [`CallReference`]s, and
//! [`ImportInfo`] records that feed the call-graph resolver.
//!
//! Layout: this file holds the public types, the language→grammar table
//! ([`SourceLanguage::tree_sitter_language`]) and the per-language dispatch
//! in [`SourceParser::parse`]. Each submodule adds the `extract_*` methods for
//! one language family to `SourceParser`, with its tests alongside; `common`
//! holds the helpers they share.

use anyhow::{Context, Result};
use myceliums_storage::{CodeSymbol, SymbolKind, SymbolMetadata};
use std::borrow::Cow;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use tree_sitter::{Language, ParseOptions, ParseState, Parser, Tree};
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
    ///
    /// This is the single language→grammar table: [`SourceParser::new`] and
    /// `myc doctor` both resolve grammars through it, so a language added here
    /// is parsed and health-checked without touching any other crate.
    pub fn tree_sitter_language(&self) -> Option<Language> {
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

/// Default cap on the wall-clock budget for one tree-sitter parse, matching
/// the `analysis.parse_timeout_secs` default. See [`parse_budget`].
pub const DEFAULT_PARSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed part of the per-file parse budget (see [`parse_budget`]): what a
/// file of a few bytes gets. Parsing a small file takes well under a
/// millisecond even unoptimized; the rest is headroom for a loaded machine.
pub const PARSE_BUDGET_BASE: Duration = Duration::from_millis(500);

/// Per-KiB part of the per-file parse budget (see [`parse_budget`]). Real
/// code parses at 0.15-1.2 ms/KiB in an unoptimized build (measured on
/// Python, Go, TypeScript, C and Rust files of 60-170 KiB), several times
/// faster optimized, so this leaves a margin of over 15x.
pub const PARSE_BUDGET_PER_KIB: Duration = Duration::from_millis(20);

/// Wall-clock budget for parsing a source of `source_len` bytes:
/// [`PARSE_BUDGET_BASE`] plus [`PARSE_BUDGET_PER_KIB`] per started KiB,
/// capped at `cap` (`analysis.parse_timeout_secs`).
///
/// The budget grows with the file because parse time does: a tiny file gets a
/// tiny budget, so tree-sitter's error recovery cannot stall the indexer for
/// the full cap on 100 bytes of garbage (#127), while a large real file still
/// gets the time it needs.
pub fn parse_budget(source_len: usize, cap: Duration) -> Duration {
    let kib = u32::try_from(source_len.div_ceil(1024)).unwrap_or(u32::MAX);
    PARSE_BUDGET_BASE
        .saturating_add(PARSE_BUDGET_PER_KIB.saturating_mul(kib))
        .min(cap)
}

/// Deepest syntax tree the extractors walk. They recurse once per tree level,
/// so a deeper tree is refused with [`ParseAbort::TooDeep`] rather than given a
/// stack of `MAX_TREE_DEPTH * STACK_BYTES_PER_LEVEL` bytes or more. Real code
/// stays far below this; garbage such as a long run of `(` does not, since a
/// tree's depth is bounded only by the file's size.
pub const MAX_TREE_DEPTH: usize = 10_000;

/// Trees up to this depth are walked on the caller's thread, whose stack is
/// unknown (2 MiB for rayon workers and test threads). In an unoptimized build
/// the extractors use up to ~11 KiB of stack per tree level (C++ nested calls;
/// most languages overflow a fresh 2 MiB stack at depth 130-260), so this
/// keeps the inline case under ~1 MiB.
const INLINE_TREE_DEPTH: usize = 64;

/// Stack reserved per tree level for deeper trees. The reservation is virtual
/// memory; only the pages the walk touches are committed, but it is reserved
/// per extracting worker: about three times the worst per-level cost measured
/// in an unoptimized build (~11 KiB). The same size is used in release builds:
/// optimized frame sizes were not measured and no CI job exercises the
/// release path, so a smaller release value would be an untested bet whose
/// failure mode (stack overflow) aborts the whole indexer. At
/// [`MAX_TREE_DEPTH`] this reserves ~315 MiB of address space per worker.
const STACK_BYTES_PER_LEVEL: usize = 32 * 1024;

/// Why [`SourceParser`] gave up on a file without extracting anything.
///
/// Returned inside the `anyhow::Error`, so callers can tell a budget skip from
/// other failures with `err.downcast_ref::<ParseAbort>()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseAbort {
    /// Tree-sitter did not finish within the parse time budget.
    Timeout(Duration),
    /// The syntax tree is nested deeper than [`MAX_TREE_DEPTH`].
    TooDeep {
        /// Depth of the tree that was refused.
        depth: usize,
    },
}

impl std::fmt::Display for ParseAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(budget) => write!(f, "parsing took longer than {budget:?}"),
            Self::TooDeep { depth } => write!(
                f,
                "syntax tree is {depth} levels deep (limit {MAX_TREE_DEPTH})"
            ),
        }
    }
}

impl std::error::Error for ParseAbort {}

/// Tree-sitter parser wrapper for a specific [`SourceLanguage`].
pub struct SourceParser {
    parser: Parser,
    lang: SourceLanguage,
    timeout: Option<Duration>,
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
    /// Constructor call (e.g. `new Foo`), or a struct/composite literal
    /// (`Foo { .. }`, `&Foo{}`): an instance of the named type.
    New(String),
    /// A literal (string, number, list, map, ...): a value of a built-in
    /// type, never an instance of a project class.
    Literal,
}

/// How a call site names its target, as far as the extractor records it
/// (#139). The resolver uses it to tell a call of a free function from a
/// method call: `f()` never calls a method in Python, TypeScript, Go or Rust,
/// and `x.f()` never calls a free function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallForm {
    /// The extractor does not say; the resolver matches by name alone.
    #[default]
    Unknown,
    /// `f(...)`: a bare name, with no receiver or qualifier.
    Bare,
    /// `x.f(...)`, `X::f(...)`, `pkg.F(...)`: the receiver is in the chain.
    Member,
    /// `new X(...)`: the callee is the class.
    New,
    /// `super(...)`: the base class's constructor.
    SuperConstructor,
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
#[derive(Debug, Clone, Default)]
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
    /// How the call site names its target.
    pub form: CallForm,
    /// Number of arguments, where the extractor counts them (selects among
    /// overloads).
    pub arg_count: Option<u32>,
}

/// What a name in the code is bound to, for typing call receivers (#139): a
/// parameter, a local variable or a field, with its declared type or the
/// expression it was initialised from. No type inference: just what the
/// declaration says.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeBinding {
    /// The parameter, variable or field name.
    pub name: String,
    /// For a field (`self.repo`, `this.repo`, a Java, Rust or Go field): the
    /// class or struct it belongs to. `None` for a parameter or a variable.
    pub owner: Option<String>,
    /// Lines the binding is visible in (1-based, inclusive): the function
    /// for a parameter or local, the class for a declared field, the
    /// assigning method for a field set by assignment, the whole file for a
    /// module-level name.
    pub scope: (u32, u32),
    /// What it is bound to.
    pub value: BindingValue,
}

/// The value side of a [`TypeBinding`].
#[derive(Debug, Clone, PartialEq)]
pub enum BindingValue {
    /// A declared type, as written (`Repository<User>`, `*store.Repo[*User]`,
    /// `&mut Repo<User>`).
    Type(String),
    /// The expression it was assigned from, as chain steps (`User(...)` is
    /// `[Ident(User), Call]`).
    Expr(Vec<ExpressionStep>),
    /// Bound, of unknown type: an untyped parameter, a closure, a loop
    /// variable. It still shadows outer names.
    Unknown,
}

/// What the call resolver needs to know about a definition beyond its
/// symbol (#139): its declared return type and base types. Kept apart from
/// [`SymbolMetadata`], which is indexed for search, for languages whose
/// extractors do not record these there (Go, Java), and for Rust trait
/// implementations, which are declared apart from their type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeclaredTypes {
    /// The definition's name and first line, as in its [`ParsedSymbol`].
    pub name: String,
    pub line: u32,
    /// Declared return type, as written (`(*Config, error)`, `Config`).
    pub return_type: Option<String>,
    /// Base classes, implemented interfaces, embedded structs.
    pub bases: Vec<String>,
    /// The definition named `name` is declared elsewhere, maybe in another
    /// file (a Rust `impl Trait for Type`): `bases` apply to it and are
    /// resolved where `line` is, in this file.
    pub elsewhere: bool,
}

/// The whole file, as a [`TypeBinding::scope`].
pub const FILE_SCOPE: (u32, u32) = (0, u32::MAX);

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
    /// Parameters, locals and fields with what they are bound to (Python,
    /// TypeScript/JavaScript, Go, Rust and Java).
    pub bindings: Vec<TypeBinding>,
    /// Return and base types of definitions (Go, Java).
    pub declared: Vec<DeclaredTypes>,
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
        Ok(Self {
            parser,
            lang,
            timeout: Some(DEFAULT_PARSE_TIMEOUT),
        })
    }

    /// Set the cap on the wall-clock budget of each tree-sitter parse
    /// (`None`: no limit; default [`DEFAULT_PARSE_TIMEOUT`]). Each parse gets
    /// [`parse_budget`] for its source size, at most `timeout`; a parse that
    /// runs over fails with [`ParseAbort::Timeout`].
    ///
    /// Tree-sitter's error recovery never finishes on some inputs of a dozen
    /// bytes (#127); this bounds them.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// The source tree-sitter sees: the `<script>` block for Vue/Svelte, the
    /// file itself otherwise.
    fn effective_source<'a>(&self, source: &'a str) -> Cow<'a, str> {
        if matches!(self.lang, SourceLanguage::Vue | SourceLanguage::Svelte) {
            Cow::Owned(Self::extract_script_block(source))
        } else {
            Cow::Borrowed(source)
        }
    }

    /// Parse `source` into a tree, within the time budget.
    fn parse_tree(&mut self, source: &str) -> Result<Tree> {
        let Some(cap) = self.timeout else {
            return self
                .parser
                .parse(source, None)
                .context("Failed to parse source");
        };
        let budget = parse_budget(source.len(), cap);
        let bytes = source.as_bytes();
        let start = Instant::now();
        let mut timed_out = false;
        let mut progress = |_: &ParseState| {
            if start.elapsed() > budget {
                timed_out = true;
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let tree = self.parser.parse_with_options(
            &mut |offset, _| bytes.get(offset..).unwrap_or_default(),
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        match tree {
            Some(tree) => Ok(tree),
            None if timed_out => {
                // A cancelled parse is kept for resumption; drop it so the
                // next call starts from scratch.
                self.parser.reset();
                Err(ParseAbort::Timeout(budget).into())
            }
            None => anyhow::bail!("Failed to parse source"),
        }
    }

    /// Run `extract` (a recursive walk of `tree`) on a stack deep enough for
    /// the tree: inline for ordinary trees, on a scoped thread with a stack
    /// sized to the depth for deep ones, and not at all beyond
    /// [`MAX_TREE_DEPTH`]. A panic inside `extract` propagates unchanged.
    fn with_stack_for<T: Send>(tree: &Tree, extract: impl FnOnce() -> T + Send) -> Result<T> {
        let depth = tree_depth(tree);
        if depth <= INLINE_TREE_DEPTH {
            return Ok(extract());
        }
        if depth > MAX_TREE_DEPTH {
            return Err(ParseAbort::TooDeep { depth }.into());
        }
        let stack_size = (depth + INLINE_TREE_DEPTH) * STACK_BYTES_PER_LEVEL;
        std::thread::scope(|scope| {
            let handle = std::thread::Builder::new()
                .name("myc-deep-extract".into())
                .stack_size(stack_size)
                .spawn_scoped(scope, extract)
                .context("Failed to spawn extraction thread")?;
            match handle.join() {
                Ok(value) => Ok(value),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        })
    }

    /// Parse using the DSL engine if rules are available for this language.
    /// Falls back to the hand-coded path if no DSL rules exist.
    pub fn parse_with_dsl(&mut self, source: &str) -> Result<ParseResult> {
        let Some(rules) = crate::dsl::rules_for(self.lang) else {
            // Fall back to hand-coded path
            return self.parse(source);
        };
        let parse_source = self.effective_source(source);
        let tree = self.parse_tree(&parse_source)?;
        let source_bytes = parse_source.as_bytes();

        Self::with_stack_for(&tree, || {
            let root = tree.root_node();
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
                    ..Default::default()
                })
                .collect();

            let rationales = Self::extract_rationales(root, source_bytes);

            ParseResult {
                symbols,
                calls,
                imports,
                rationales,
                aliases: vec![],
                bindings: vec![],
                declared: vec![],
            }
        })
    }

    pub fn parse(&mut self, source: &str) -> Result<ParseResult> {
        // For Vue/Svelte, extract the <script> block and parse as TypeScript
        let parse_source = self.effective_source(source);
        let tree = self.parse_tree(&parse_source)?;
        let source_bytes = parse_source.as_bytes();
        let this = &*self;
        Self::with_stack_for(&tree, || this.extract(tree.root_node(), source_bytes))
    }

    /// Run the hand-coded extractors for this language over a parsed tree.
    fn extract(&self, root: tree_sitter::Node, source_bytes: &[u8]) -> ParseResult {
        let mut symbols = Vec::new();
        let mut calls = Vec::new();
        let mut imports = Vec::new();
        let mut aliases = Vec::new();
        let mut bindings = Vec::new();
        let mut declared = Vec::new();

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
                self.extract_ts_bindings(root, source_bytes, &mut bindings, None, None);
            }
            SourceLanguage::Python => {
                self.extract_py_symbols(root, source_bytes, &mut symbols, None);
                self.extract_py_calls(root, source_bytes, &mut calls, None);
                self.extract_py_imports(root, source_bytes, &mut imports);
                self.extract_py_aliases(root, source_bytes, &mut aliases);
                self.extract_py_bindings(root, source_bytes, &mut bindings, None, None);
            }
            SourceLanguage::Go => {
                self.extract_go_symbols(root, source_bytes, &mut symbols, None);
                self.extract_go_calls(root, source_bytes, &mut calls, None);
                self.extract_go_imports(root, source_bytes, &mut imports);
                self.extract_go_bindings(root, source_bytes, &mut bindings, None);
                self.extract_go_declared_types(root, source_bytes, &mut declared);
            }
            SourceLanguage::Rust => {
                self.extract_rust_symbols(root, source_bytes, &mut symbols, None);
                self.extract_rust_calls(root, source_bytes, &mut calls, None);
                self.extract_rust_imports(root, source_bytes, &mut imports);
                self.extract_rust_bindings(root, source_bytes, &mut bindings, None);
                self.extract_rust_trait_impls(root, source_bytes, &mut declared);
            }
            SourceLanguage::Java => {
                self.extract_java_symbols(root, source_bytes, &mut symbols, None);
                self.extract_java_calls(root, source_bytes, &mut calls, None);
                self.extract_java_imports(root, source_bytes, &mut imports);
                self.extract_java_bindings(root, source_bytes, &mut bindings, None, None);
                self.extract_java_declared_types(root, source_bytes, &mut declared);
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

        ParseResult {
            symbols,
            calls,
            imports,
            rationales,
            aliases,
            bindings,
            declared,
        }
    }
}

/// Maximum depth of `tree` (the root is depth 0), found without recursion.
fn tree_depth(tree: &Tree) -> usize {
    let mut cursor = tree.walk();
    let (mut depth, mut max_depth) = (0usize, 0usize);
    loop {
        if cursor.goto_first_child() {
            depth += 1;
            max_depth = max_depth.max(depth);
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return max_depth;
            }
            depth -= 1;
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

    /// The exported grammar table covers exactly the languages that are not
    /// content files or notebooks; `myc doctor` relies on it for every code
    /// language it checks.
    #[test]
    fn grammar_table_covers_exactly_the_code_languages() {
        let exts = [
            "ts", "tsx", "js", "py", "go", "rs", "java", "cs", "c", "cpp", "rb", "kt", "swift",
            "php", "lua", "zig", "ps1", "ex", "scala", "m", "dart", "vue", "svelte", "md", "mdx",
            "txt", "ipynb", "eml", "mbox",
        ];
        for ext in exts {
            let lang = SourceLanguage::from_extension(ext).unwrap();
            let is_code = !lang.is_content() && lang != SourceLanguage::Jupyter;
            assert_eq!(
                lang.tree_sitter_language().is_some(),
                is_code,
                "{}",
                lang.name()
            );
        }
    }
}
