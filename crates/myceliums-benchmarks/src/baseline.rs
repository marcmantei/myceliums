use anyhow::Result;

/// Measurement result - stores actual or estimated metrics
#[derive(Debug, Clone)]
pub struct MeasurementResult {
    pub time_ms: f64,
    pub tokens: u32,
    pub tool_calls: u32,
    /// True if this is a real measurement, false if it's an illustrative estimate
    pub is_verified: bool,
}

/// Estimate baseline measurements for a given scenario
///
/// **IMPORTANT**: These are illustrative estimates based on theoretical token costs,
/// NOT verified measurements. They are provided for comparison purposes only and
/// should not be treated as definitive performance benchmarks.
///
/// Real measurements would require:
/// - Actual grep/ripgrep execution on real codebases
/// - Real timing of file I/O and parsing
/// - Actual token counting from language model calls
/// - Real tool invocation tracing
pub fn estimate_baseline(
    _scenario: &str,
    time_ms: f64,
    tokens: u32,
    tool_calls: u32,
) -> Result<MeasurementResult> {
    Ok(MeasurementResult {
        time_ms,
        tokens,
        tool_calls,
        is_verified: false, // These are estimates, not verified measurements
    })
}

/// Estimate Myceliums measurements for a given scenario
///
/// **IMPORTANT**: These are illustrative estimates, NOT verified measurements.
/// They represent theoretical improvements from structured graph queries.
///
/// Real measurements would require:
/// - Integration with actual myceliums-core implementation
/// - Real graph query execution on indexed codebases
/// - Actual token counting from structured output
/// - Real timing under production conditions
pub fn estimate_myceliums(
    _scenario: &str,
    time_ms: f64,
    tokens: u32,
    tool_calls: u32,
) -> Result<MeasurementResult> {
    Ok(MeasurementResult {
        time_ms,
        tokens,
        tool_calls,
        is_verified: false, // These are estimates, not verified measurements
    })
}

// Backward compatibility aliases (deprecated)
#[deprecated(
    since = "0.2.0",
    note = "Use estimate_baseline() instead. These functions returned fabricated data."
)]
pub fn simulate_baseline(
    _scenario: &str,
    time_ms: f64,
    tokens: u32,
    tool_calls: u32,
) -> Result<MeasurementResult> {
    estimate_baseline(_scenario, time_ms, tokens, tool_calls)
}

#[deprecated(
    since = "0.2.0",
    note = "Use estimate_myceliums() instead. These functions returned fabricated data."
)]
pub fn simulate_myceliums(
    _scenario: &str,
    time_ms: f64,
    tokens: u32,
    tool_calls: u32,
) -> Result<MeasurementResult> {
    estimate_myceliums(_scenario, time_ms, tokens, tool_calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_baseline() {
        let result = estimate_baseline("test", 1000.0, 5000, 5).expect("Failed to estimate");
        assert_eq!(result.time_ms, 1000.0);
        assert_eq!(result.tokens, 5000);
        assert_eq!(result.tool_calls, 5);
        assert!(!result.is_verified); // These are estimates, not verified
    }

    #[test]
    fn test_estimate_myceliums() {
        let result = estimate_myceliums("test", 100.0, 500, 1).expect("Failed to estimate");
        assert_eq!(result.time_ms, 100.0);
        assert_eq!(result.tokens, 500);
        assert_eq!(result.tool_calls, 1);
        assert!(!result.is_verified); // These are estimates, not verified
    }

    #[test]
    #[allow(deprecated)]
    fn test_backward_compat_simulate_baseline() {
        // Legacy function should still work but be marked as deprecated
        let result = simulate_baseline("test", 1000.0, 5000, 5).expect("Failed to simulate");
        assert_eq!(result.time_ms, 1000.0);
        assert!(!result.is_verified);
    }

    #[test]
    #[allow(deprecated)]
    fn test_backward_compat_simulate_myceliums() {
        // Legacy function should still work but be marked as deprecated
        let result = simulate_myceliums("test", 100.0, 500, 1).expect("Failed to simulate");
        assert_eq!(result.time_ms, 100.0);
        assert!(!result.is_verified);
    }
}
