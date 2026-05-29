//! Progress reporting trait for analysis phases.
//!
//! The [`ProgressReporter`] trait allows the CLI to display progress bars
//! while keeping the core crate UI-agnostic. Use [`SilentReporter`] in
//! headless / hook mode where stdout must stay clean.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Describes the current phase of analysis.
#[derive(Debug, Clone)]
pub enum AnalysisPhase {
    /// Discovering source files in the repository.
    Discovering,
    /// Parsing source files. `current` and `total` track progress.
    Parsing { current: usize, total: usize },
    /// Building the symbol relationship graph.
    BuildingRelationships,
    /// Running Leiden community detection.
    DetectingCommunities,
    /// Tracing execution flows / processes.
    TracingProcesses,
    /// Generating vector embeddings for semantic search.
    GeneratingEmbeddings { current: usize, total: usize },
    /// Analysis finished successfully.
    Complete { symbols: usize, files: usize },
}

/// Trait for reporting analysis progress. Implementors must be thread-safe
/// because the parsing phase runs on a rayon thread pool.
pub trait ProgressReporter: Send + Sync {
    fn report(&self, phase: AnalysisPhase);
}

/// No-op reporter — suppresses all progress output.
/// Use this in `--yes` (hook) mode or when progress display is unwanted.
pub struct SilentReporter;

impl ProgressReporter for SilentReporter {
    fn report(&self, _phase: AnalysisPhase) {}
}

/// Thread-safe counter for tracking progress inside rayon parallel iterators.
/// Increment from worker threads, read from the progress polling task.
#[derive(Clone)]
pub struct AtomicProgress {
    counter: Arc<AtomicUsize>,
    total: usize,
}

impl AtomicProgress {
    pub fn new(total: usize) -> Self {
        Self {
            counter: Arc::new(AtomicUsize::new(0)),
            total,
        }
    }

    /// Increment by 1 (call from rayon worker threads).
    pub fn increment(&self) {
        self.counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Read current value (call from progress polling task).
    pub fn current(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silent_reporter() {
        let reporter = SilentReporter;
        // Should not panic
        reporter.report(AnalysisPhase::Discovering);
        reporter.report(AnalysisPhase::Parsing {
            current: 50,
            total: 100,
        });
        reporter.report(AnalysisPhase::Complete {
            symbols: 42,
            files: 10,
        });
    }

    #[test]
    fn test_atomic_progress() {
        let progress = AtomicProgress::new(100);
        assert_eq!(progress.current(), 0);
        assert_eq!(progress.total(), 100);

        progress.increment();
        progress.increment();
        progress.increment();
        assert_eq!(progress.current(), 3);
    }

    #[test]
    fn test_atomic_progress_clone_shared() {
        let progress = AtomicProgress::new(50);
        let cloned = progress.clone();

        progress.increment();
        cloned.increment();
        // Both share the same underlying atomic
        assert_eq!(progress.current(), 2);
        assert_eq!(cloned.current(), 2);
    }
}
