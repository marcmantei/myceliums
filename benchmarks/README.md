# Verified Metrics

This directory contains verified benchmark results for each Myceliums release.

## Files

- `metrics/v{VERSION}.json` — Complete benchmark results for version {VERSION}
- `latest.json` — Symlink or copy of newest release metrics
- `METHODOLOGY.md` — Detailed methodology for how metrics are measured

## JSON Schema

Each metrics file contains:

```json
{
  "version": "0.1.0",
  "timestamp": "2026-03-13T12:38:55Z",
  "environment": {
    "os": "linux",
    "cpu_count": 4,
    "memory_gb": 16,
    "rust_version": "1.75.0"
  },
  "scenarios": [
    {
      "name": "find_all_callers",
      "description": "Find all callers of function X",
      "baseline": {
        "time_ms": 2400,
        "tokens": 15000,
        "tool_calls": 8
      },
      "with_myceliums": {
        "time_ms": 320,
        "tokens": 2100,
        "tool_calls": 2
      },
      "improvements": {
        "time_reduction_percent": 86.7,
        "token_reduction_percent": 86.0,
        "tool_call_reduction_percent": 75.0
      }
    }
  ],
  "aggregate": {
    "avg_time_reduction_percent": 78.5,
    "avg_token_reduction_percent": 81.2,
    "avg_tool_call_reduction_percent": 72.4
  }
}
```

## Website Integration

The website loads the latest metrics from `benchmarks/latest.json` and displays them in the hero section:

```typescript
// website/src/lib/metrics.ts
export async function getLatestMetrics() {
  const response = await fetch(
    'https://raw.githubusercontent.com/marcmantei/myceliums/main/benchmarks/latest.json'
  );
  return response.json();
}
```

## Metrics by Release

| Version | Time Reduction | Token Reduction | Fewer Tool Calls |
|---------|----------------|-----------------|------------------|
| 0.1.0   | 78.5%          | 81.2%           | 72.4%            |

## Methodology

See [METHODOLOGY.md](./METHODOLOGY.md) for detailed information about:
- How each scenario is defined and measured
- Baseline assumptions
- Token counting method
- Environment specifications
- Reproducibility instructions

## Local Development

To run benchmarks locally (post-MVP implementation):

```bash
cargo bench --release -p myceliums-benchmarks
```

Results will be printed to stdout and saved to `benchmarks/metrics/v{VERSION}.json`.
