//! MBOX file parser for extracting individual emails from MBOX archives.
//!
//! The MBOX format stores multiple email messages in a single file, separated
//! by lines starting with `"From "` (with a space after "From"). This module
//! splits an MBOX file into individual messages and parses each one using
//! [`crate::email::parse_eml`].

use anyhow::{Context, Result};

use crate::email::{self, ParsedEmail};

/// Parse an MBOX file from raw bytes into a vector of parsed emails.
///
/// MBOX files contain multiple emails separated by lines starting with `"From "`.
/// Each email is extracted and parsed individually via [`email::parse_eml`].
///
/// # Arguments
/// * `content` - The raw bytes of the MBOX file
///
/// # Returns
/// A vector of [`ParsedEmail`] structs, one per message in the MBOX file.
///
/// # Errors
/// Returns an error if any individual email fails to parse.
pub fn parse_mbox(content: &[u8]) -> Result<Vec<ParsedEmail>> {
    let text = std::str::from_utf8(content).context("MBOX content is not valid UTF-8")?;

    let mut emails = Vec::new();
    let mut current_message = String::new();

    for line in text.lines() {
        if line.starts_with("From ") && !current_message.is_empty() {
            // We hit a new separator — parse the accumulated message
            let parsed = email::parse_eml(current_message.as_bytes())
                .context("Failed to parse email within MBOX")?;
            emails.push(parsed);
            current_message.clear();
        } else if line.starts_with("From ") && current_message.is_empty() {
            // First "From " line — skip the envelope separator
            continue;
        } else {
            current_message.push_str(line);
            current_message.push('\n');
        }
    }

    // Parse the last accumulated message
    if !current_message.trim().is_empty() {
        let parsed = email::parse_eml(current_message.as_bytes())
            .context("Failed to parse final email within MBOX")?;
        emails.push(parsed);
    }

    Ok(emails)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_test_mbox() -> Vec<u8> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let fixture_path = Path::new(manifest_dir)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/sample-email/test.mbox");
        std::fs::read(&fixture_path).unwrap_or_else(|e| {
            panic!(
                "Failed to read test fixture {}: {}",
                fixture_path.display(),
                e
            )
        })
    }

    #[test]
    fn test_parse_mbox_email_count() {
        let content = load_test_mbox();
        let emails = parse_mbox(&content).expect("Failed to parse MBOX");
        assert_eq!(emails.len(), 2, "Expected 2 emails in MBOX fixture");
    }

    #[test]
    fn test_parse_mbox_first_email() {
        let content = load_test_mbox();
        let emails = parse_mbox(&content).expect("Failed to parse MBOX");

        let first = &emails[0];
        assert_eq!(first.subject, "Project Update");
        assert_eq!(first.from, "alice@example.com");
        assert_eq!(first.from_name, "Alice Smith");
        assert!(first.body.contains("latest project update"));
    }

    #[test]
    fn test_parse_mbox_second_email() {
        let content = load_test_mbox();
        let emails = parse_mbox(&content).expect("Failed to parse MBOX");

        let second = &emails[1];
        assert_eq!(second.subject, "Re: Project Update");
        assert_eq!(second.from, "bob@example.com");
        assert_eq!(second.from_name, "Bob Jones");
        assert!(!second.cc.is_empty());
        assert!(second.in_reply_to.is_some());
    }

    #[test]
    fn test_parse_mbox_empty() {
        let emails = parse_mbox(b"").expect("Failed to parse empty MBOX");
        assert!(emails.is_empty());
    }

    #[test]
    fn test_parse_mbox_single_email() {
        let mbox = b"From sender@example.com Mon Apr 20 10:00:00 2026\nFrom: sender@example.com\nTo: recipient@example.com\nSubject: Single\nContent-Type: text/plain\n\nBody text\n";
        let emails = parse_mbox(mbox).expect("Failed to parse single-email MBOX");
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].subject, "Single");
    }
}
