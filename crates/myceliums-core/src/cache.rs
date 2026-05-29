//! Analysis cache freshness checks.
//!
//! [`check_cache`] determines whether a previously-stored analysis can be
//! reused or needs to be re-run, based on age, git changes, and structural
//! file modifications.

use anyhow::Result;
use myceliums_storage::RepoInfo;
use std::path::Path;
use std::process::Command;
use tracing::info;

/// Structural files that trigger a full re-analysis when changed.
const STRUCTURAL_FILES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "tsconfig.json",
    "tsconfig.node.json",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "pnpm-workspace.yaml",
    "lerna.json",
];

/// Result of checking whether cached analysis can be reused.
#[derive(Debug)]
pub enum CacheDecision {
    /// Cached analysis is fresh enough to reuse.
    UseCached { repo_id: String, reason: String },
    /// Re-analysis is needed.
    ReanalyzeNeeded { reason: String },
}

/// Configuration for cache freshness checks.
///
/// Controls the thresholds used by [`check_cache`] to decide whether
/// a cached analysis is still fresh enough to reuse.
pub struct CacheCheckConfig {
    /// Maximum age of analysis in minutes before forcing re-analysis.
    pub max_age_minutes: u64,
    /// Maximum number of changed files before forcing re-analysis.
    pub max_changed_files: usize,
}

impl Default for CacheCheckConfig {
    fn default() -> Self {
        Self {
            max_age_minutes: 60,
            max_changed_files: 50,
        }
    }
}

/// Check whether the cached analysis for a repository is still fresh.
pub fn check_cache(
    repo_info: &RepoInfo,
    repo_path: &Path,
    config: &CacheCheckConfig,
) -> CacheDecision {
    // Check age
    let analyzed_at = match chrono::DateTime::parse_from_rfc3339(&repo_info.analyzed_at) {
        Ok(dt) => dt,
        Err(_) => {
            return CacheDecision::ReanalyzeNeeded {
                reason: "Could not parse analyzed_at timestamp".to_string(),
            };
        }
    };

    let age = chrono::Utc::now().signed_duration_since(analyzed_at);
    let age_minutes = age.num_minutes() as u64;

    if age_minutes > config.max_age_minutes {
        return CacheDecision::ReanalyzeNeeded {
            reason: format!(
                "Analysis is {} minutes old (max: {})",
                age_minutes, config.max_age_minutes
            ),
        };
    }

    // Check git changes since last analyzed commit
    let analyzed_commit = match &repo_info.analyzed_commit {
        Some(commit) => commit.clone(),
        None => {
            // No git commit recorded — try timestamp-based cache check
            return check_cache_by_mtime(repo_info, repo_path, config);
        }
    };

    // Get changed files since last analysis
    let changed_files = match get_changed_files(repo_path, &analyzed_commit) {
        Ok(files) => files,
        Err(e) => {
            // Git not available (non-git directory) — fall back to timestamp-based check
            info!("Git unavailable, falling back to timestamp-based cache: {}", e);
            return check_cache_by_mtime(repo_info, repo_path, config);
        }
    };

    if changed_files.is_empty() {
        return CacheDecision::UseCached {
            repo_id: repo_info.id.clone(),
            reason: "No files changed since last analysis".to_string(),
        };
    }

    // Check for structural file changes
    let structural_changed: Vec<&str> = changed_files
        .iter()
        .filter(|f| STRUCTURAL_FILES.iter().any(|sf| f.ends_with(sf)))
        .map(|s| s.as_str())
        .collect();

    if !structural_changed.is_empty() {
        return CacheDecision::ReanalyzeNeeded {
            reason: format!(
                "Structural files changed: {}",
                structural_changed.join(", ")
            ),
        };
    }

    // Check number of changed files
    if changed_files.len() > config.max_changed_files {
        return CacheDecision::ReanalyzeNeeded {
            reason: format!(
                "{} files changed (max: {})",
                changed_files.len(),
                config.max_changed_files
            ),
        };
    }

    CacheDecision::UseCached {
        repo_id: repo_info.id.clone(),
        reason: format!(
            "Analysis is {} minutes old, {} files changed (within thresholds)",
            age_minutes,
            changed_files.len()
        ),
    }
}

/// Timestamp-based cache check for non-git directories.
///
/// Walks the directory and compares file modification times against the
/// last analysis timestamp. This is less precise than git diff (catches
/// saves without actual content changes) but works for any directory.
fn check_cache_by_mtime(
    repo_info: &RepoInfo,
    repo_path: &Path,
    config: &CacheCheckConfig,
) -> CacheDecision {
    let analyzed_at = match chrono::DateTime::parse_from_rfc3339(&repo_info.analyzed_at) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => {
            return CacheDecision::ReanalyzeNeeded {
                reason: "Could not parse analyzed_at timestamp".to_string(),
            };
        }
    };

    let analyzed_system_time: std::time::SystemTime = analyzed_at.into();
    let mut changed_count = 0usize;

    let skip_dirs: &[&str] = &[
        ".git",
        "node_modules",
        "__pycache__",
        "target",
        "dist",
        "build",
        ".venv",
        "venv",
    ];

    for entry in walkdir::WalkDir::new(repo_path)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden dirs and known noise dirs, but always allow the root
            if e.depth() == 0 {
                return true;
            }
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                return !skip_dirs.iter().any(|d| name == *d) && !name.starts_with('.');
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime > analyzed_system_time {
                    changed_count += 1;
                    if changed_count > config.max_changed_files {
                        return CacheDecision::ReanalyzeNeeded {
                            reason: format!(
                                "{} files modified since last analysis (timestamp-based)",
                                changed_count
                            ),
                        };
                    }
                }
            }
        }
    }

    if changed_count > 0 {
        CacheDecision::ReanalyzeNeeded {
            reason: format!(
                "{} files modified since last analysis (timestamp-based)",
                changed_count
            ),
        }
    } else {
        CacheDecision::UseCached {
            repo_id: repo_info.id.clone(),
            reason: "No files modified since last analysis (timestamp-based)".to_string(),
        }
    }
}

/// Get the current HEAD commit hash for a repository.
pub fn get_head_commit(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Get list of files changed between a commit and HEAD.
fn get_changed_files(repo_path: &Path, since_commit: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", since_commit, "HEAD"])
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git diff --name-only failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let files: Vec<String> = String::from_utf8(output.stdout)?
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(files)
}

// ── In-process query result cache ────────────────────────────────────

use dashmap::DashMap;
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct CacheEntry {
    result: String,
    created_at: Instant,
}

/// Thread-safe, TTL-based query result cache.
///
/// Keyed on `(query, repo_id, commit)` triples so results are automatically
/// invalidated when the repository advances to a new commit.
pub struct QueryCache {
    entries: DashMap<u64, CacheEntry>,
    max_entries: usize,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl QueryCache {
    /// Create a new cache with the given capacity and TTL (in seconds).
    pub fn new(max_entries: usize, ttl_secs: u64) -> Self {
        Self {
            entries: DashMap::with_capacity(max_entries),
            max_entries,
            ttl: Duration::from_secs(ttl_secs),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Look up a cached result. Returns `None` on miss or expiry.
    pub fn get(&self, query: &str, repo_id: &str, commit: &str) -> Option<String> {
        let key = Self::make_key(query, repo_id, commit);
        if let Some(entry) = self.entries.get(&key) {
            if entry.created_at.elapsed() < self.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.result.clone());
            }
            drop(entry); // drop Ref before remove
            self.entries.remove(&key);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a query result into the cache, evicting expired or oldest
    /// entries when at capacity.
    pub fn insert(&self, query: &str, repo_id: &str, commit: &str, result: String) {
        let key = Self::make_key(query, repo_id, commit);
        // Evict if at capacity
        if self.entries.len() >= self.max_entries {
            let mut to_remove = None;
            for entry in self.entries.iter() {
                if entry.value().created_at.elapsed() >= self.ttl {
                    to_remove = Some(*entry.key());
                    break;
                }
            }
            if to_remove.is_none() {
                // No expired entry found — evict the first entry we see.
                if let Some(entry) = self.entries.iter().next() {
                    to_remove = Some(*entry.key());
                }
            }
            if let Some(k) = to_remove {
                self.entries.remove(&k);
            }
        }
        self.entries.insert(
            key,
            CacheEntry {
                result,
                created_at: Instant::now(),
            },
        );
    }

    /// Remove all cached entries (e.g. after a new analysis completes).
    pub fn invalidate_repo(&self, _repo_id: &str) {
        self.entries.clear();
    }

    /// Return `(hits, misses)` counters.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    fn make_key(query: &str, repo_id: &str, commit: &str) -> u64 {
        let mut hasher = FxHasher::default();
        query.hash(&mut hasher);
        repo_id.hash(&mut hasher);
        commit.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_decision_expired() {
        let repo_info = RepoInfo {
            id: "test-123".to_string(),
            name: "test".to_string(),
            path: "/tmp/test".to_string(),
            analyzed_at: "2020-01-01T00:00:00+00:00".to_string(),
            symbol_count: 10,
            file_count: 5,
            analyzed_commit: Some("abc123".to_string()),
        };
        let config = CacheCheckConfig::default();
        let decision = check_cache(&repo_info, Path::new("/tmp/test"), &config);
        assert!(matches!(decision, CacheDecision::ReanalyzeNeeded { .. }));
    }

    #[test]
    fn test_cache_decision_no_commit_falls_back_to_mtime() {
        // When analyzed_commit is None (non-git dir), the cache uses
        // timestamp-based checking instead of forcing re-analysis.
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_info = RepoInfo {
            id: "test-123".to_string(),
            name: "test".to_string(),
            path: tmp.path().to_string_lossy().to_string(),
            analyzed_at: chrono::Utc::now().to_rfc3339(),
            symbol_count: 10,
            file_count: 5,
            analyzed_commit: None,
        };
        let config = CacheCheckConfig::default();
        // Empty dir + recent analysis → cache is fresh
        let decision = check_cache(&repo_info, tmp.path(), &config);
        assert!(matches!(decision, CacheDecision::UseCached { .. }));
    }

    #[test]
    fn test_cache_mtime_detects_new_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Create a file
        let file_path = tmp.path().join("new_file.ts");
        std::fs::write(&file_path, "export const x = 1;").unwrap();
        let file_mtime = std::fs::metadata(&file_path).unwrap().modified().unwrap();

        // Set analyzed_at to far in the past so the file mtime is definitely newer
        let past = chrono::Utc::now() - chrono::Duration::hours(1);
        let analyzed_system_time = std::time::SystemTime::from(past);

        // Verify the precondition: file is newer than analyzed_at
        assert!(
            file_mtime > analyzed_system_time,
            "File mtime {:?} should be > analyzed_at {:?}",
            file_mtime,
            analyzed_system_time
        );

        let repo_info = RepoInfo {
            id: "test-mtime".to_string(),
            name: "test".to_string(),
            path: tmp.path().to_string_lossy().to_string(),
            analyzed_at: past.to_rfc3339(),
            symbol_count: 0,
            file_count: 0,
            analyzed_commit: None,
        };
        let config = CacheCheckConfig {
            max_age_minutes: 120,
            ..Default::default()
        };

        // Count files via walkdir to verify our walker sees them
        let file_count = walkdir::WalkDir::new(tmp.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count();
        assert!(file_count > 0, "Walker should find at least 1 file");

        let decision = check_cache_by_mtime(&repo_info, tmp.path(), &config);
        assert!(
            matches!(decision, CacheDecision::ReanalyzeNeeded { .. }),
            "Expected ReanalyzeNeeded, got: {:?}",
            decision
        );
    }

    // ── QueryCache tests ────────────────────────────────────────────

    #[test]
    fn query_cache_insert_and_get() {
        let cache = QueryCache::new(64, 60);
        cache.insert("SELECT *", "repo-1", "abc123", "result-1".to_string());

        let hit = cache.get("SELECT *", "repo-1", "abc123");
        assert_eq!(hit, Some("result-1".to_string()));

        // Different commit → miss
        let miss = cache.get("SELECT *", "repo-1", "def456");
        assert_eq!(miss, None);
    }

    #[test]
    fn query_cache_ttl_expiry() {
        // 1 ms TTL so entries expire almost immediately
        let cache = QueryCache {
            entries: DashMap::with_capacity(64),
            max_entries: 64,
            ttl: Duration::from_millis(1),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        };
        cache.insert("q", "r", "c", "val".to_string());
        std::thread::sleep(Duration::from_millis(5));

        let result = cache.get("q", "r", "c");
        assert_eq!(result, None, "entry should have expired");
    }

    #[test]
    fn query_cache_invalidate_repo() {
        let cache = QueryCache::new(64, 60);
        cache.insert("q1", "repo-1", "c1", "v1".to_string());
        cache.insert("q2", "repo-1", "c1", "v2".to_string());

        cache.invalidate_repo("repo-1");

        assert_eq!(cache.get("q1", "repo-1", "c1"), None);
        assert_eq!(cache.get("q2", "repo-1", "c1"), None);
    }

    #[test]
    fn query_cache_stats_counting() {
        let cache = QueryCache::new(64, 60);
        cache.insert("q", "r", "c", "v".to_string());

        // 1 hit
        let _ = cache.get("q", "r", "c");
        // 2 misses
        let _ = cache.get("q", "r", "other");
        let _ = cache.get("missing", "r", "c");

        let (hits, misses) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 2);
    }

    #[test]
    fn query_cache_capacity_eviction() {
        let cache = QueryCache::new(2, 60);
        cache.insert("q1", "r", "c", "v1".to_string());
        cache.insert("q2", "r", "c", "v2".to_string());
        // At capacity — inserting a third should evict one
        cache.insert("q3", "r", "c", "v3".to_string());

        // The newest entry must exist
        assert_eq!(cache.get("q3", "r", "c"), Some("v3".to_string()));
        // At most 2 entries remain
        assert!(cache.entries.len() <= 2);
    }
}
