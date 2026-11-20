//! Classifying symbols as test or product code at index time (#146).
//!
//! Search ranks test code below product code by default, so the index records
//! which symbols are tests ([`myceliums_storage::SymbolMetadata::test`]). Two
//! sources decide it:
//!
//! - **The file's path**, by the conventions of the languages the parser
//!   reads ([`is_test_path`]) — applied to every symbol of the file.
//! - **Markers inside a Rust file** that is not a test file by path: an item
//!   compiled for tests only (`#[cfg(test)]` on a module, function, `impl` or
//!   type — with everything inside it) and a function carrying a test
//!   attribute (`#[test]`, `#[tokio::test]`, …). The Rust extractor sets
//!   those ([`is_rust_test_attribute`], [`is_rust_cfg_test`]).
//!
//! **The rule is conservative: only unambiguous test conventions count.** A
//! name that is a test in one ecosystem and product code in another is left
//! as product code unless its position settles it: `test/` only at the
//! repository root or directly under `src/` (Maven/Gradle), so Django's
//! `django/test/` package stays product code; `spec/` only at the root and
//! `*_spec` only for Ruby; camel-case source sets only directly under `src/`
//! (`src/androidTest/`). Test *helpers* that ship with the product — a
//! `testing` module, Flask's `testing.py`, hono's `src/helper/testing/`, a
//! `test-utils` package, `test_utils.py` — and benchmarks and examples are
//! product code: they are part of what a user imports or runs.

/// Directory names that hold tests wherever they appear, compared
/// case-insensitively (`Tests/` is SwiftPM's).
const TEST_DIRS: [&str; 6] = [
    "tests",
    "__tests__",
    "__test__",
    "__mocks__",
    "testdata",
    "e2e",
];

/// Python files named `test_*` that by convention hold helpers, not tests.
const PYTHON_HELPERS: [&str; 6] = [
    "test_utils",
    "test_util",
    "test_helpers",
    "test_helper",
    "test_support",
    "test_settings",
];

/// Whether a file at `path` (repository-relative, `/`- or `\`-separated)
/// holds test code by its language's convention:
///
/// - **a test directory**: `tests/`, `__tests__/`, `__mocks__/`,
///   `testdata/` or `e2e/` anywhere; a name ending in `-tests` or `_tests`
///   (`runtime-tests/`, `integration_tests/`); `test/` at the root or directly
///   under `src/` (`src/test/java`); `spec/` at the root; a Gradle/Kotlin
///   test source set directly under `src/` (`src/androidTest/`,
///   `src/commonTest/`, `src/testFixtures/`); a .NET test project
///   (`MyApp.Tests/`, `MyApp.UnitTests/`);
/// - **a test file name**: `*.test.*`, `*.spec.*`, `*.test-d.ts`,
///   `*.e2e-spec.*` (JavaScript, TypeScript); `*_test.*` (Go, Python, Dart,
///   Elixir, Ruby, C++) and `*_unittest.*`; Python's `test_*.py` (except
///   helper modules such as `test_utils.py`), `tests.py` (Django) and
///   `conftest.py`; Ruby's `*_spec.rb`; Rust's `tests.rs` (an out-of-line
///   `mod tests;`); and for Java, Kotlin, Scala, Groovy, C#, Swift, PHP and
///   Objective-C a class file named `*Test` or `*Tests` (`*Spec` for Scala,
///   Groovy and Kotlin).
pub fn is_test_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let mut parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    let Some(name) = parts.pop() else {
        return false;
    };
    parts
        .iter()
        .enumerate()
        .any(|(i, dir)| is_test_dir(dir, i, i.checked_sub(1).map(|p| parts[p])))
        || is_test_file_name(name)
}

/// `dir` at depth `depth` (0 = repository root), below `parent`.
fn is_test_dir(dir: &str, depth: usize, parent: Option<&str>) -> bool {
    let lower = dir.to_ascii_lowercase();
    let at_root = depth == 0;
    let under_src = parent == Some("src");
    TEST_DIRS.contains(&lower.as_str())
        || lower.ends_with("-tests")
        || lower.ends_with("_tests")
        || ((at_root || under_src) && lower == "test")
        || (at_root && lower == "spec")
        || (under_src && (dir == "testFixtures" || ends_with_camel_word(dir, "Test")))
        || dir
            .rsplit_once('.')
            .is_some_and(|(_, last)| last == "Test" || last.ends_with("Tests"))
}

fn is_test_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some((stem, extension)) = name.split_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    if [
        ".test.",
        ".spec.",
        ".test-d.",
        ".e2e-spec.",
        "_test.",
        "_unittest.",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    match extension.as_str() {
        "py" => {
            (lower.starts_with("test_") && !PYTHON_HELPERS.contains(&stem))
                || lower == "tests.py"
                || lower == "conftest.py"
        }
        "rb" => lower.ends_with("_spec.rb"),
        "rs" => lower == "tests.rs",
        "java" | "cs" | "swift" | "php" | "m" => {
            ends_with_camel_word(stem, "Test") || ends_with_camel_word(stem, "Tests")
        }
        "kt" | "kts" | "scala" | "groovy" => ["Test", "Tests", "Spec"]
            .iter()
            .any(|word| ends_with_camel_word(stem, word)),
        _ => false,
    }
}

/// `name` is `<something>Word` where `<something>` ends in a lower-case
/// letter or a digit — `UserTest`, `commonTest`, not `Test` or `LATEST`.
fn ends_with_camel_word(name: &str, word: &str) -> bool {
    name.strip_suffix(word)
        .and_then(|head| head.chars().last())
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether a Rust attribute (`#[...]`, as written) marks a test function:
/// its path ends in `test` — `#[test]`, `#[tokio::test]`,
/// `#[tokio::test(flavor = "multi_thread")]`, `#[sqlx::test]`.
pub fn is_rust_test_attribute(attribute: &str) -> bool {
    let Some(inner) = attribute
        .trim()
        .strip_prefix("#[")
        .and_then(|a| a.strip_suffix(']'))
    else {
        return false;
    };
    let path = inner.split(['(', '=', ' ']).next().unwrap_or_default();
    path.rsplit("::").next() == Some("test")
}

/// Whether a Rust attribute (`#[...]`, as written) compiles its item for
/// tests only: `#[cfg(test)]`, or `#[cfg(all(test, …))]`. `#[cfg(not(test))]`
/// and `#[cfg(any(test, …))]` do not count — the item exists outside tests.
pub fn is_rust_cfg_test(attribute: &str) -> bool {
    let compact: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(predicate) = compact
        .strip_prefix("#[cfg(")
        .and_then(|a| a.strip_suffix(")]"))
    else {
        return false;
    };
    if predicate == "test" {
        return true;
    }
    predicate
        .strip_prefix("all(")
        .and_then(|a| a.strip_suffix(')'))
        .is_some_and(|args| top_level_args(args).contains(&"test"))
}

/// Split `a,b(c,d),e` at the commas outside parentheses.
fn top_level_args(args: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_by_language_convention() {
        for path in [
            // Directories
            "tests/test_requests.py",
            "test/unit/a.js",
            "src/__tests__/a.ts",
            "src/__mocks__/fs.ts",
            "testdata/protoexample/test.pb.go",
            "binding/testdata/a.go",
            "spec/models/user_spec.rb",
            "spec/support/helpers.rb",
            "runtime-tests/deno/hono.ts",
            "integration_tests/api.rs",
            "src/test/java/com/acme/UserService.java",
            "src/androidTest/kotlin/A.kt",
            "src/commonTest/kotlin/A.kt",
            "src/testFixtures/java/A.java",
            "Tests/MyLibTests/A.swift",
            "MyApp.Tests/UserServiceFacts.cs",
            "src/MyApp.UnitTests/A.cs",
            "e2e/login.ts",
            "crates/core/tests/cli.rs",
            // File names
            "src/hono.test.ts",
            "src/utils/url.spec.ts",
            "src/app.e2e-spec.ts",
            "src/types.test-d.ts",
            "context_test.go",
            "pkg/foo_test.py",
            "lib/user_test.dart",
            "test/user_test.rb",
            "base/strings_unittest.cc",
            "pkg/test_api.py",
            "polls/tests.py",
            "src/flask/conftest.py",
            "lib/models/user_spec.rb",
            "src/parser/tests.rs",
            "src/main/java/com/acme/UserServiceTest.java",
            "App/LoginTests.swift",
            "app/UserSpec.scala",
            "src/UserTest.php",
            "windows\\tests\\a.cs",
        ] {
            assert!(is_test_path(path), "{path} should be test code");
        }
    }

    #[test]
    fn product_code_is_not_test_code() {
        for path in [
            "src/hono.ts",
            "src/helper/testing/index.ts",
            "src/flask/testing.py",
            "src/requests/sessions.py",
            "context.go",
            "contest.py",
            "latest.ts",
            "src/attestation.rs",
            "src/main/java/com/acme/Test.java",
            "src/main/java/com/acme/LATEST.java",
            "src/main/java/com/acme/Contest.java",
            "benchmarks/handle-event/index.mjs",
            "examples/basic/main.go",
            "src/testing.rs",
            "docs/testing.md",
            "README.md",
            "",
            // Ambiguous names: product code unless their position settles it.
            "django/test/client.py",
            "django/test/utils.py",
            "packages/test-utils/src/index.ts",
            "src/test_utils.py",
            "pkg/test_helpers.py",
            "api/spec/openapi.ts",
            "lib/openapi_spec.ts",
            "src/features/abTest/variant.ts",
            "MyAppTests/Login.swift",
            "src/test_vectors.c",
            "lib/my-test.js",
        ] {
            assert!(!is_test_path(path), "{path} should be product code");
        }
    }

    #[test]
    fn rust_test_attributes() {
        for attribute in [
            "#[test]",
            "#[tokio::test]",
            "#[tokio::test(flavor = \"multi_thread\")]",
            "#[sqlx::test]",
        ] {
            assert!(is_rust_test_attribute(attribute), "{attribute}");
        }
        for attribute in [
            "#[derive(Debug)]",
            "#[testing]",
            "#[cfg(test)]",
            "#[inline]",
        ] {
            assert!(!is_rust_test_attribute(attribute), "{attribute}");
        }
    }

    #[test]
    fn rust_cfg_test_modules() {
        for attribute in [
            "#[cfg(test)]",
            "#[cfg( test )]",
            "#[cfg(all(test, feature = \"x\"))]",
            "#[cfg(all(unix, test))]",
        ] {
            assert!(is_rust_cfg_test(attribute), "{attribute}");
        }
        for attribute in [
            "#[cfg(not(test))]",
            "#[cfg(any(test, feature = \"x\"))]",
            "#[cfg(feature = \"test\")]",
            "#[cfg(all(not(test), unix))]",
            "#[test]",
        ] {
            assert!(!is_rust_cfg_test(attribute), "{attribute}");
        }
    }
}
