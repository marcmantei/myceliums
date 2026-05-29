//! File-level protections against pathological inputs.
//!
//! Provides logic for skipping oversized files, files with excessively long lines,
//! and minified/bundled files before parsing.

use crate::config::AnalysisSection;
use std::path::Path;

/// Reason why a file was skipped from analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSkipReason {
    /// File exceeds configured size limit.
    OversizedFile,
    /// File contains a line longer than the configured limit.
    LineTooLong,
    /// File matches a skip pattern (minified, bundled, map files).
    MinifiedOrBundled,
    /// File parsing timed out.
    ParseTimeout,
    /// File is not valid UTF-8 (binary file).
    BinaryFile,
}

impl std::fmt::Display for FileSkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OversizedFile => write!(f, "oversized file"),
            Self::LineTooLong => write!(f, "line too long"),
            Self::MinifiedOrBundled => write!(f, "minified/bundled"),
            Self::ParseTimeout => write!(f, "parse timeout"),
            Self::BinaryFile => write!(f, "binary file (not UTF-8)"),
        }
    }
}

/// Check if a file should be skipped based on pre-parse rules.
///
/// Returns `Some(FileSkipReason)` if the file should be skipped, `None` otherwise.
pub fn should_skip_file(
    path: &Path,
    raw_bytes: &[u8],
    config: &AnalysisSection,
) -> Option<FileSkipReason> {
    // Check file size (convert KB limit to bytes)
    if config.max_file_size_kb > 0 {
        let max_size_bytes = config.max_file_size_kb * 1024;
        if raw_bytes.len() as u64 > max_size_bytes {
            return Some(FileSkipReason::OversizedFile);
        }
    }

    // Check UTF-8 validity as the first check for content validation.
    // Binary files (e.g., PNG with .ts extension) fail this check and are skipped gracefully.
    if std::str::from_utf8(raw_bytes).is_err() {
        return Some(FileSkipReason::BinaryFile);
    }

    // Check for long lines (only if UTF-8 and max_line_length_bytes > 0)
    if config.max_line_length_bytes > 0 {
        if let Ok(content) = std::str::from_utf8(raw_bytes) {
            if content
                .lines()
                .any(|l| l.len() > config.max_line_length_bytes)
            {
                return Some(FileSkipReason::LineTooLong);
            }
        }
    }

    // Check extension against skip patterns
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        for pattern in &config.skip_patterns {
            if name.ends_with(pattern) {
                return Some(FileSkipReason::MinifiedOrBundled);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip_oversized_file() {
        let config = AnalysisSection {
            max_file_size_kb: 1,
            ..Default::default()
        };
        let path = Path::new("test.rs");
        let oversized = vec![0u8; 2048]; // 2KB, exceeds 1KB limit

        assert_eq!(
            should_skip_file(path, &oversized, &config),
            Some(FileSkipReason::OversizedFile)
        );
    }

    #[test]
    fn test_should_not_skip_within_size_limit() {
        let config = AnalysisSection {
            max_file_size_kb: 10,
            ..Default::default()
        };
        let path = Path::new("test.rs");
        let small = b"fn main() {}".to_vec();

        assert_eq!(should_skip_file(path, &small, &config), None);
    }

    #[test]
    fn test_should_skip_line_too_long() {
        let config = AnalysisSection {
            max_line_length_bytes: 10,
            ..Default::default()
        };
        let path = Path::new("test.rs");
        let long_line = b"this line is definitely longer than ten bytes".to_vec();

        assert_eq!(
            should_skip_file(path, &long_line, &config),
            Some(FileSkipReason::LineTooLong)
        );
    }

    #[test]
    fn test_should_not_skip_short_line() {
        let config = AnalysisSection {
            max_line_length_bytes: 50,
            ..Default::default()
        };
        let path = Path::new("test.rs");
        let short_line = b"fn main() {}".to_vec();

        assert_eq!(should_skip_file(path, &short_line, &config), None);
    }

    #[test]
    fn test_should_skip_minified_js() {
        let config = AnalysisSection {
            skip_patterns: vec!["min.js".into()],
            ..Default::default()
        };
        let path = Path::new("bundle.min.js");
        let content = b"var x = 1;".to_vec();

        assert_eq!(
            should_skip_file(path, &content, &config),
            Some(FileSkipReason::MinifiedOrBundled)
        );
    }

    #[test]
    fn test_should_skip_map_file() {
        let config = AnalysisSection {
            skip_patterns: vec!["map".into()],
            ..Default::default()
        };
        let path = Path::new("app.js.map");
        let content = b"{}".to_vec();

        assert_eq!(
            should_skip_file(path, &content, &config),
            Some(FileSkipReason::MinifiedOrBundled)
        );
    }

    #[test]
    fn test_should_not_skip_normal_js() {
        let config = AnalysisSection {
            skip_patterns: vec!["min.js".into(), "bundle.js".into()],
            ..Default::default()
        };
        let path = Path::new("app.js");
        let content = b"function main() {}".to_vec();

        assert_eq!(should_skip_file(path, &content, &config), None);
    }

    #[test]
    fn test_skip_zero_limit_disables_check() {
        let config = AnalysisSection {
            max_file_size_kb: 0,      // 0 means no limit
            max_line_length_bytes: 0, // 0 means no limit
            skip_patterns: vec![],    // no patterns
            ..Default::default()
        };
        let path = Path::new("test.rs");
        let large = vec![0u8; 10_000_000]; // 10MB

        assert_eq!(should_skip_file(path, &large, &config), None);
    }

    #[test]
    fn test_should_skip_binary_file() {
        let config = AnalysisSection::default();
        let path = Path::new("binary.ts");
        // Binary data with invalid UTF-8 sequences (like PNG header)
        let binary = vec![0x00, 0x01, 0x02, 0xff, 0xfe, 0x80, 0x81, 0xc0, 0xc1];

        assert_eq!(
            should_skip_file(path, &binary, &config),
            Some(FileSkipReason::BinaryFile)
        );
    }

    #[test]
    fn test_should_skip_binary_with_valid_extension() {
        let config = AnalysisSection::default();
        let path = Path::new("image.rs"); // .rs extension but binary content
                                          // Invalid UTF-8 sequence
        let binary = vec![0xff, 0xfe, 0xfd];

        assert_eq!(
            should_skip_file(path, &binary, &config),
            Some(FileSkipReason::BinaryFile)
        );
    }

    #[test]
    fn test_binary_check_before_line_length_check() {
        let config = AnalysisSection {
            max_line_length_bytes: 5,
            ..Default::default()
        };
        let path = Path::new("test.ts");
        // Binary data - should be detected before line length check
        let binary = vec![0xff, 0xfe];

        assert_eq!(
            should_skip_file(path, &binary, &config),
            Some(FileSkipReason::BinaryFile)
        );
    }
}
