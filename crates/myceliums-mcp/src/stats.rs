//! Local, in-memory statistics of the tool calls this server answered.
//!
//! Nothing here leaves the machine: the numbers are read with the
//! `server_stats` tool (toolset `admin`) or written to the file given with
//! `myc mcp --stats-file <path>`. There is no network telemetry.
//!
//! Memory is bounded: one entry per tool in the catalog (every other name is
//! counted under [`UNKNOWN_TOOL`]), and latency percentiles come from the
//! last [`LATENCY_WINDOW`] calls of each tool.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many recent latencies per tool the percentiles are computed from.
pub(crate) const LATENCY_WINDOW: usize = 1024;

/// The key for calls to names that are not tools of this server.
pub(crate) const UNKNOWN_TOOL: &str = "(unknown tool)";

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A result without `isError`.
    Ok,
    /// A result with `isError: true` (a failure the agent can act on).
    IsError,
    /// A JSON-RPC error (`invalid_params`, `internal_error`, unknown tool).
    ProtocolError,
    /// The request was dropped before it answered (client gone or cancelled).
    Cancelled,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::IsError => "is_error",
            Outcome::ProtocolError => "protocol_error",
            Outcome::Cancelled => "cancelled",
        }
    }
}

/// Rough token count of `bytes` of text: one token per four bytes, rounded up.
pub(crate) fn approx_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

#[derive(Default)]
struct ToolStats {
    calls: u64,
    is_errors: u64,
    protocol_errors: u64,
    cancelled: u64,
    result_bytes: u64,
    /// The last [`LATENCY_WINDOW`] latencies in microseconds, as a ring.
    latencies_us: Vec<u64>,
    next: usize,
    max_us: u64,
}

impl ToolStats {
    fn record(&mut self, outcome: Outcome, elapsed: Duration, bytes: usize) {
        self.calls += 1;
        match outcome {
            Outcome::Ok => {}
            Outcome::IsError => self.is_errors += 1,
            Outcome::ProtocolError => self.protocol_errors += 1,
            Outcome::Cancelled => self.cancelled += 1,
        }
        self.result_bytes += bytes as u64;
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.max_us = self.max_us.max(us);
        if self.latencies_us.len() < LATENCY_WINDOW {
            self.latencies_us.push(us);
        } else {
            self.latencies_us[self.next] = us;
        }
        self.next = (self.next + 1) % LATENCY_WINDOW;
    }
}

/// Nearest-rank percentile (`p` in 0..=100) of unsorted samples.
fn percentile(samples: &[u64], p: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn ms(us: u64) -> f64 {
    // Two decimals: microsecond noise is not information here.
    (us as f64 / 10.0).round() / 100.0
}

/// Per-tool call statistics of one server process (shared by every HTTP
/// session).
pub(crate) struct CallStats {
    started: Instant,
    tools: Mutex<BTreeMap<&'static str, ToolStats>>,
}

impl Default for CallStats {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            tools: Mutex::new(BTreeMap::new()),
        }
    }
}

impl CallStats {
    /// Records one finished call of `tool`.
    pub(crate) fn record(&self, tool: &str, outcome: Outcome, elapsed: Duration, bytes: usize) {
        let key = crate::toolsets::static_name(tool).unwrap_or(UNKNOWN_TOOL);
        let mut tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        tools
            .entry(key)
            .or_default()
            .record(outcome, elapsed, bytes);
    }

    /// The numbers so far.
    pub(crate) fn snapshot(&self) -> StatsSnapshot {
        let tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        let tools: Vec<ToolSnapshot> = tools
            .iter()
            .map(|(name, s)| ToolSnapshot {
                tool: name.to_string(),
                toolset: crate::toolsets::toolset_of(name).map(|t| t.name()),
                calls: s.calls,
                is_errors: s.is_errors,
                protocol_errors: s.protocol_errors,
                cancelled: s.cancelled,
                p50_ms: ms(percentile(&s.latencies_us, 50)),
                p95_ms: ms(percentile(&s.latencies_us, 95)),
                max_ms: ms(s.max_us),
                result_bytes: s.result_bytes,
                approx_tokens: approx_tokens(s.result_bytes as usize) as u64,
            })
            .collect();
        StatsSnapshot {
            uptime_secs: self.started.elapsed().as_secs(),
            total_calls: tools.iter().map(|t| t.calls).sum(),
            latency_window: LATENCY_WINDOW,
            tools,
        }
    }

    /// The number of calls recorded so far.
    pub(crate) fn total_calls(&self) -> u64 {
        let tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        tools.values().map(|s| s.calls).sum()
    }
}

/// A point-in-time copy of [`CallStats`] (the JSON of `--stats-file`).
#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    pub uptime_secs: u64,
    pub total_calls: u64,
    /// Percentiles are over at most this many recent calls per tool.
    pub latency_window: usize,
    pub tools: Vec<ToolSnapshot>,
}

/// The numbers of one tool.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSnapshot {
    pub tool: String,
    pub toolset: Option<&'static str>,
    pub calls: u64,
    /// Results with `isError: true`.
    pub is_errors: u64,
    /// JSON-RPC errors.
    pub protocol_errors: u64,
    /// Calls dropped before they answered.
    pub cancelled: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    /// Bytes of result text (what the agent reads), summed.
    pub result_bytes: u64,
    /// `result_bytes / 4`, rounded up.
    pub approx_tokens: u64,
}

impl StatsSnapshot {
    /// The text the `server_stats` tool returns: one line per tool, busiest
    /// first.
    pub(crate) fn to_text(&self) -> String {
        let mut out = format!(
            "Tool calls since this server started {} s ago: {} (local only, never sent anywhere).\n",
            self.uptime_secs, self.total_calls
        );
        if self.tools.is_empty() {
            return out;
        }
        out.push_str(&format!(
            "Latency percentiles cover the last {} calls of each tool.\n\n",
            self.latency_window
        ));
        out.push_str(
            "tool | toolset | calls | isError | protocol errors | cancelled | p50 ms | p95 ms | max ms | result bytes | ~tokens\n",
        );
        let mut tools: Vec<&ToolSnapshot> = self.tools.iter().collect();
        tools.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.tool.cmp(&b.tool)));
        for t in tools {
            out.push_str(&format!(
                "{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}\n",
                t.tool,
                t.toolset.unwrap_or("-"),
                t.calls,
                t.is_errors,
                t.protocol_errors,
                t.cancelled,
                t.p50_ms,
                t.p95_ms,
                t.max_ms,
                t.result_bytes,
                t.approx_tokens
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms_(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn aggregates_counts_bytes_and_percentiles_per_tool() {
        let stats = CallStats::default();
        for i in 1..=100 {
            stats.record("search_code", Outcome::Ok, ms_(i), 40);
        }
        stats.record("search_code", Outcome::IsError, ms_(1), 4);
        stats.record("analyze", Outcome::ProtocolError, ms_(7), 0);
        stats.record("analyze", Outcome::Cancelled, ms_(9), 0);

        let snap = stats.snapshot();
        assert_eq!(snap.total_calls, 103);
        let search = snap.tools.iter().find(|t| t.tool == "search_code").unwrap();
        assert_eq!(search.toolset, Some("core"));
        assert_eq!(search.calls, 101);
        assert_eq!(search.is_errors, 1);
        assert_eq!(search.protocol_errors, 0);
        assert_eq!(search.result_bytes, 4004);
        assert_eq!(search.approx_tokens, 1001);
        assert_eq!(search.p50_ms, 50.0);
        assert_eq!(search.p95_ms, 95.0);
        assert_eq!(search.max_ms, 100.0);
        let analyze = snap.tools.iter().find(|t| t.tool == "analyze").unwrap();
        assert_eq!(
            (analyze.calls, analyze.protocol_errors, analyze.cancelled),
            (2, 1, 1)
        );

        let text = snap.to_text();
        assert!(text.contains("Tool calls since"), "{text}");
        let first_row = text.lines().nth(4).unwrap();
        assert!(
            first_row.starts_with("search_code | core | 101 | 1 |"),
            "{text}"
        );
    }

    #[test]
    fn memory_is_bounded_by_the_catalog_and_the_latency_window() {
        let stats = CallStats::default();
        for i in 0..5_000 {
            stats.record(&format!("made_up_{i}"), Outcome::ProtocolError, ms_(1), 0);
        }
        for i in 0..(LATENCY_WINDOW as u64 * 3) {
            // The newest window holds only the slow calls.
            let elapsed = if i < LATENCY_WINDOW as u64 * 2 {
                1
            } else {
                500
            };
            stats.record("find_callers", Outcome::Ok, ms_(elapsed), 1);
        }
        let snap = stats.snapshot();
        assert_eq!(snap.tools.len(), 2, "unknown names share one entry");
        let unknown = snap.tools.iter().find(|t| t.tool == UNKNOWN_TOOL).unwrap();
        assert_eq!(unknown.calls, 5_000);
        let callers = snap
            .tools
            .iter()
            .find(|t| t.tool == "find_callers")
            .unwrap();
        assert_eq!(callers.p50_ms, 500.0, "percentiles cover the recent window");
        let tools = stats.tools.lock().unwrap();
        assert!(tools
            .values()
            .all(|t| t.latencies_us.len() <= LATENCY_WINDOW));
    }

    #[test]
    fn empty_stats_render() {
        let snap = CallStats::default().snapshot();
        assert_eq!(snap.total_calls, 0);
        assert!(snap.to_text().contains(": 0 "));
        assert_eq!(percentile(&[], 95), 0);
        assert_eq!(approx_tokens(0), 0);
        assert_eq!(approx_tokens(5), 2);
    }
}
