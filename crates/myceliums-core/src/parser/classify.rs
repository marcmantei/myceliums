//! Classifying symbols as test or product code at index time (#146).
//!
//! Search ranks test code below product code by default, so the index records
//! which symbols are tests ([`myceliums_storage::SymbolMetadata::test`]). Two
//! sources decide it:
//!
//! - **The file's path**, by the conventions of the languages the parser
//!   reads ([`is_test_path`]) — applied to every symbol of the file.
//! - **Markers inside a file** that is not a test file by path: in Rust, a
//!   `#[cfg(test)]` module and everything in it, and a function carrying a
//!   test attribute (`#[test]`, `#[tokio::test]`, …). The Rust extractor sets
//!   those ([`is_rust_test_attribute`], [`is_rust_cfg_test`]).
//!
//! Benchmarks, examples and test *helpers* shipped with the product (a
//! `testing` module, Flask's `testing.py`, hono's `src/helper/testing/`) are
//! not tests: they are part of what a user imports.

/// Directory names that hold tests, compared case-insensitively.
const TEST_DIRS: [&str; 9] = [
    "test",
    "tests",
    "__tests__",
    "__test__",
    "__mocks__",
    "testdata",
    "spec",
    "specs",
    "e2e",
];

/// Whether a file at `path` (repository-relative, `/`- or `\`-separated)
/// holds test code by its language's convention:
///
/// - **a test directory** anywhere on the path: `test/`, `tests/`,
///   `__tests__/`, `__mocks__/`, `testdata/`, `spec/`, `e2e/`, a name ending
///   in `-tests`/`_tests`/`-test`/`_test` or starting with `test-`/`test_`
///   (`runtime-tests/`, `integration_tests/`), or a camel-case source set or
///   test target (`androidTest/`, `commonTest/`, `MyAppTests/`) — which also
///   covers Maven/Gradle's `src/test/java`;
/// - **a test file name**: `*.test.*`, `*.spec.*`, `*-test.*` and
///   `*-spec.*` (JavaScript, TypeScript), `*_test.*` (Go, Python, Dart, C++, Elixir),
///   `*_unittest.*`, `*_spec.*` (Ruby), `test_*.*` (pytest, C),
///   `conftest.py`, and for the JVM, .NET, Swift and PHP a class file named
///   `*Test`, `*Tests` or `*Spec` (`UserServiceTest.java`).
pub fn is_test_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let mut parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    let Some(name) = parts.pop() else {
        return false;
    };
    parts.iter().any(|dir| is_test_dir(dir)) || is_test_file_name(name)
}

fn is_test_dir(dir: &str) -> bool {
    let lower = dir.to_ascii_lowercase();
    TEST_DIRS.contains(&lower.as_str())
        || ["-tests", "_tests", "-test", "_test"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        || ["test-", "test_"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        || ends_with_camel_word(dir, "Test")
        || ends_with_camel_word(dir, "Tests")
}

fn is_test_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if [
        ".test.",
        ".spec.",
        "-test.",
        "-spec.",
        "_test.",
        "_unittest.",
        "_spec.",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || lower.starts_with("test_")
        || lower == "conftest.py"
    {
        return true;
    }
    let Some((stem, extension)) = name.split_once('.') else {
        return false;
    };
    const CLASS_PER_FILE: [&str; 9] = [
        "java", "kt", "kts", "scala", "groovy", "cs", "swift", "php", "m",
    ];
    CLASS_PER_FILE.contains(&extension.to_ascii_lowercase().as_str())
        && ["Test", "Tests", "Spec"]
            .iter()
            .any(|word| ends_with_camel_word(stem, word))
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
            "spec/models/user_spec.rb",
            "runtime-tests/deno/hono.test.ts",
            "integration_tests/api.rs",
            "src/test/java/com/acme/UserService.java",
            "src/androidTest/kotlin/A.kt",
            "src/commonTest/kotlin/A.kt",
            "MyAppTests/LoginTests.swift",
            "Tests/MyLibTests/A.swift",
            "e2e/login.ts",
            // File names
            "src/hono.test.ts",
            "src/utils/url.spec.ts",
            "src/app.e2e-spec.ts",
            "context_test.go",
            "pkg/foo_test.py",
            "lib/user_test.dart",
            "base/strings_unittest.cc",
            "test_utils.py",
            "src/flask/conftest.py",
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
