//! PDF-to-markdown conversion via the `opendataloader-pdf` CLI tool.
//!
//! This module is gated behind the `pdf` Cargo feature. It shells out to
//! `opendataloader-pdf` which converts a PDF file into a Markdown file on
//! disk, then reads and returns the resulting content.

use std::path::Path;

use anyhow::{bail, Context, Result};
use tracing::debug;

/// Check whether the `opendataloader-pdf` CLI is available on `$PATH`.
#[allow(dead_code)]
pub fn is_pdf_cli_available() -> bool {
    std::process::Command::new("opendataloader-pdf")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Convert a PDF file to Markdown by invoking `opendataloader-pdf`.
///
/// Copies the PDF to a temp directory before conversion to avoid writing
/// artifacts into the user's source tree.
pub fn convert_pdf_to_markdown(path: &Path) -> Result<String> {
    let path = path
        .canonicalize()
        .with_context(|| format!("PDF path does not exist: {}", path.display()))?;

    // Copy PDF to temp dir so the CLI writes its .md output there, not in the source tree
    let tmp_dir = std::env::temp_dir().join("myceliums-pdf");
    std::fs::create_dir_all(&tmp_dir)?;
    let tmp_pdf = tmp_dir.join(
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("input.pdf")),
    );
    std::fs::copy(&path, &tmp_pdf)
        .with_context(|| format!("Failed to copy PDF to temp dir: {}", path.display()))?;

    let md_output = tmp_pdf.with_extension("md");

    let output = std::process::Command::new("opendataloader-pdf")
        .arg("--format")
        .arg("markdown")
        .arg(&tmp_pdf)
        .output()
        .with_context(|| "Failed to run opendataloader-pdf. Is it installed and on PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&tmp_pdf);
        bail!(
            "opendataloader-pdf exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }

    debug!("opendataloader-pdf completed for {}", path.display());

    let markdown = std::fs::read_to_string(&md_output).with_context(|| {
        format!(
            "opendataloader-pdf succeeded but output file not found: {}",
            md_output.display()
        )
    })?;

    // Clean up temp files
    let _ = std::fs::remove_file(&tmp_pdf);
    let _ = std::fs::remove_file(&md_output);

    Ok(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pdf_cli_available_does_not_panic() {
        // Simply ensure the function runs without panicking.
        // The result depends on the host environment.
        let _ = is_pdf_cli_available();
    }

    #[test]
    fn test_convert_pdf_nonexistent_file_returns_error() {
        let result = convert_pdf_to_markdown(Path::new("/tmp/nonexistent_file_abc123.pdf"));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("does not exist") || msg.contains("No such file"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_convert_pdf_to_markdown_output_path() {
        // Verify the expected .md output path derivation logic.
        let pdf_path = Path::new("/some/dir/report.pdf");
        let expected = pdf_path.with_extension("md");
        assert_eq!(expected, Path::new("/some/dir/report.md"));
    }
}
