//! File watcher for incremental re-indexing.
//!
//! Uses the `notify` crate for cross-platform file watching with debouncing
//! so that rapid saves don't trigger multiple re-indexes.

use anyhow::Result;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, info, warn};

/// Events emitted by the file watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// One or more files were changed (created or modified).
    FilesChanged(Vec<PathBuf>),
    /// One or more files were removed.
    FilesRemoved(Vec<PathBuf>),
}

/// Directories and patterns that should be ignored by the watcher.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
];

/// Start watching a directory for file changes.
///
/// Returns a tokio channel receiver that yields [`WatchEvent`]s.
/// The watcher runs on a background thread and debounces events with a 500ms
/// window so rapid saves are batched together.
///
/// The returned `notify_debouncer_mini::Debouncer` must be kept alive for the
/// duration of the watch — dropping it stops the watcher.
pub fn start_watching(
    root: &Path,
) -> Result<(
    tokio_mpsc::UnboundedReceiver<WatchEvent>,
    notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
)> {
    let (sync_tx, sync_rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(500), sync_tx)?;

    // Watch recursively
    debouncer
        .watcher()
        .watch(root, notify::RecursiveMode::Recursive)?;

    info!("Watching {} for file changes", root.display());

    // Bridge from std::sync::mpsc to tokio::sync::mpsc
    let (tx, rx) = tokio_mpsc::unbounded_channel();

    std::thread::spawn(move || {
        loop {
            match sync_rx.recv() {
                Ok(Ok(events)) => {
                    let mut changed = HashSet::new();
                    let mut removed = HashSet::new();

                    for event in events {
                        let path = &event.path;

                        // Skip ignored directories
                        if should_ignore(path) {
                            continue;
                        }

                        match event.kind {
                            DebouncedEventKind::Any => {
                                if path.exists() {
                                    // Only track files, not directories
                                    if path.is_file() {
                                        debug!("File changed: {}", path.display());
                                        changed.insert(path.clone());
                                    }
                                } else {
                                    debug!("File removed: {}", path.display());
                                    removed.insert(path.clone());
                                }
                            }
                            DebouncedEventKind::AnyContinuous => {
                                // Ongoing change, wait for final event
                            }
                            _ => {}
                        }
                    }

                    if !changed.is_empty() {
                        let paths: Vec<PathBuf> = changed.into_iter().collect();
                        if tx.send(WatchEvent::FilesChanged(paths)).is_err() {
                            break;
                        }
                    }
                    if !removed.is_empty() {
                        let paths: Vec<PathBuf> = removed.into_iter().collect();
                        if tx.send(WatchEvent::FilesRemoved(paths)).is_err() {
                            break;
                        }
                    }
                }
                Ok(Err(error)) => {
                    warn!("File watch error: {:?}", error);
                }
                Err(_) => {
                    // Channel closed — watcher was dropped
                    break;
                }
            }
        }
    });

    Ok((rx, debouncer))
}

/// Check whether a path should be ignored (hidden files, common build dirs).
fn should_ignore(path: &Path) -> bool {
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name = name.to_string_lossy();
            if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_ref()) {
                return true;
            }
        }
    }
    false
}

/// Convenience: resolve a root path from a potentially relative path.
pub fn resolve_watch_root(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .map_err(|e| anyhow::anyhow!("Cannot resolve watch path {}: {}", path.display(), e))
}
