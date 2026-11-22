use std::time::{Duration, Instant};

/// Tracks timing for each phase of the indexing pipeline.
///
/// Phases:
/// - **File Discovery**: Walking filesystem and collecting source files
/// - **Parsing**: AST parsing and symbol extraction (tree-sitter)
/// - **Graph Construction**: Resolving calls, building relationships
/// - **Embedding Generation**: Creating vectors for semantic search
/// - **Communities** and **Processes**: run after the analyzer, by `myc
///   analyze` and the MCP `analyze` tool ([`TimingReport::with_graph_phases`])
#[derive(Debug, Clone)]
pub struct TimingReport {
    /// Time to discover all source files
    pub file_discovery_ms: f64,
    /// Time for AST parsing and symbol extraction
    pub parsing_ms: f64,
    /// Time to resolve calls and build relationships
    pub graph_construction_ms: f64,
    /// Time to generate and store embeddings
    pub embedding_generation_ms: f64,
    /// Time to detect and store communities (0 until recorded)
    pub communities_ms: f64,
    /// Time to trace and store processes (0 until recorded)
    pub processes_ms: f64,
    /// Total time (sum of all phases)
    pub total_ms: f64,
}

impl TimingReport {
    /// Create a new timing report from phase durations.
    pub fn new(
        file_discovery_ms: f64,
        parsing_ms: f64,
        graph_construction_ms: f64,
        embedding_generation_ms: f64,
    ) -> Self {
        let total_ms =
            file_discovery_ms + parsing_ms + graph_construction_ms + embedding_generation_ms;

        Self {
            file_discovery_ms,
            parsing_ms,
            graph_construction_ms,
            embedding_generation_ms,
            communities_ms: 0.0,
            processes_ms: 0.0,
            total_ms,
        }
    }

    /// Adds the community detection and process tracing phases.
    pub fn with_graph_phases(mut self, communities_ms: f64, processes_ms: f64) -> Self {
        self.communities_ms = communities_ms;
        self.processes_ms = processes_ms;
        self.total_ms = self.file_discovery_ms
            + self.parsing_ms
            + self.graph_construction_ms
            + self.embedding_generation_ms
            + communities_ms
            + processes_ms;
        self
    }

    /// The phases on one line, in seconds: `discover 0.1 s, parse 12.3 s,
    /// resolve 4.5 s, communities 2.1 s, processes 0.4 s, embed 30.2 s`.
    /// `embed` replaces the embedding time when the run did not embed
    /// itself (e.g. `"in background"`, `"skipped"`).
    pub fn phase_line(&self, embed: Option<&str>) -> String {
        let s = |ms: f64| format!("{:.1} s", ms / 1000.0);
        let embed = embed
            .map(str::to_string)
            .unwrap_or_else(|| s(self.embedding_generation_ms));
        format!(
            "discover {}, parse {}, resolve {}, communities {}, processes {}, embed {}",
            s(self.file_discovery_ms),
            s(self.parsing_ms),
            s(self.graph_construction_ms),
            s(self.communities_ms),
            s(self.processes_ms),
            embed
        )
    }

    /// Format the timing report as a pretty-printed string with percentages.
    pub fn format_report(&self) -> String {
        if self.total_ms == 0.0 {
            return "(no timing data)".to_string();
        }

        let fd_pct = (self.file_discovery_ms / self.total_ms) * 100.0;
        let p_pct = (self.parsing_ms / self.total_ms) * 100.0;
        let gc_pct = (self.graph_construction_ms / self.total_ms) * 100.0;
        let eg_pct = (self.embedding_generation_ms / self.total_ms) * 100.0;
        let cd_pct = (self.communities_ms / self.total_ms) * 100.0;
        let pt_pct = (self.processes_ms / self.total_ms) * 100.0;

        format!(
            "Timing breakdown:\n  \
             File discovery:      {:>7.2}ms ({:>5.1}%)\n  \
             AST parsing:         {:>7.2}ms ({:>5.1}%)\n  \
             Graph construction:  {:>7.2}ms ({:>5.1}%)\n  \
             Embedding generation:{:>7.2}ms ({:>5.1}%)\n  \
             Communities:         {:>7.2}ms ({:>5.1}%)\n  \
             Processes:           {:>7.2}ms ({:>5.1}%)\n  \
             ─────────────────────────────────\n  \
             Total time:          {:>7.2}ms",
            self.file_discovery_ms,
            fd_pct,
            self.parsing_ms,
            p_pct,
            self.graph_construction_ms,
            gc_pct,
            self.embedding_generation_ms,
            eg_pct,
            self.communities_ms,
            cd_pct,
            self.processes_ms,
            pt_pct,
            self.total_ms
        )
    }

    /// Get the percentage of time spent on embeddings.
    pub fn embedding_percentage(&self) -> f64 {
        if self.total_ms == 0.0 {
            0.0
        } else {
            (self.embedding_generation_ms / self.total_ms) * 100.0
        }
    }

    /// Get the percentage of time spent on parsing + graph construction (non-embedding work).
    pub fn non_embedding_percentage(&self) -> f64 {
        100.0 - self.embedding_percentage()
    }
}

/// A simple timer for measuring elapsed time.
pub struct Timer {
    start: Instant,
}

impl Timer {
    /// Create a new timer and start it immediately.
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed time in milliseconds.
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    /// Get elapsed duration.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timing_report_formatting() {
        let report = TimingReport::new(100.0, 500.0, 200.0, 9200.0);
        let formatted = report.format_report();
        assert!(formatted.contains("File discovery"));
        assert!(formatted.contains("AST parsing"));
        assert!(formatted.contains("Graph construction"));
        assert!(formatted.contains("Embedding generation"));
        assert!(formatted.contains("10000.00"));
    }

    #[test]
    fn graph_phases_join_the_report_and_the_total() {
        let report = TimingReport::new(100.0, 500.0, 200.0, 0.0).with_graph_phases(1200.0, 300.0);
        assert_eq!(report.total_ms, 2300.0);
        let formatted = report.format_report();
        assert!(formatted.contains("Communities:"), "{formatted}");
        assert!(formatted.contains("Processes:"), "{formatted}");
        assert_eq!(
            report.phase_line(Some("skipped")),
            "discover 0.1 s, parse 0.5 s, resolve 0.2 s, communities 1.2 s, processes 0.3 s, \
             embed skipped"
        );
        assert!(report.phase_line(None).ends_with("embed 0.0 s"));
    }

    #[test]
    fn test_timing_percentages() {
        let report = TimingReport::new(100.0, 100.0, 100.0, 700.0);
        assert_eq!(report.embedding_percentage(), 70.0);
        assert_eq!(report.non_embedding_percentage(), 30.0);
    }

    #[test]
    fn test_timer_elapsed() {
        let timer = Timer::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.elapsed_ms();
        assert!(elapsed >= 10.0);
    }
}
