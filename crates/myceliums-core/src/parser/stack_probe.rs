//! Measures how much stack the recursive extractors use per tree level (#159).
//!
//! [`STACK_BYTES_PER_LEVEL`] sizes the thread a deep tree is walked on, and
//! optimized frames are much smaller than unoptimized ones, so the constant
//! has to be checked in the build profile it ships in. This probe finds, for
//! each language, extraction path (hand-coded and DSL) and deeply nested
//! input shape, the smallest thread stack the walk survives, by binary search
//! over stack sizes. A stack overflow aborts the whole process, so every
//! attempt runs in a child process: this test binary re-executed with only
//! [`stack_probe_child`] selected and the attempt described in an environment
//! variable.
//!
//! The result is expressed in the unit [`SourceParser::with_stack_for`] uses:
//! minimum stack / (tree depth + [`INLINE_TREE_DEPTH`]). The test fails unless
//! [`STACK_BYTES_PER_LEVEL`] is at least [`REQUIRED_MARGIN`] times the worst
//! value, so a regression in frame sizes fails CI instead of crashing the
//! indexer.
//!
//! Both tests are `#[ignore]`d: the probe spawns thousands of processes and
//! only means something per build profile. Run it with
//!
//! ```text
//! cargo test -p myceliums-core --no-default-features --release --lib \
//!     parser::stack_probe::measure_stack_per_tree_level -- --ignored --exact --nocapture
//! ```
//!
//! (and without `--release` for the debug numbers). With
//! `GITHUB_STEP_SUMMARY` set, the table is appended to that file as well.

use super::{SourceLanguage, SourceParser, INLINE_TREE_DEPTH, STACK_BYTES_PER_LEVEL};
use std::io::Write;
use std::process::Command;
use std::sync::Mutex;

/// Environment variable that turns [`stack_probe_child`] into one attempt:
/// `<language index>:<shape index>:<nesting>:<hand|dsl>:<stack bytes>`.
const CHILD_ENV: &str = "MYC_STACK_PROBE_CHILD";

/// [`STACK_BYTES_PER_LEVEL`] must be at least this many times the worst
/// measured per-level cost.
const REQUIRED_MARGIN: usize = 2;

/// Nesting levels of the generated inputs: a shallow tree, where the fixed
/// cost of the walk weighs most per level, and a deep one, where the
/// per-level frames dominate.
const NESTINGS: [usize; 2] = [100, 2_000];

/// Binary search stops when the bracket is this tight (relative)...
const PRECISION: f64 = 0.03;
/// ...or this tight in bytes.
const PRECISION_BYTES: usize = 4 * 1024;

/// Every language that goes through tree-sitter.
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

/// Names of the input shapes built by [`deep_input`].
const SHAPES: [&str; 5] = ["(x)", "[x]", "{}", "x + x", "f(x)"];

/// Input nested `n` levels deep in shape `shape` (the shapes of
/// `tests/extractor_budgets.rs`, which reach depth in every grammar).
fn deep_input(lang: SourceLanguage, shape: usize, n: usize) -> String {
    let body = match shape {
        0 => format!("{}x{}", "(".repeat(n), ")".repeat(n)),
        1 => format!("{}x{}", "[".repeat(n), "]".repeat(n)),
        2 => format!("{}{}", "{".repeat(n), "}".repeat(n)),
        3 => format!("x{}", " + x".repeat(n)),
        4 => format!("{}x{}", "f(".repeat(n), ")".repeat(n)),
        _ => unreachable!("shape {shape}"),
    };
    if matches!(lang, SourceLanguage::Vue | SourceLanguage::Svelte) {
        format!("<script>{body}</script>")
    } else {
        body
    }
}

/// A parser for `lang`, the source it parses and its tree for the input.
fn parse_input(
    lang: SourceLanguage,
    shape: usize,
    n: usize,
) -> (SourceParser, String, tree_sitter::Tree) {
    let mut parser = SourceParser::new(lang).unwrap();
    parser.set_timeout(None);
    let input = deep_input(lang, shape, n);
    let source = parser.effective_source(&input).into_owned();
    let tree = parser.parse_tree(&source).unwrap();
    (parser, source, tree)
}

/// One attempt: parse the input on this thread, then walk it on a fresh
/// thread with exactly `stack` bytes of stack.
fn extract_on_stack(lang: SourceLanguage, shape: usize, n: usize, dsl: bool, stack: usize) {
    let (parser, source, tree) = parse_input(lang, shape, n);
    let bytes = source.as_bytes();
    let parser = &parser;
    let tree = &tree;
    std::thread::scope(|scope| {
        let walk = std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(scope, move || {
                let root = tree.root_node();
                let result = if dsl {
                    let rules = crate::dsl::rules_for(lang).unwrap();
                    SourceParser::extract_with_dsl(&rules, root, bytes)
                } else {
                    parser.extract(root, bytes)
                };
                std::hint::black_box(result);
            })
            .unwrap();
        walk.join().unwrap();
    });
}

/// Child side of the probe: does nothing unless [`CHILD_ENV`] is set.
#[test]
#[ignore = "child process of measure_stack_per_tree_level"]
fn stack_probe_child() {
    let Ok(spec) = std::env::var(CHILD_ENV) else {
        return;
    };
    let fields: Vec<&str> = spec.split(':').collect();
    let &[lang, shape, n, path, stack] = fields.as_slice() else {
        panic!("bad {CHILD_ENV}: {spec}");
    };
    extract_on_stack(
        LANGUAGES[lang.parse::<usize>().unwrap()],
        shape.parse().unwrap(),
        n.parse().unwrap(),
        path == "dsl",
        stack.parse().unwrap(),
    );
}

/// Whether the walk survives a `stack`-byte thread. Panics on any failure
/// other than a stack overflow, so a broken child cannot pass for a small
/// stack.
fn survives(lang: usize, shape: usize, n: usize, dsl: bool, stack: usize) -> bool {
    let path = if dsl { "dsl" } else { "hand" };
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "parser::stack_probe::stack_probe_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
            "--quiet",
        ])
        .env(CHILD_ENV, format!("{lang}:{shape}:{n}:{path}:{stack}"))
        .output()
        .expect("spawn probe child");
    if output.status.success() {
        return true;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has overflowed its stack"),
        "probe child {} shape {shape} n {n} {path} stack {stack} failed without a stack overflow ({}):\n{stderr}",
        LANGUAGES[lang].name(),
        output.status,
    );
    false
}

/// One measured combination.
struct Measurement {
    lang: usize,
    shape: usize,
    n: usize,
    dsl: bool,
    depth: usize,
    /// Smallest stack (within the search precision) the walk survived.
    min_stack: usize,
}

impl Measurement {
    /// Bytes per level in the unit of `STACK_BYTES_PER_LEVEL`.
    fn per_level(&self) -> usize {
        self.min_stack.div_ceil(self.depth + INLINE_TREE_DEPTH)
    }
}

/// Smallest surviving stack for one combination, by geometric bisection
/// between a size that fails (or the floor) and one that survives.
fn min_stack(lang: usize, shape: usize, n: usize, dsl: bool, depth: usize) -> usize {
    let floor = 16 * 1024;
    // Four times what production would reserve: comfortably survivable,
    // unless the reserve itself is far too small, which the caller reports.
    let mut hi = 4 * (depth + INLINE_TREE_DEPTH) * STACK_BYTES_PER_LEVEL;
    assert!(
        survives(lang, shape, n, dsl, hi),
        "{} shape {} n {n} dsl {dsl}: overflowed even {hi} bytes (4x the production reserve)",
        LANGUAGES[lang].name(),
        SHAPES[shape],
    );
    if survives(lang, shape, n, dsl, floor) {
        return floor;
    }
    let mut lo = floor;
    while hi - lo > PRECISION_BYTES && (hi as f64) > (lo as f64) * (1.0 + PRECISION) {
        let mid = ((lo as f64) * (hi as f64)).sqrt() as usize;
        let mid = mid.clamp(lo + 1, hi - 1);
        if survives(lang, shape, n, dsl, mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The probe: measures every combination, prints a table per language and
/// asserts the reserve keeps [`REQUIRED_MARGIN`] over the worst case.
#[test]
#[ignore = "spawns thousands of processes; run per build profile (see module docs)"]
fn measure_stack_per_tree_level() {
    // Jobs: every combination whose tree is deep enough to leave the inline path.
    let mut jobs = Vec::new();
    for (lang_index, &lang) in LANGUAGES.iter().enumerate() {
        let has_dsl = crate::dsl::rules_for(lang).is_some();
        for shape in 0..SHAPES.len() {
            for n in NESTINGS {
                let depth = super::tree_depth(&parse_input(lang, shape, n).2);
                if depth <= INLINE_TREE_DEPTH {
                    continue;
                }
                for dsl in [false, true] {
                    if dsl && !has_dsl {
                        continue;
                    }
                    jobs.push((lang_index, shape, n, dsl, depth));
                }
            }
        }
    }
    let total = jobs.len();
    let jobs = Mutex::new(jobs);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((lang, shape, n, dsl, depth)) = jobs.lock().unwrap().pop() else {
                    break;
                };
                let min_stack = min_stack(lang, shape, n, dsl, depth);
                results.lock().unwrap().push(Measurement {
                    lang,
                    shape,
                    n,
                    dsl,
                    depth,
                    min_stack,
                });
            });
        }
    });
    let results = results.into_inner().unwrap();
    assert_eq!(results.len(), total);

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let mut table = format!(
        "### Extractor stack per tree level ({profile}, {}-{})\n\n\
         Minimum thread stack the walk survives, per (tree depth + {INLINE_TREE_DEPTH}) — \
         the unit of `STACK_BYTES_PER_LEVEL`. Worst over {} input shapes, nesting {NESTINGS:?}, \
         per extraction path. Slope: extra stack per extra level between the two nestings.\n\n\
         | Language | hand B/level | DSL B/level | worst input | depth | min stack | slope B/level |\n\
         |---|---:|---:|---|---:|---:|---:|\n",
        std::env::consts::ARCH,
        std::env::consts::OS,
        SHAPES.len(),
    );
    let mut worst_overall: Option<&Measurement> = None;
    for (lang_index, lang) in LANGUAGES.iter().enumerate() {
        let of_lang: Vec<&Measurement> = results.iter().filter(|m| m.lang == lang_index).collect();
        let worst_of = |dsl: bool| {
            of_lang
                .iter()
                .filter(|m| m.dsl == dsl)
                .max_by_key(|m| m.per_level())
                .copied()
        };
        let hand = worst_of(false);
        let dsl = worst_of(true);
        let worst = [hand, dsl]
            .into_iter()
            .flatten()
            .max_by_key(|m| m.per_level())
            .expect("every language has a deep input");
        // Slope of the worst (shape, path) between the two nestings.
        let pair: Vec<&&Measurement> = of_lang
            .iter()
            .filter(|m| m.shape == worst.shape && m.dsl == worst.dsl)
            .collect();
        let slope = match pair[..] {
            [a, b] if a.depth != b.depth => {
                let (lo, hi) = if a.depth < b.depth { (a, b) } else { (b, a) };
                (hi.min_stack.saturating_sub(lo.min_stack) / (hi.depth - lo.depth)).to_string()
            }
            _ => "-".into(),
        };
        let cell = |m: Option<&Measurement>| m.map_or("-".into(), |m| m.per_level().to_string());
        table.push_str(&format!(
            "| {} | {} | {} | `{}` {} n={} | {} | {} KiB | {} |\n",
            lang.name(),
            cell(hand),
            cell(dsl),
            SHAPES[worst.shape],
            if worst.dsl { "DSL" } else { "hand" },
            worst.n,
            worst.depth,
            worst.min_stack.div_ceil(1024),
            slope,
        ));
        if worst_overall.is_none_or(|w| worst.per_level() > w.per_level()) {
            worst_overall = Some(worst);
        }
    }
    let worst = worst_overall.expect("at least one language");
    let ratio = STACK_BYTES_PER_LEVEL as f64 / worst.per_level() as f64;
    table.push_str(&format!(
        "\nWorst: {} B/level ({}). `STACK_BYTES_PER_LEVEL` = {} B: {ratio:.1}x the worst \
         (required: {REQUIRED_MARGIN}x).\n",
        worst.per_level(),
        LANGUAGES[worst.lang].name(),
        STACK_BYTES_PER_LEVEL,
    ));
    println!("{table}");
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        writeln!(file, "{table}").unwrap();
    }
    assert!(
        STACK_BYTES_PER_LEVEL >= REQUIRED_MARGIN * worst.per_level(),
        "STACK_BYTES_PER_LEVEL ({STACK_BYTES_PER_LEVEL}) is less than {REQUIRED_MARGIN}x the \
         worst measured per-level stack ({} B, {}); raise it",
        worst.per_level(),
        LANGUAGES[worst.lang].name(),
    );
}
