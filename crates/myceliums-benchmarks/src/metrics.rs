//! Timing records for the Criterion benchmarks.
//!
//! The scenario "estimates" that used to live alongside these types — constant
//! time, token and tool-call figures serialised by a `verified_metrics` bench —
//! were retired in #92: they measured nothing, and a struct called
//! `VerifiedMetrics` holding them invited exactly the misreading it warned
//! against. Measured retrieval quality lives in [`crate::eval`].

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Legacy benchmark result structure - kept for backward compatibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub category: String,
    pub duration_ms: f64,
    pub files_processed: usize,
    pub symbols_found: usize,
    pub memory_peak_mb: f64,
    pub timestamp: String,
}

/// Legacy benchmark results collection - kept for backward compatibility
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkResults {
    pub version: String,
    pub timestamp: String,
    pub results: Vec<BenchmarkResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_reduction_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_savings_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fewer_tool_calls_pct: Option<f64>,
}

impl BenchmarkResults {
    pub fn new(version: String) -> Self {
        Self {
            version,
            timestamp: chrono::Local::now().to_rfc3339(),
            results: Vec::new(),
            time_reduction_pct: None,
            cost_savings_pct: None,
            fewer_tool_calls_pct: None,
        }
    }

    pub fn add_result(&mut self, result: BenchmarkResult) {
        self.results.push(result);
    }

    pub fn set_improvements(
        &mut self,
        time_reduction_pct: f64,
        cost_savings_pct: f64,
        fewer_tool_calls_pct: f64,
    ) {
        self.time_reduction_pct = Some(time_reduction_pct);
        self.cost_savings_pct = Some(cost_savings_pct);
        self.fewer_tool_calls_pct = Some(fewer_tool_calls_pct);
    }
}

/// Timer for measuring benchmark duration
pub struct BenchmarkTimer {
    start: Instant,
}

impl BenchmarkTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    pub fn stop(self) -> f64 {
        self.elapsed_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_benchmark_timer() {
        let timer = BenchmarkTimer::start();
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = timer.stop();
        assert!(elapsed >= 10.0);
    }
}
