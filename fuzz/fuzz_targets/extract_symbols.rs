//! Run the tree-sitter symbol extractors on arbitrary bytes. The first byte
//! selects the language; the rest is the file content, decoded lossily as the
//! analyzer does. Any panic is a bug: extraction runs on arbitrary
//! repositories and must never take the indexer down.
#![no_main]

use libfuzzer_sys::fuzz_target;
use myceliums_core::parser::{to_code_symbols, ParseResult, SourceLanguage, SourceParser};

const LANGUAGES: &[SourceLanguage] = &[
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

fn check(result: &ParseResult) {
    for sym in &result.symbols {
        assert!(sym.start_line <= sym.end_line, "{sym:?}");
    }
    let _ = to_code_symbols(&result.symbols, "src/fuzz", "fuzz");
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let lang = LANGUAGES[selector as usize % LANGUAGES.len()];
    let source = String::from_utf8_lossy(rest);

    let mut parser = SourceParser::new(lang).expect("tree-sitter language");
    if let Ok(result) = parser.parse(&source) {
        check(&result);
    }
    let mut parser = SourceParser::new(lang).expect("tree-sitter language");
    if let Ok(result) = parser.parse_with_dsl(&source) {
        check(&result);
    }
});
