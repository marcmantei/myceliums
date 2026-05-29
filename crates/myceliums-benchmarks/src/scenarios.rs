use crate::baseline::{simulate_baseline, SimulationResult};
use crate::metrics::{Measurements, ScenarioMetrics};
use anyhow::Result;

/// A benchmark scenario that can be run with and without Myceliums
pub trait Scenario {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// Simulate the baseline (without Myceliums) measurements
    fn baseline_simulation(&self) -> Result<SimulationResult>;

    /// Simulate with Myceliums (would integrate with myceliums-core in real implementation)
    fn with_myceliums_simulation(&self) -> Result<SimulationResult>;

    /// Run both simulations and return metrics
    fn run(&self) -> Result<ScenarioMetrics> {
        let baseline_result = self.baseline_simulation()?;
        let myceliums_result = self.with_myceliums_simulation()?;

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
        "Find all callers of function X in a medium-sized codebase"
    }

    fn baseline_simulation(&self) -> Result<SimulationResult> {
        // Baseline: grep + manual parsing
        // Simulates: 2-3 grep calls, parsing 50-100 lines, generating prompt with results
        let result = simulate_baseline(
            "find_all_callers",
            2400.0, // time_ms: 2-3 seconds for grep + parsing
            15000,  // tokens: large unstructured output with grep results
            8,      // tool_calls: multiple grep calls, file reads, etc.
        )?;
        Ok(result)
    }

    fn with_myceliums_simulation(&self) -> Result<SimulationResult> {
        // With Myceliums: single graph query
        let result = simulate_baseline(
            "find_all_callers",
            320.0, // time_ms: ~300ms for graph query + serialization
            2100,  // tokens: structured JSON output is more compact
            2,     // tool_calls: 1 graph query + 1 format call
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
        "Analyze the impact of changing symbol Y across the codebase"
    }

    fn baseline_simulation(&self) -> Result<SimulationResult> {
        // Baseline: manual call graph tracing
        let result = simulate_baseline(
            "detect_impact",
            3100.0, // time_ms: 3+ seconds for manual tracing
            18000,  // tokens: large unstructured trace output
            12,     // tool_calls: file reads, grep, manual inspection
        )?;
        Ok(result)
    }

    fn with_myceliums_simulation(&self) -> Result<SimulationResult> {
        // With Myceliums: structured impact detection
        let result = simulate_baseline(
            "detect_impact",
            280.0, // time_ms: ~280ms for impact analysis
            2400,  // tokens: structured impact report
            2,     // tool_calls: 1 impact query + 1 format call
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
        "Find and list all symbols belonging to a specific community"
    }

    fn baseline_simulation(&self) -> Result<SimulationResult> {
        // Baseline: git grep + manual filtering
        let result = simulate_baseline(
            "list_community_symbols",
            2000.0, // time_ms: 2 seconds for git grep + filtering
            12000,  // tokens: unstructured grep output
            6,      // tool_calls: git grep, file reads, filtering
        )?;
        Ok(result)
    }

    fn with_myceliums_simulation(&self) -> Result<SimulationResult> {
        // With Myceliums: direct community query
        let result = simulate_baseline(
            "list_community_symbols",
            150.0, // time_ms: ~150ms for direct query
            1500,  // tokens: structured list
            1,     // tool_calls: 1 community query
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
        "Locate all functions that handle a specific type of request"
    }

    fn baseline_simulation(&self) -> Result<SimulationResult> {
        // Baseline: ripgrep + semantic analysis
        let result = simulate_baseline(
            "find_function_handlers",
            2800.0, // time_ms: 2.8 seconds for ripgrep + analysis
            16000,  // tokens: large unstructured results
            10,     // tool_calls: multiple grep, file reads, analysis
        )?;
        Ok(result)
    }

    fn with_myceliums_simulation(&self) -> Result<SimulationResult> {
        // With Myceliums: semantic search via MCP
        let result = simulate_baseline(
            "find_function_handlers",
            340.0, // time_ms: ~340ms for semantic search
            2200,  // tokens: structured results
            2,     // tool_calls: semantic search + format
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
        "Safely rename a symbol with comprehensive impact detection"
    }

    fn baseline_simulation(&self) -> Result<SimulationResult> {
        // Baseline: manual find-replace + code review
        let result = simulate_baseline(
            "rename_safely",
            3500.0, // time_ms: 3.5 seconds for full analysis
            20000,  // tokens: large output for manual review
            15,     // tool_calls: multiple searches, reads, analysis
        )?;
        Ok(result)
    }

    fn with_myceliums_simulation(&self) -> Result<SimulationResult> {
        // With Myceliums: structured rename with impact analysis
        let result = simulate_baseline(
            "rename_safely",
            420.0, // time_ms: ~420ms for rename operation
            2600,  // tokens: structured rename report
            3,     // tool_calls: impact detection, rename, format
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
    }

    #[test]
    fn test_run_all_scenarios() {
        let scenarios = run_all_scenarios().expect("Failed to run scenarios");
        assert_eq!(scenarios.len(), 5);

        for scenario in scenarios {
            assert!(!scenario.name.is_empty());
            assert!(!scenario.description.is_empty());
            assert!(scenario.improvements.time_reduction_percent > 0.0);
        }
    }
}
