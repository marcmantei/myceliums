use crate::baseline::{estimate_baseline, MeasurementResult};
use crate::metrics::{Measurements, ScenarioMetrics};
use anyhow::Result;

/// A benchmark scenario that can be run with and without Myceliums
pub trait Scenario {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// Estimate the baseline (without Myceliums) measurements
    /// **IMPORTANT**: These are illustrative estimates, not verified measurements
    fn baseline_estimate(&self) -> Result<MeasurementResult>;

    /// Estimate with Myceliums (theoretical improvements from structured queries)
    /// **IMPORTANT**: These are illustrative estimates, not verified measurements
    fn with_myceliums_estimate(&self) -> Result<MeasurementResult>;

    /// Run both estimates and return metrics
    /// Note: These metrics are marked as unverified estimates
    fn run(&self) -> Result<ScenarioMetrics> {
        let baseline_result = self.baseline_estimate()?;
        let myceliums_result = self.with_myceliums_estimate()?;

        let baseline = Measurements {
            time_ms: baseline_result.time_ms,
            tokens: baseline_result.tokens,
            tool_calls: baseline_result.tool_calls,
        };

        let with_myceliums = Measurements {
            time_ms: myceliums_result.time_ms,
            tokens: myceliums_result.tokens,
            tool_calls: myceliums_result.tool_calls,
        };

        Ok(ScenarioMetrics::new(
            self.name().to_string(),
            self.description().to_string(),
            baseline,
            with_myceliums,
            false, // Mark as unverified estimate
        ))
    }
}

/// Scenario 1: Find all callers of function X
pub struct FindAllCallersScenario;

impl Scenario for FindAllCallersScenario {
    fn name(&self) -> &str {
        "find_all_callers"
    }

    fn description(&self) -> &str {
        "Find all callers of function X in a medium-sized codebase (ILLUSTRATIVE ESTIMATE)"
    }

    fn baseline_estimate(&self) -> Result<MeasurementResult> {
        // Estimate baseline: grep + manual parsing
        // Theoretical: 2-3 grep calls, parsing 50-100 lines, generating prompt with results
        let result = estimate_baseline(
            "find_all_callers",
            2400.0, // time_ms: estimated 2-3 seconds for grep + parsing
            15000,  // tokens: estimated large unstructured output with grep results
            8,      // tool_calls: estimated multiple grep calls, file reads, etc.
        )?;
        Ok(result)
    }

    fn with_myceliums_estimate(&self) -> Result<MeasurementResult> {
        // Estimate with Myceliums: single graph query
        let result = estimate_baseline(
            "find_all_callers",
            320.0, // time_ms: estimated ~300ms for graph query + serialization
            2100,  // tokens: estimated structured JSON output is more compact
            2,     // tool_calls: estimated 1 graph query + 1 format call
        )?;
        Ok(result)
    }
}

/// Scenario 2: Detect impact of changing symbol Y
pub struct DetectImpactScenario;

impl Scenario for DetectImpactScenario {
    fn name(&self) -> &str {
        "detect_impact"
    }

    fn description(&self) -> &str {
        "Analyze the impact of changing symbol Y across the codebase (ILLUSTRATIVE ESTIMATE)"
    }

    fn baseline_estimate(&self) -> Result<MeasurementResult> {
        // Estimate baseline: manual call graph tracing
        let result = estimate_baseline(
            "detect_impact",
            3100.0, // time_ms: estimated 3+ seconds for manual tracing
            18000,  // tokens: estimated large unstructured trace output
            12,     // tool_calls: estimated file reads, grep, manual inspection
        )?;
        Ok(result)
    }

    fn with_myceliums_estimate(&self) -> Result<MeasurementResult> {
        // Estimate with Myceliums: structured impact detection
        let result = estimate_baseline(
            "detect_impact",
            280.0, // time_ms: estimated ~280ms for impact analysis
            2400,  // tokens: estimated structured impact report
            2,     // tool_calls: estimated 1 impact query + 1 format call
        )?;
        Ok(result)
    }
}

/// Scenario 3: List all symbols in community Z
pub struct ListCommunitySymbolsScenario;

impl Scenario for ListCommunitySymbolsScenario {
    fn name(&self) -> &str {
        "list_community_symbols"
    }

    fn description(&self) -> &str {
        "Find and list all symbols belonging to a specific community (ILLUSTRATIVE ESTIMATE)"
    }

    fn baseline_estimate(&self) -> Result<MeasurementResult> {
        // Estimate baseline: git grep + manual filtering
        let result = estimate_baseline(
            "list_community_symbols",
            2000.0, // time_ms: estimated 2 seconds for git grep + filtering
            12000,  // tokens: estimated unstructured grep output
            6,      // tool_calls: estimated git grep, file reads, filtering
        )?;
        Ok(result)
    }

    fn with_myceliums_estimate(&self) -> Result<MeasurementResult> {
        // Estimate with Myceliums: direct community query
        let result = estimate_baseline(
            "list_community_symbols",
            150.0, // time_ms: estimated ~150ms for direct query
            1500,  // tokens: estimated structured list
            1,     // tool_calls: estimated 1 community query
        )?;
        Ok(result)
    }
}

/// Scenario 4: Find functions handling request X
pub struct FindFunctionHandlersScenario;

impl Scenario for FindFunctionHandlersScenario {
    fn name(&self) -> &str {
        "find_function_handlers"
    }

    fn description(&self) -> &str {
        "Locate all functions that handle a specific type of request (ILLUSTRATIVE ESTIMATE)"
    }

    fn baseline_estimate(&self) -> Result<MeasurementResult> {
        // Estimate baseline: ripgrep + semantic analysis
        let result = estimate_baseline(
            "find_function_handlers",
            2800.0, // time_ms: estimated 2.8 seconds for ripgrep + analysis
            16000,  // tokens: estimated large unstructured results
            10,     // tool_calls: estimated multiple grep, file reads, analysis
        )?;
        Ok(result)
    }

    fn with_myceliums_estimate(&self) -> Result<MeasurementResult> {
        // Estimate with Myceliums: semantic search via MCP
        let result = estimate_baseline(
            "find_function_handlers",
            340.0, // time_ms: estimated ~340ms for semantic search
            2200,  // tokens: estimated structured results
            2,     // tool_calls: estimated semantic search + format
        )?;
        Ok(result)
    }
}

/// Scenario 5: Rename symbol X safely
pub struct RenameSafelyScenario;

impl Scenario for RenameSafelyScenario {
    fn name(&self) -> &str {
        "rename_safely"
    }

    fn description(&self) -> &str {
        "Safely rename a symbol with comprehensive impact detection (ILLUSTRATIVE ESTIMATE)"
    }

    fn baseline_estimate(&self) -> Result<MeasurementResult> {
        // Estimate baseline: manual find-replace + code review
        let result = estimate_baseline(
            "rename_safely",
            3500.0, // time_ms: estimated 3.5 seconds for full analysis
            20000,  // tokens: estimated large output for manual review
            15,     // tool_calls: estimated multiple searches, reads, analysis
        )?;
        Ok(result)
    }

    fn with_myceliums_estimate(&self) -> Result<MeasurementResult> {
        // Estimate with Myceliums: structured rename with impact analysis
        let result = estimate_baseline(
            "rename_safely",
            420.0, // time_ms: estimated ~420ms for rename operation
            2600,  // tokens: estimated structured rename report
            3,     // tool_calls: estimated impact detection, rename, format
        )?;
        Ok(result)
    }
}

/// Run all scenarios and return their metrics
pub fn run_all_scenarios() -> Result<Vec<ScenarioMetrics>> {
    let scenarios: Vec<Box<dyn Scenario>> = vec![
        Box::new(FindAllCallersScenario),
        Box::new(DetectImpactScenario),
        Box::new(ListCommunitySymbolsScenario),
        Box::new(FindFunctionHandlersScenario),
        Box::new(RenameSafelyScenario),
    ];

    let mut results = Vec::new();
    for scenario in scenarios {
        results.push(scenario.run()?);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_all_callers_scenario() {
        let scenario = FindAllCallersScenario;
        let metrics = scenario.run().expect("Failed to run scenario");

        // Verify that Myceliums is faster than baseline
        assert!(metrics.with_myceliums.time_ms < metrics.baseline.time_ms);
        assert!(metrics.improvements.time_reduction_percent > 0.0);
        assert!(metrics.improvements.token_reduction_percent > 0.0);
        assert!(metrics.improvements.tool_call_reduction_percent > 0.0);
        assert!(!metrics.is_verified); // These are estimates, not verified
    }

    #[test]
    fn test_run_all_scenarios() {
        let scenarios = run_all_scenarios().expect("Failed to run scenarios");
        assert_eq!(scenarios.len(), 5);

        for scenario in scenarios {
            assert!(!scenario.name.is_empty());
            assert!(!scenario.description.is_empty());
            assert!(scenario.improvements.time_reduction_percent > 0.0);
            assert!(!scenario.is_verified); // All are estimates
        }
    }
}
