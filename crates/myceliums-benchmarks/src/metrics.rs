use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Represents measurements for a single run (time, tokens, tool calls)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurements {
    pub time_ms: f64,
    pub tokens: u32,
    pub tool_calls: u32,
}

/// Represents improvements calculated from baseline vs with_myceliums
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Improvements {
    pub time_reduction_percent: f64,
    pub token_reduction_percent: f64,
    pub tool_call_reduction_percent: f64,
}

impl Improvements {
    /// Calculate improvements from baseline and with_myceliums measurements
    pub fn calculate(baseline: &Measurements, with_myceliums: &Measurements) -> Self {
        let time_reduction_percent = if baseline.time_ms > 0.0 {
            ((baseline.time_ms - with_myceliums.time_ms) / baseline.time_ms) * 100.0
        } else {
            0.0
        };

        let token_reduction_percent = if baseline.tokens > 0 {
            ((baseline.tokens - with_myceliums.tokens) as f64 / baseline.tokens as f64) * 100.0
        } else {
            0.0
        };

        let tool_call_reduction_percent = if baseline.tool_calls > 0 {
            ((baseline.tool_calls - with_myceliums.tool_calls) as f64 / baseline.tool_calls as f64)
                * 100.0
        } else {
            0.0
        };

        Self {
            time_reduction_percent,
            token_reduction_percent,
            tool_call_reduction_percent,
        }
    }
}

/// Represents a single scenario's benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMetrics {
    pub name: String,
    pub description: String,
    pub baseline: Measurements,
    pub with_myceliums: Measurements,
    pub improvements: Improvements,
    /// True if measurements are from real runs, false if illustrative estimates
    pub is_verified: bool,
}

impl ScenarioMetrics {
    pub fn new(
        name: String,
        description: String,
        baseline: Measurements,
        with_myceliums: Measurements,
        is_verified: bool,
    ) -> Self {
        let improvements = Improvements::calculate(&baseline, &with_myceliums);
        Self {
            name,
            description,
            baseline,
            with_myceliums,
            improvements,
            is_verified,
        }
    }

    /// Deprecated: use new() with is_verified parameter instead
    #[deprecated(since = "0.2.0", note = "Use new() with is_verified parameter")]
    pub fn from_measurements(
        name: String,
        description: String,
        baseline: Measurements,
        with_myceliums: Measurements,
    ) -> Self {
        Self::new(name, description, baseline, with_myceliums, false)
    }
}

/// Environment information for benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub os: String,
    pub cpu_count: u32,
    pub memory_gb: u32,
    pub rust_version: String,
}

/// Aggregate metrics across all scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub avg_time_reduction_percent: f64,
    pub avg_token_reduction_percent: f64,
    pub avg_tool_call_reduction_percent: f64,
}

impl AggregateMetrics {
    pub fn calculate(scenarios: &[ScenarioMetrics]) -> Self {
        if scenarios.is_empty() {
            return Self {
                avg_time_reduction_percent: 0.0,
                avg_token_reduction_percent: 0.0,
                avg_tool_call_reduction_percent: 0.0,
            };
        }

        let sum_time: f64 = scenarios
            .iter()
            .map(|s| s.improvements.time_reduction_percent)
            .sum();
        let sum_tokens: f64 = scenarios
            .iter()
            .map(|s| s.improvements.token_reduction_percent)
            .sum();
        let sum_calls: f64 = scenarios
            .iter()
            .map(|s| s.improvements.tool_call_reduction_percent)
            .sum();

        let count = scenarios.len() as f64;

        Self {
            avg_time_reduction_percent: sum_time / count,
            avg_token_reduction_percent: sum_tokens / count,
            avg_tool_call_reduction_percent: sum_calls / count,
        }
    }
}

/// Verified metrics report - the final output
///
/// **IMPORTANT**: This structure may contain either verified measurements or illustrative
/// estimates depending on the `is_verified` flag in each ScenarioMetrics.
///
/// - When `is_verified = true`: Metrics are from real measurements (actual grep runs,
///   real timing, actual token counts)
/// - When `is_verified = false`: Metrics are illustrative estimates for comparison purposes
///   only and should not be treated as definitive performance benchmarks.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifiedMetrics {
    pub version: String,
    pub timestamp: String,
    pub environment: EnvironmentInfo,
    pub scenarios: Vec<ScenarioMetrics>,
    pub aggregate: AggregateMetrics,
}

impl VerifiedMetrics {
    pub fn new(
        version: String,
        environment: EnvironmentInfo,
        scenarios: Vec<ScenarioMetrics>,
    ) -> Self {
        let timestamp = chrono::Local::now().to_rfc3339();
        let aggregate = AggregateMetrics::calculate(&scenarios);

        Self {
            version,
            timestamp,
            environment,
            scenarios,
            aggregate,
        }
    }

    /// Convert to JSON string with pretty formatting
    pub fn to_json_string(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Save to a JSON file
    pub fn save_to_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new(".")))?;
        let json = self.to_json_string()?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

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
    fn test_improvements_calculation() {
        let baseline = Measurements {
            time_ms: 2400.0,
            tokens: 15000,
            tool_calls: 8,
        };

        let with_myceliums = Measurements {
            time_ms: 320.0,
            tokens: 2100,
            tool_calls: 2,
        };

        let improvements = Improvements::calculate(&baseline, &with_myceliums);

        // Time reduction: (2400 - 320) / 2400 * 100 = 86.67%
        assert!((improvements.time_reduction_percent - 86.67).abs() < 0.1);
        // Token reduction: (15000 - 2100) / 15000 * 100 = 86.0%
        assert!((improvements.token_reduction_percent - 86.0).abs() < 0.1);
        // Tool call reduction: (8 - 2) / 8 * 100 = 75.0%
        assert!((improvements.tool_call_reduction_percent - 75.0).abs() < 0.1);
    }

    #[test]
    fn test_aggregate_metrics() {
        let scenarios = vec![
            ScenarioMetrics::new(
                "scenario1".to_string(),
                "Test 1".to_string(),
                Measurements {
                    time_ms: 1000.0,
                    tokens: 5000,
                    tool_calls: 5,
                },
                Measurements {
                    time_ms: 200.0,
                    tokens: 1000,
                    tool_calls: 1,
                },
                false, // is_verified = false (estimates)
            ),
            ScenarioMetrics::new(
                "scenario2".to_string(),
                "Test 2".to_string(),
                Measurements {
                    time_ms: 1000.0,
                    tokens: 5000,
                    tool_calls: 5,
                },
                Measurements {
                    time_ms: 200.0,
                    tokens: 1000,
                    tool_calls: 1,
                },
                false, // is_verified = false (estimates)
            ),
        ];

        let agg = AggregateMetrics::calculate(&scenarios);
        assert!(agg.avg_time_reduction_percent > 0.0);
        assert!(agg.avg_token_reduction_percent > 0.0);
        assert!(agg.avg_tool_call_reduction_percent > 0.0);
    }

    #[test]
    fn test_benchmark_timer() {
        let timer = BenchmarkTimer::start();
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = timer.stop();
        assert!(elapsed >= 10.0);
    }

    #[test]
    fn test_verified_metrics_json() {
        let env = EnvironmentInfo {
            os: "linux".to_string(),
            cpu_count: 4,
            memory_gb: 16,
            rust_version: "1.75.0".to_string(),
        };

        let scenarios = vec![ScenarioMetrics::new(
            "test".to_string(),
            "Test scenario".to_string(),
            Measurements {
                time_ms: 1000.0,
                tokens: 5000,
                tool_calls: 5,
            },
            Measurements {
                time_ms: 200.0,
                tokens: 1000,
                tool_calls: 1,
            },
            false, // is_verified = false (estimates)
        )];

        let metrics = VerifiedMetrics::new("0.1.0".to_string(), env, scenarios);
        let json = metrics.to_json_string().expect("Failed to serialize");
        assert!(json.contains("0.1.0"));
        assert!(json.contains("test"));
    }
}
