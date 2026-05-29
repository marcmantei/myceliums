//! Git metadata extraction for code symbols.
//!
//! This module extracts ownership and history information from git blame,
//! associating each symbol with last author, modification date, commit count,
//! and age information.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Git blame metadata for a code symbol.
///
/// Extracted from `git blame` and `git log --follow` for a file
/// and symbol's line range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitMetadata {
    /// The name of the author who last modified the symbol.
    pub last_author: String,
    /// ISO 8601 date when the symbol was last modified.
    pub last_modified: String,
    /// Total number of commits touching this symbol's lines.
    pub commit_count: u32,
    /// Age of the symbol in days since last modification.
    pub age_days: u32,
    /// Optional commit hash of the last modification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit_hash: Option<String>,
}

impl GitMetadata {
    /// Create GitMetadata from components.
    pub fn new(
        last_author: String,
        last_modified: String,
        commit_count: u32,
        age_days: u32,
    ) -> Self {
        Self {
            last_author,
            last_modified,
            commit_count,
            age_days,
            last_commit_hash: None,
        }
    }

    /// Create GitMetadata from components with commit hash.
    pub fn with_commit(
        last_author: String,
        last_modified: String,
        commit_count: u32,
        age_days: u32,
        last_commit_hash: String,
    ) -> Self {
        Self {
            last_author,
            last_modified,
            commit_count,
            age_days,
            last_commit_hash: Some(last_commit_hash),
        }
    }
}

/// Git blame extractor for efficiently processing symbols in a repository.
///
/// The extractor caches blame information per file and batches
/// line range queries to minimize subprocess overhead.
pub struct GitMetadataExtractor {
    repo_path: PathBuf,
    /// Cache: file_path -> line -> (author, date, commit_hash)
    blame_cache: HashMap<PathBuf, HashMap<u32, (String, String, String)>>,
    /// Cache: file_path -> total_commits_touching_file
    file_commit_count: HashMap<PathBuf, u32>,
}

impl GitMetadataExtractor {
    /// Create a new extractor for the given repository.
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            blame_cache: HashMap::new(),
            file_commit_count: HashMap::new(),
        }
    }

    /// Extract git metadata for a symbol at a given file and line range.
    ///
    /// This is the primary entry point. It caches blame information
    /// per file to avoid redundant subprocess calls.
    pub fn extract(
        &mut self,
        file_path: &Path,
        start_line: u32,
        end_line: u32,
    ) -> Result<GitMetadata> {
        // Normalize the path relative to repo root
        let abs_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.repo_path.join(file_path)
        };

        // Load blame cache for this file if not already cached
        if !self.blame_cache.contains_key(&abs_path) {
            self.load_blame_for_file(&abs_path)?;
        }

        // Determine the most recent modification across the symbol's lines
        let blame_data = self
            .blame_cache
            .get(&abs_path)
            .context("File blame data not found after loading")?;

        let mut last_author = String::new();
        let mut last_modified = String::new();
        let mut last_commit_hash = String::new();
        let mut most_recent_timestamp: Option<i64> = None;
        let mut touched_lines: HashMap<String, u32> = HashMap::new(); // commit_hash -> count

        for line_num in start_line..=end_line {
            if let Some((author, date_str, commit_hash)) = blame_data.get(&line_num) {
                // Parse the date to determine which is most recent
                if let Some(ts) = parse_git_date(date_str) {
                    if most_recent_timestamp.is_none() || ts > most_recent_timestamp.unwrap() {
                        most_recent_timestamp = Some(ts);
                        last_author = author.clone();
                        last_modified = date_str.clone();
                        last_commit_hash = commit_hash.clone();
                    }
                }
                *touched_lines.entry(commit_hash.clone()).or_insert(0) += 1;
            }
        }

        if last_author.is_empty() {
            // Fallback if no blame information found
            last_author = "unknown".to_string();
            last_modified = "unknown".to_string();
        }

        let commit_count = touched_lines.len() as u32;
        let age_days = calculate_age_days(&last_modified);

        Ok(GitMetadata::with_commit(
            last_author,
            last_modified,
            commit_count,
            age_days,
            last_commit_hash,
        ))
    }

    /// Load blame information for a specific file using `git blame`.
    ///
    /// This runs once per file and caches the results.
    fn load_blame_for_file(&mut self, abs_path: &Path) -> Result<()> {
        // Skip if path doesn't exist or is outside repo
        if !abs_path.exists() {
            debug!("File not found for blame: {:?}", abs_path);
            self.blame_cache
                .insert(abs_path.to_path_buf(), HashMap::new());
            return Ok(());
        }

        // Get relative path for git command
        let rel_path = abs_path
            .strip_prefix(&self.repo_path)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .to_string();

        // Run git blame --line-porcelain for detailed output
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("blame")
            .arg("--line-porcelain")
            .arg(&rel_path)
            .output()
            .context(format!("Failed to run git blame on {}", rel_path))?;

        if !output.status.success() {
            warn!(
                "git blame failed for {}: {}",
                rel_path,
                String::from_utf8_lossy(&output.stderr)
            );
            self.blame_cache
                .insert(abs_path.to_path_buf(), HashMap::new());
            return Ok(());
        }

        let blame_output =
            String::from_utf8(output.stdout).context("git blame output is not valid UTF-8")?;

        let mut blame_map: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut line_num: u32 = 1;
        let mut current_author = String::new();
        let mut current_date = String::new();
        let mut current_commit = String::new();

        for line in blame_output.lines() {
            // Parse git blame --line-porcelain output
            // First line of each entry: <commit_hash> <original_line> <final_line> [<num_lines>]
            if line.starts_with(|c: char| c.is_ascii_hexdigit()) && line.contains(' ') {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if !parts.is_empty() {
                    current_commit = parts[0].to_string();
                }
            } else if let Some(author_part) = line.strip_prefix("author ") {
                current_author = author_part.to_string();
            } else if let Some(date_part) = line.strip_prefix("author-time ") {
                if let Ok(timestamp) = date_part.parse::<i64>() {
                    current_date = format_unix_timestamp(timestamp);
                }
            }

            // When we hit a line that starts with a tab, we've completed one entry
            if line.starts_with('\t') && !current_author.is_empty() && !current_date.is_empty() {
                blame_map.insert(
                    line_num,
                    (
                        current_author.clone(),
                        current_date.clone(),
                        current_commit.clone(),
                    ),
                );
                line_num += 1;
            }
        }

        self.blame_cache.insert(abs_path.to_path_buf(), blame_map);

        // Cache total commit count for the file
        let commit_count = self.count_commits_for_file(&rel_path)?;
        self.file_commit_count
            .insert(abs_path.to_path_buf(), commit_count);

        Ok(())
    }

    /// Count total commits that have touched a file using `git log`.
    fn count_commits_for_file(&self, rel_path: &str) -> Result<u32> {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("log")
            .arg("--follow")
            .arg("--pretty=format:%H")
            .arg(rel_path)
            .output()
            .context(format!("Failed to run git log for {}", rel_path))?;

        if !output.status.success() {
            return Ok(0);
        }

        let log_output =
            String::from_utf8(output.stdout).context("git log output is not valid UTF-8")?;

        let count = log_output.lines().filter(|l| !l.is_empty()).count() as u32;
        Ok(count)
    }
}

/// Parse a git date string in ISO 8601 format (YYYY-MM-DD HH:MM:SS +ZZZZ).
/// Returns Unix timestamp if successfully parsed, or None.
fn parse_git_date(date_str: &str) -> Option<i64> {
    // Expected format: "2026-04-09 10:24:00 +0200"
    let parts: Vec<&str> = date_str.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    // Very simple parsing: just try to convert to a comparable format
    // For our purposes, we mainly need relative ordering
    let date_part = parts[0]; // YYYY-MM-DD
    let time_part = parts[1]; // HH:MM:SS

    // Convert YYYY-MM-DD HH:MM:SS to a sortable number
    let date_parts: Vec<&str> = date_part.split('-').collect();
    let time_parts: Vec<&str> = time_part.split(':').collect();

    if date_parts.len() != 3 || time_parts.len() != 3 {
        return None;
    }

    let year: i64 = date_parts[0].parse().ok()?;
    let month: i64 = date_parts[1].parse().ok()?;
    let day: i64 = date_parts[2].parse().ok()?;
    let hour: i64 = time_parts[0].parse().ok()?;
    let minute: i64 = time_parts[1].parse().ok()?;
    let second: i64 = time_parts[2].parse().ok()?;

    // Simple timestamp approximation for relative comparison
    // (not precise but sufficient for ordering)
    Some(
        year * 10000000000
            + month * 100000000
            + day * 1000000
            + hour * 10000
            + minute * 100
            + second,
    )
}

/// Convert a Unix timestamp (seconds) to ISO 8601 date string.
fn format_unix_timestamp(timestamp: i64) -> String {
    // Convert timestamp to SystemTime
    let system_time = UNIX_EPOCH + std::time::Duration::from_secs(timestamp as u64);

    // For simplicity, format as ISO 8601-like string
    // In production, use chrono crate for robust formatting
    use std::time::*;

    let duration = system_time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();

    // Very basic conversion (would be better with chrono)
    let days_since_epoch = total_secs / 86400;
    let secs_today = total_secs % 86400;

    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    let seconds = secs_today % 60;

    // Approximate date calculation (simplified)
    let mut year = 1970;
    let mut remaining_days = days_since_epoch;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year as u64 {
            break;
        }
        remaining_days -= days_in_year as u64;
        year += 1;
    }

    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month_days_adjusted = month_days;
    if is_leap_year(year) {
        month_days_adjusted[1] = 29;
    }

    let mut month = 1;
    let mut day = remaining_days + 1;
    for &days in &month_days_adjusted {
        if (day as u32) <= days {
            break;
        }
        day -= days as u64;
        month += 1;
    }

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Check if a year is a leap year.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Calculate age in days from an ISO 8601 date string to now.
fn calculate_age_days(date_str: &str) -> u32 {
    // Parse date_str: "YYYY-MM-DD HH:MM:SS +ZZZZ"
    let parts: Vec<&str> = date_str.split_whitespace().collect();
    if parts.is_empty() {
        return 0;
    }

    let date_part = parts[0]; // YYYY-MM-DD
    let date_parts: Vec<&str> = date_part.split('-').collect();

    if date_parts.len() != 3 {
        return 0;
    }

    let year: i64 = date_parts[0].parse().unwrap_or(2026);
    let month: u32 = date_parts[1].parse().unwrap_or(1);
    let day: u32 = date_parts[2].parse().unwrap_or(1);

    // Get current date (simplified)
    let now = SystemTime::now();
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();
    let days_since_epoch = total_secs / 86400;

    // Calculate days for the target date (simplified)
    let mut target_days: u64 = 0;
    for y in 1970..year {
        target_days += if is_leap_year(y) { 366 } else { 365 };
    }

    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month_days_adjusted = month_days;
    if is_leap_year(year) {
        month_days_adjusted[1] = 29;
    }

    for days in &month_days_adjusted[..month as usize - 1] {
        target_days += *days as u64;
    }
    target_days += (day - 1) as u64;

    (days_since_epoch as i64 - target_days as i64).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_date() {
        let date_str = "2026-04-09 10:24:00 +0200";
        let ts = parse_git_date(date_str);
        assert!(ts.is_some());
    }

    #[test]
    fn test_format_unix_timestamp() {
        // Test a known timestamp
        let timestamp: i64 = 1712658240; // 2024-04-09 10:24:00 UTC
        let formatted = format_unix_timestamp(timestamp);
        assert!(!formatted.is_empty());
        assert!(formatted.contains("-"));
        assert!(formatted.contains(":"));
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
    }
}
