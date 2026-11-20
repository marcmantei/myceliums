//! Small terminal helpers: confirmation prompts and human-readable sizes.

use anyhow::Result;
use std::io::Write;

/// Format bytes into a human-readable string (KB, MB, GB).
pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    myceliums_core::text::truncate_ellipsis(s, max)
}

/// Prompt user with a yes/no question, returns true if yes.
pub(crate) fn prompt_yes_no(question: &str) -> Result<bool> {
    print!("{} [y/N] ", question);
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes" || answer == "j" || answer == "ja")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello...");
        assert_eq!(truncate_str("hi", 2), "hi");
        // Multi-byte names used to panic on a byte-index slice.
        assert_eq!(truncate_str("Größenänderung", 8), "Größe...");
        assert_eq!(truncate_str("🦀🦀🦀🦀🦀🦀", 5), "🦀🦀...");
    }
}
