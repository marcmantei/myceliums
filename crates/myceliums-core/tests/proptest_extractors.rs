//! Property tests for the tree-sitter symbol extractors (#91).
//!
//! The extractors run on arbitrary repositories, so for every tree-sitter
//! language they must never panic, whatever bytes a file contains. Two input
//! shapes are generated: raw bytes (decoded lossily, as the analyzer would
//! after reading a file) and "code-shaped" text assembled from tokens common
//! to the supported languages, which reaches far deeper into the extractors
//! than random bytes do.
//!
//! Case counts are kept small so the suite stays fast; the cargo-fuzz targets
//! in `fuzz/` explore the same surface for much longer.

use myceliums_core::parser::{to_code_symbols, ParseResult, SourceLanguage, SourceParser};
use proptest::prelude::*;

/// Every language that goes through `SourceParser` (tree-sitter).
const TREE_SITTER_LANGUAGES: &[SourceLanguage] = &[
    SourceLanguage::TypeScript,
    SourceLanguage::Tsx,
    SourceLanguage::JavaScript,
    SourceLanguage::Python,
    SourceLanguage::Go,
    SourceLanguage::Rust,
    SourceLanguage::Java,
    SourceLanguage::CSharp,
    SourceLanguage::C,
    SourceLanguage::Cpp,
    SourceLanguage::Ruby,
    SourceLanguage::Kotlin,
    SourceLanguage::Swift,
    SourceLanguage::Php,
    SourceLanguage::Lua,
    SourceLanguage::Zig,
    SourceLanguage::PowerShell,
    SourceLanguage::Elixir,
    SourceLanguage::Scala,
    SourceLanguage::ObjectiveC,
    SourceLanguage::Dart,
    SourceLanguage::Vue,
    SourceLanguage::Svelte,
];

/// Tokens that occur across the supported languages; random sequences of
/// them produce plausible-looking (often half-valid) declarations.
const CODE_TOKENS: &[&str] = &[
    "fn",
    "def",
    "func",
    "function",
    "class",
    "struct",
    "interface",
    "impl",
    "trait",
    "enum",
    "module",
    "package",
    "import",
    "from",
    "use",
    "require",
    "return",
    "pub",
    "public",
    "private",
    "static",
    "async",
    "await",
    "let",
    "var",
    "val",
    "const",
    "self",
    "this",
    "new",
    "end",
    "do",
    "if",
    "else",
    "for",
    "while",
    "@",
    "#",
    "//",
    "/*",
    "*/",
    "///",
    "\"\"\"",
    "'",
    "\"",
    "`",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "<",
    ">",
    ":",
    "::",
    ";",
    ",",
    ".",
    "->",
    "=>",
    "=",
    "\n",
    "\n    ",
    "\t",
    " ",
    "foo",
    "Bar",
    "baz_qux",
    "x",
    "0",
    "42",
    "<script>",
    "</script>",
    "<script lang=\"ts\">",
    "$x",
    "%{",
    "|>",
    "?",
    "!",
    "&",
    "*",
    "ü",
    "日本",
    "🙂",
    "\u{0}",
    "\r\n",
];

fn code_shaped() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(CODE_TOKENS), 0..120).prop_map(|t| t.concat())
}

fn run_extractors(lang: SourceLanguage, source: &str) {
    let mut parser = SourceParser::new(lang).expect("tree-sitter language initialises");
    if let Ok(result) = parser.parse(source) {
        check_invariants(lang, source, &result);
    }
    let mut parser = SourceParser::new(lang).expect("tree-sitter language initialises");
    if let Ok(result) = parser.parse_with_dsl(source) {
        check_invariants(lang, source, &result);
    }
}

/// Structural invariants that must hold for any extraction result.
fn check_invariants(lang: SourceLanguage, source: &str, result: &ParseResult) {
    // Vue/Svelte line numbers refer to the extracted <script> block, which
    // can never be longer than the file itself.
    let line_count = source.split('\n').count() as u32;
    for sym in &result.symbols {
        assert!(
            sym.start_line <= sym.end_line,
            "{}: symbol {:?} starts after it ends ({}..{})",
            lang.name(),
            sym.name,
            sym.start_line,
            sym.end_line
        );
        assert!(
            sym.end_line <= line_count + 1,
            "{}: symbol {:?} ends on line {} of a {}-line file",
            lang.name(),
            sym.name,
            sym.end_line,
            line_count
        );
    }
    let code_symbols = to_code_symbols(&result.symbols, "src/fuzz", "repo");
    assert_eq!(code_symbols.len(), result.symbols.len());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Raw bytes, decoded the way the analyzer decodes files it reads.
    #[test]
    fn extractors_never_panic_on_arbitrary_bytes(
        lang in prop::sample::select(TREE_SITTER_LANGUAGES),
        bytes in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        run_extractors(lang, &String::from_utf8_lossy(&bytes));
    }

    /// Arbitrary Unicode text.
    #[test]
    fn extractors_never_panic_on_arbitrary_text(
        lang in prop::sample::select(TREE_SITTER_LANGUAGES),
        source in any::<String>(),
    ) {
        run_extractors(lang, &source);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Code-shaped token soup, which exercises declaration/call/import paths.
    #[test]
    fn extractors_never_panic_on_code_shaped_text(
        lang in prop::sample::select(TREE_SITTER_LANGUAGES),
        source in code_shaped(),
    ) {
        run_extractors(lang, &source);
    }
}

/// Regression (#91): constructing a `SourceParser` for a language that is not
/// parsed with tree-sitter used to hit `unreachable!()`; it now returns an
/// error the caller can handle.
#[test]
fn source_parser_rejects_non_tree_sitter_languages() {
    for lang in [
        SourceLanguage::Markdown,
        SourceLanguage::Mdx,
        SourceLanguage::PlainText,
        SourceLanguage::Jupyter,
        SourceLanguage::Email,
        SourceLanguage::Mbox,
    ] {
        let err = SourceParser::new(lang)
            .err()
            .unwrap_or_else(|| panic!("{} should be rejected", lang.name()));
        assert!(err.to_string().contains(lang.name()), "{err}");
    }
}

/// Every tree-sitter language initialises and parses an empty file.
#[test]
fn every_tree_sitter_language_parses_empty_input() {
    for &lang in TREE_SITTER_LANGUAGES {
        run_extractors(lang, "");
    }
}
