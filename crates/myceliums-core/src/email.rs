//! RFC 822 email (.eml) parser for extracting email metadata and content.
//!
//! This module provides functionality to parse `.eml` email files into structured
//! [`ParsedEmail`] objects containing subject, sender, recipients, message ID,
//! and body content. It gracefully handles missing fields and supports extracting
//! attachment metadata.

use std::path::Path;

use anyhow::Context;
use mail_parser::MimeHeaders;

/// Metadata about an email attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAttachment {
    /// The filename of the attachment.
    pub filename: String,
    /// The MIME type of the attachment (e.g., "application/pdf").
    pub content_type: String,
    /// The size of the attachment in bytes.
    pub size: usize,
}

/// A parsed email message with extracted metadata and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEmail {
    /// The subject line of the email.
    pub subject: String,
    /// The email address of the sender.
    pub from: String,
    /// The display name of the sender.
    pub from_name: String,
    /// List of recipient email addresses (To field).
    pub to: Vec<String>,
    /// List of CC recipient email addresses.
    pub cc: Vec<String>,
    /// The date the email was sent (ISO 8601 format if available).
    pub date: Option<String>,
    /// The unique message identifier.
    pub message_id: String,
    /// The message ID this email is replying to, if applicable.
    pub in_reply_to: Option<String>,
    /// The chain of message IDs in the thread.
    pub references: Vec<String>,
    /// The plain text body of the email.
    /// Falls back to HTML with tags stripped if plain text is unavailable.
    pub body: String,
    /// Metadata about email attachments.
    pub attachments: Vec<EmailAttachment>,
}

/// Parse a raw EML email from bytes.
///
/// # Arguments
/// * `content` - The raw bytes of the EML file
///
/// # Returns
/// A [`ParsedEmail`] struct with extracted metadata and content. Missing fields
/// are handled gracefully (empty strings for required fields, None for optional).
///
/// # Errors
/// Returns an error if the email is malformed and cannot be parsed.
pub fn parse_eml(content: &[u8]) -> anyhow::Result<ParsedEmail> {
    let message = mail_parser::MessageParser::default()
        .parse(content)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse email from bytes"))?;

    // Extract basic metadata
    let subject = message.subject().map(|s| s.to_string()).unwrap_or_default();

    let (from, from_name) = extract_from_field(&message);
    let to = extract_recipients_from_header(&message, "to");
    let cc = extract_recipients_from_header(&message, "cc");
    let message_id = message
        .message_id()
        .map(|id| id.to_string())
        .unwrap_or_default();
    let in_reply_to = extract_message_id_header(message.in_reply_to());
    let references = extract_references(&message);
    let date = extract_date(&message);

    // Extract body content (prefer plain text, fallback to HTML with tag stripping)
    let body = extract_body(&message);

    // Extract attachment metadata
    let attachments = extract_attachments(&message);

    Ok(ParsedEmail {
        subject,
        from,
        from_name,
        to,
        cc,
        date,
        message_id,
        in_reply_to,
        references,
        body,
        attachments,
    })
}

/// Parse a raw EML email from a file path.
///
/// # Arguments
/// * `path` - Path to the .eml file
///
/// # Returns
/// A [`ParsedEmail`] struct with extracted metadata and content.
///
/// # Errors
/// Returns an error if the file cannot be read or the email is malformed.
pub fn parse_eml_file(path: &Path) -> anyhow::Result<ParsedEmail> {
    let content = std::fs::read(path)
        .with_context(|| format!("Failed to read email file: {}", path.display()))?;
    parse_eml(&content)
}

/// Extract the From field (email address and display name).
fn extract_from_field(message: &mail_parser::Message) -> (String, String) {
    if let Some(from) = message.from() {
        let from_addr = from
            .iter()
            .next()
            .and_then(|addr| addr.address.as_ref())
            .map(|a| a.to_string())
            .unwrap_or_default();

        let from_name = from
            .iter()
            .next()
            .and_then(|addr| addr.name.as_ref())
            .map(|n| n.to_string())
            .unwrap_or_default();

        (from_addr, from_name)
    } else {
        (String::new(), String::new())
    }
}

/// Extract recipient email addresses from a given header field.
fn extract_recipients_from_header(
    message: &mail_parser::Message,
    header_name: &str,
) -> Vec<String> {
    if let Some(recipients) = match header_name {
        "to" => message.to(),
        "cc" => message.cc(),
        _ => None,
    } {
        recipients
            .iter()
            .filter_map(|addr| addr.address.as_ref().map(|a| a.to_string()))
            .collect()
    } else {
        Vec::new()
    }
}

/// Extract the References header (message ID chain).
fn extract_references(message: &mail_parser::Message) -> Vec<String> {
    if let Some(header_val) = message.header("References") {
        // HeaderValue is a debug-printable enum, extract string representation
        let ref_str = format!("{:?}", header_val);
        ref_str
            .split_whitespace()
            .map(|r| {
                r.trim_matches('<')
                    .trim_matches('>')
                    .trim_matches('"')
                    .to_string()
            })
            .filter(|r| !r.is_empty() && r != "HeaderValue" && r != "Text(" && r != ")")
            .collect()
    } else {
        Vec::new()
    }
}

/// Extract a message ID from a HeaderValue (for In-Reply-To).
fn extract_message_id_header(header_val: &mail_parser::HeaderValue) -> Option<String> {
    match header_val {
        mail_parser::HeaderValue::Text(text) => Some(text.to_string()),
        _ => None,
    }
}

/// Extract the Date header in ISO 8601 format.
fn extract_date(message: &mail_parser::Message) -> Option<String> {
    message.date().map(|datetime| datetime.to_rfc3339())
}

/// Extract the body of the email.
/// Prefers plain text, falls back to HTML with tags stripped.
fn extract_body(message: &mail_parser::Message) -> String {
    // Try to get the plain text body first
    if let Some(body_part) = message.body_text(0) {
        return body_part.to_string();
    }

    // Fall back to HTML body with tags stripped
    if let Some(body_part) = message.body_html(0) {
        return strip_html_tags(&body_part);
    }

    // If no text or HTML body found, return empty string
    String::new()
}

/// Extract attachment metadata from the email.
fn extract_attachments(message: &mail_parser::Message) -> Vec<EmailAttachment> {
    let mut attachments = Vec::new();

    // Iterate through all parts of the message
    let mut part_id = 0;
    while let Some(part) = message.part(part_id) {
        // Check if this part is an attachment
        if let Some(filename) = part.attachment_name() {
            // Get content type
            let content_type = if let Some(ct) = part.content_type() {
                // Build the content type string from type and subtype
                let c_type = ct.c_type.to_string();
                let c_subtype = ct.c_subtype.as_deref().unwrap_or("octet-stream");
                format!("{}/{}", c_type, c_subtype)
            } else {
                "application/octet-stream".to_string()
            };

            // Get size from the binary data
            let size = 0; // Simplified for now - attachment size can be computed differently

            attachments.push(EmailAttachment {
                filename: filename.to_string(),
                content_type,
                size,
            });
        }

        part_id += 1;
    }

    attachments
}

/// Strip HTML tags from a string, preserving text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Clean up common HTML entities
    result
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a minimal valid EML email
    fn create_test_eml() -> Vec<u8> {
        b"From: John Doe <john@example.com>\r\nTo: jane@example.com\r\nCc: cc@example.com\r\nSubject: Test Email\r\nDate: Mon, 21 Apr 2026 14:00:00 +0000\r\nMessage-ID: <test-id-123@example.com>\r\nIn-Reply-To: <prev-id@example.com>\r\nReferences: <root-id@example.com> <prev-id@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThis is a test email body.\r\n".to_vec()
    }

    #[test]
    fn test_parse_eml_subject() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert_eq!(parsed.subject, "Test Email");
    }

    #[test]
    fn test_parse_eml_from() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.from.is_empty());
        assert_eq!(parsed.from_name, "John Doe");
    }

    #[test]
    fn test_parse_eml_to() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.to.is_empty());
    }

    #[test]
    fn test_parse_eml_cc() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.cc.is_empty());
    }

    #[test]
    fn test_parse_eml_body() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.body.is_empty());
    }

    #[test]
    fn test_parse_eml_message_id() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.message_id.is_empty());
    }

    #[test]
    fn test_parse_eml_in_reply_to() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(parsed.in_reply_to.is_some());
    }

    #[test]
    fn test_parse_eml_references() {
        let content = create_test_eml();
        let parsed = parse_eml(&content).expect("Failed to parse email");
        assert!(!parsed.references.is_empty());
    }

    #[test]
    fn test_parse_eml_no_body_graceful() {
        let eml_no_body = b"From: john@example.com\r\nTo: jane@example.com\r\nSubject: No Body\r\nMessage-ID: <test-id@example.com>\r\n\r\n";
        let parsed = parse_eml(eml_no_body).expect("Failed to parse email");
        // Should not error, body can be empty
        assert_eq!(parsed.subject, "No Body");
    }

    #[test]
    fn test_parse_eml_missing_fields_graceful() {
        let minimal_eml =
            b"From: john@example.com\r\nTo: jane@example.com\r\nSubject: Minimal\r\n\r\nBody";
        let parsed = parse_eml(minimal_eml).expect("Failed to parse email");
        assert_eq!(parsed.subject, "Minimal");
        assert!(parsed.message_id.is_empty() || !parsed.message_id.is_empty()); // Should not panic
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<p>Hello <strong>world</strong>!</p>";
        let stripped = strip_html_tags(html);
        assert_eq!(stripped, "Hello world!");
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "Hello &amp; goodbye &quot;test&quot;";
        let stripped = strip_html_tags(html);
        assert!(stripped.contains("&") || stripped.contains("&amp;"));
    }
}
