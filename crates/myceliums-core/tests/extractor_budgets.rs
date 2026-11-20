//! Time and stack budgets of the tree-sitter extraction (#127).
//!
//! The extractors run on arbitrary repository files, so neither a file's
//! content nor its nesting may stall or crash the indexer:
//!
//! - tree-sitter's error recovery never finishes on some short inputs: 0.25 on
//!   the first fuzz input from #127 (fixed upstream in 0.26), 0.26.13 still on
//!   later ones (Objective-C, 12 bytes minimized). Each parse is bounded by a
//!   time budget that grows with the file's size ([`parse_budget`]), capped by
//!   [`SourceParser::set_timeout`];
//! - the extractors recurse once per tree level, so deep trees (garbage such
//!   as a long run of `(`) overflowed a 2 MiB thread stack; they now run on a
//!   stack sized to the tree, and trees deeper than [`MAX_TREE_DEPTH`] are
//!   refused.

use std::time::{Duration, Instant};

use myceliums_core::parser::{
    parse_budget, ParseAbort, SourceLanguage, SourceParser, DEFAULT_PARSE_TIMEOUT, MAX_TREE_DEPTH,
    PARSE_BUDGET_BASE, PARSE_BUDGET_PER_KIB,
};

/// The `extract_symbols` fuzz input from #127: the first byte selects
/// Objective-C, the rest is the file (invalid UTF-8 included).
const ISSUE_127_INPUT: &[u8] = b"o<\x03improttempublic class A extend: B4 lp#emiments C { @Override \x88ublic voi; m() { hel\xe2elf) ->helpe&s\xdfelf) ->helper() } ";

/// The same stall reduced to 13 bytes of plain ASCII: invalid UTF-8 was not
/// needed to trigger it.
const ISSUE_127_MINIMIZED: &str = "<\x03p#s C{@v>e ";

/// Later `extract_symbols` fuzz inputs from #127 (CI Fuzz on PRs #132 and
/// #133, after the upgrade to tree-sitter 0.26.13), each with its first byte
/// (the language selector: Objective-C) and its 12-byte minimized form.
/// Tree-sitter never finishes any of them: the parse stops advancing through
/// the input (>60 s unoptimized for the full inputs, >30 s for the minimized
/// ones).
const ISSUE_127_STALLS: &[(&str, &[u8])] = &[
    (
        "PR #132 input",
        b"oi<\x03mrpottempublic class A extend: B4 lpimprottempublic class A extend: B4 lp#emiments C { @Override \x88ubli->helper() } ",
    ),
    ("PR #132 minimized", b"oi<p#m C{@i>)"),
    (
        "PR #133 input",
        b"or<mtupoitempublic clas\xf3 A extend: B4 lp#emiments C { @Ove { lf) ->helper() } ",
    ),
    ("PR #133 minimized", b"or<p#s C{@{f)"),
];

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

/// Run both extraction paths, as the fuzz target does, and return how long
/// they took together.
fn time_both_paths(lang: SourceLanguage, source: &str) -> Duration {
    let start = Instant::now();
    SourceParser::new(lang)
        .unwrap()
        .parse(source)
        .expect("parse succeeds");
    SourceParser::new(lang)
        .unwrap()
        .parse_with_dsl(source)
        .expect("parse_with_dsl succeeds");
    start.elapsed()
}

fn issue_127_source() -> String {
    String::from_utf8_lossy(&ISSUE_127_INPUT[1..]).into_owned()
}

#[test]
fn issue_127_input_parses_quickly() {
    // Before the fix this never returned (>120 s in a release build).
    let elapsed = time_both_paths(SourceLanguage::ObjectiveC, &issue_127_source());
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn issue_127_minimized_input_parses_quickly() {
    let elapsed = time_both_paths(SourceLanguage::ObjectiveC, ISSUE_127_MINIMIZED);
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

/// The budget is a small base plus a per-KiB allowance, never above the cap.
#[test]
fn parse_budget_grows_with_size_up_to_the_cap() {
    let cap = DEFAULT_PARSE_TIMEOUT;
    assert_eq!(parse_budget(0, cap), PARSE_BUDGET_BASE);
    assert_eq!(
        parse_budget(1, cap),
        PARSE_BUDGET_BASE + PARSE_BUDGET_PER_KIB
    );
    assert_eq!(
        parse_budget(1024, cap),
        PARSE_BUDGET_BASE + PARSE_BUDGET_PER_KIB
    );
    assert_eq!(
        parse_budget(100 * 1024, cap),
        PARSE_BUDGET_BASE + PARSE_BUDGET_PER_KIB * 100
    );
    assert_eq!(parse_budget(64 << 20, cap), cap);
    assert_eq!(parse_budget(usize::MAX, cap), cap);
    assert_eq!(
        parse_budget(10, Duration::from_millis(100)),
        Duration::from_millis(100)
    );
    assert_eq!(parse_budget(10, Duration::ZERO), Duration::ZERO);
    // A fuzz input (libFuzzer -max_len=4096) gets well under a second.
    assert!(parse_budget(4096, cap) < Duration::from_millis(600));
}

/// Regression (#127, second round): inputs on which tree-sitter 0.26.13 never
/// finishes are cut off by the size-aware budget (~0.5 s for these sizes),
/// not by the 30 s cap, so a tiny file cannot stall `analyze` and the fuzz
/// target stays far below libFuzzer's 10 s timeout. Also checks that
/// tree-sitter's progress callback fires often enough for the cancellation
/// to land close to the budget.
#[test]
fn issue_127_stalls_end_at_the_size_aware_budget() {
    for &(name, input) in ISSUE_127_STALLS {
        let source = String::from_utf8_lossy(&input[1..]).into_owned();
        let budget = parse_budget(source.len(), DEFAULT_PARSE_TIMEOUT);
        for dsl in [false, true] {
            let mut parser = SourceParser::new(SourceLanguage::ObjectiveC).unwrap();
            let start = Instant::now();
            let result = if dsl {
                parser.parse_with_dsl(&source)
            } else {
                parser.parse(&source)
            };
            let elapsed = start.elapsed();
            // Should tree-sitter ever finish these, a fast success is fine too.
            if let Err(err) = result {
                assert_eq!(
                    err.parse_abort(),
                    Some(&ParseAbort::Timeout(budget)),
                    "{name} (dsl: {dsl}): {err:#}"
                );
            }
            assert!(
                elapsed < budget + Duration::from_millis(750),
                "{name} (dsl: {dsl}): took {elapsed:?}, budget {budget:?}"
            );
        }
    }
}

#[test]
fn issue_127_input_scaled_up_stays_fast() {
    // 64 copies (~8 KB): measured at a few tens of milliseconds unoptimized.
    let source = issue_127_source().repeat(64);
    let elapsed = time_both_paths(SourceLanguage::ObjectiveC, &source);
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}

/// A parse that runs over its budget fails with `ParseAbort::Timeout`, and the
/// same parser parses normally afterwards (the cancelled parse is discarded,
/// not resumed).
#[test]
fn parse_over_budget_is_aborted_and_parser_recovers() {
    let source = "def f(x):\n    return g(x)\n\n".repeat(2_000);
    let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();

    parser.set_timeout(Some(Duration::ZERO));
    let err = parser.parse(&source).expect_err("zero budget must abort");
    assert_eq!(
        err.parse_abort(),
        Some(&ParseAbort::Timeout(Duration::ZERO)),
        "{err:#}"
    );
    let err = parser
        .parse_with_dsl(&source)
        .expect_err("zero budget must abort the DSL path too");
    assert!(
        matches!(err.parse_abort(), Some(ParseAbort::Timeout(_))),
        "{err:#}"
    );

    parser.set_timeout(None);
    let result = parser.parse(&source).expect("unbounded parse succeeds");
    assert_eq!(result.symbols.len(), 2_000);
}

/// Inputs whose syntax trees nest thousands of levels deep, in shapes that
/// reach that depth in every grammar (see the per-language assertion below).
fn deep_inputs(n: usize) -> Vec<String> {
    vec![
        format!("{}x{}", "(".repeat(n), ")".repeat(n)),
        format!("{}x{}", "[".repeat(n), "]".repeat(n)),
        format!("{}{}", "{".repeat(n), "}".repeat(n)),
        format!("x{}", " + x".repeat(n)),
        format!("{}x{}", "f(".repeat(n), ")".repeat(n)),
    ]
}

/// Depth of the tree tree-sitter builds for `source` (what the extractors
/// recurse over), computed without recursion.
fn tree_depth(lang: SourceLanguage, source: &str) -> usize {
    let source = if matches!(lang, SourceLanguage::Vue | SourceLanguage::Svelte) {
        format!("<script>{source}</script>")
    } else {
        source.to_string()
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.tree_sitter_language().unwrap())
        .unwrap();
    let tree = parser.parse(&source, None).unwrap();
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

/// Regression (#127 follow-up): a proptest run in CI died with a stack
/// overflow. Unoptimized, the recursive extractors overflowed a 2 MiB stack
/// at a tree depth of 130-260 in most languages (about 1000 in the most
/// frugal ones). Extraction now runs on a stack sized to the tree, so the
/// caller's 2 MiB is enough at any depth up to the limit.
#[test]
fn deep_trees_do_not_overflow_a_2_mib_stack() {
    let worker = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            for &lang in TREE_SITTER_LANGUAGES {
                let mut deepest = 0;
                for input in deep_inputs(1_200) {
                    let source = if matches!(lang, SourceLanguage::Vue | SourceLanguage::Svelte) {
                        format!("<script>{input}</script>")
                    } else {
                        input.clone()
                    };
                    deepest = deepest.max(tree_depth(lang, &input));
                    for result in [
                        SourceParser::new(lang).unwrap().parse(&source),
                        SourceParser::new(lang).unwrap().parse_with_dsl(&source),
                    ] {
                        // Either extracted, or refused as too deep: never a crash.
                        if let Err(err) = result {
                            assert!(
                                matches!(
                                    err.parse_abort(),
                                    Some(ParseAbort::TooDeep { .. })
                                ),
                                "{}: {err:#}",
                                lang.name()
                            );
                        }
                    }
                }
                assert!(
                    deepest >= 1_200,
                    "{}: no input reached depth 1200 (max {deepest}); the test does not exercise deep trees",
                    lang.name()
                );
            }
        })
        .unwrap();
    worker.join().expect("extraction on a 2 MiB stack");
}

#[test]
fn trees_deeper_than_the_limit_are_refused() {
    let n = MAX_TREE_DEPTH + 1_000;
    let source = format!("{}x{}", "(".repeat(n), ")".repeat(n));
    let depth = tree_depth(SourceLanguage::Python, &source);
    assert!(depth > MAX_TREE_DEPTH, "depth {depth}");

    for result in [
        SourceParser::new(SourceLanguage::Python)
            .unwrap()
            .parse(&source),
        SourceParser::new(SourceLanguage::Python)
            .unwrap()
            .parse_with_dsl(&source),
    ] {
        let err = result.expect_err("too deep");
        assert_eq!(
            err.parse_abort(),
            Some(&ParseAbort::TooDeep { depth }),
            "{err:#}"
        );
    }
}
