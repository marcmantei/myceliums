# Myceliums Benchmark Suite

A comprehensive, reproducible benchmark suite for measuring Myceliums's performance across indexing, query, and agent task categories.

## Overview

This benchmark suite enables anyone to:
- Measure Myceliumss's real performance metrics locally
- Compare performance with and without indexing
- Verify improvements over time
- Generate shareable reports for CI/CD integration

## Benchmark Categories

### 1. Indexing Performance

Measures the time required to analyze and index projects of various sizes:

- **Small Project (10 files)** - TypeScript and Python
- **Medium Project (100 files)** - TypeScript
- **Large Project (500 files)** - Python

Includes tree-sitter parsing overhead as part of the baseline.

### 2. Query Performance

Benchmarks symbol lookup and graph traversal operations:

- **Simple Symbol Lookup** - Find a symbol across indexed files (~100ms target)
- **Complex Call Graph Query** - Traverse call relationships (~500ms target)
- **Graph Traversal** - BFS through dependency graph (~1s for large graphs)

### 3. Agent Task Completion

Simulates agent workflows to measure end-to-end performance:

- **Code Understanding** - Parse and analyze code structure
- **Context Search** - Find related code through semantic search
- **Impact Analysis** - Determine what code depends on a change
- **Combined Workflow** - Full agent task simulation with Myceliums optimization

## Running Benchmarks

### Run All Benchmarks

```bash
cargo bench -p myceliums-benchmarks
```

### Run Specific Benchmark Category

```bash
# Indexing benchmarks only
cargo bench -p myceliums-benchmarks --bench indexing

# Query benchmarks only
cargo bench -p myceliums-benchmarks --bench query

# Agent task benchmarks only
cargo bench -p myceliums-benchmarks --bench agent_tasks
```

### Generate Reports

After running benchmarks, Criterion.rs automatically generates HTML reports in:

```
target/criterion/
```

Open `target/criterion/report/index.html` in a browser to view detailed charts and comparisons.

## JSON Report Output

The benchmark suite can generate JSON reports for CI/CD integration:

```json
{
  "version": "0.1.0",
  "timestamp": "2024-03-14T10:30:00Z",
  "results": [
    {
      "name": "indexing_small_ts_10_files",
      "category": "indexing",
      "duration_ms": 45.23,
      "files_processed": 10,
      "symbols_found": 28,
      "memory_peak_mb": 12.5,
      "timestamp": "2024-03-14T10:30:00Z"
    }
  ],
  "time_reduction_pct": 35.5,
  "cost_savings_pct": 28.3,
  "fewer_tool_calls_pct": 42.1
}
```

## Benchmark Methodology

### Baseline Metrics

Each benchmark includes:

- **Duration** - Time to complete the operation (milliseconds)
- **Files Processed** - Number of source files analyzed
- **Symbols Found** - Number of unique symbols extracted
- **Memory Peak** - Maximum memory usage during operation (MB)
- **Timestamp** - When the benchmark was run

### Synthetic Test Fixtures

The suite uses procedurally generated test projects to ensure:

- **Reproducibility** - Same structure every run
- **Scalability** - Test different project sizes
- **Isolation** - No dependency on real-world codebases

### Tree-sitter Parsing Overhead

All indexing benchmarks include the parsing overhead from tree-sitter, as this is part of the real-world performance profile.

## CI Integration

### GitHub Actions Workflow

Benchmarks can be run on every merge or release:

```yaml
- name: Run benchmarks
  run: cargo bench -p myceliums-benchmarks

- name: Upload results
  uses: actions/upload-artifact@v3
  with:
    name: benchmark-results
    path: target/criterion/
```

### Storing Results

Results can be stored as:

1. **JSON artifacts** - For programmatic analysis
2. **GitHub artifacts** - For easy access in CI
3. **S3/Cloud storage** - For historical tracking

## Performance Targets

Based on the issue specification:

| Category | Benchmark | Target |
|----------|-----------|--------|
| Indexing | Small (100 files) | < 1s |
| Indexing | Medium (1,000 files) | < 10s |
| Indexing | Large (10,000 files) | < 60s |
| Query | Simple lookup | < 100ms |
| Query | Complex call graph | < 500ms |
| Query | Graph traversal | < 1s |
| Agent | Task completion | 30%+ faster with Myceliums |

## Future Enhancements

- [ ] Real SWE-bench Lite integration
- [ ] Custom task benchmarks
- [ ] Comparison with other tools (Noodlbox, HydraDB)
- [ ] Public benchmark dashboard
- [ ] Historical trend tracking
- [ ] Memory profiling with detailed allocation tracking
- [ ] Streaming processing benchmarks

## Development

### Adding a New Benchmark

1. Create a new file in `benches/`:
   ```rust
   use criterion::{black_box, criterion_group, criterion_main, Criterion};

   fn my_benchmark(c: &mut Criterion) {
       c.bench_function("my_operation", |b| {
           b.iter(|| /* operation */)
       });
   }

   criterion_group!(benches, my_benchmark);
   criterion_main!(benches);
   ```

2. Update `Cargo.toml` to add the new bench:
   ```toml
   [[bench]]
   name = "my_benchmark"
   harness = false
   ```

3. Run it:
   ```bash
   cargo bench -p myceliums-benchmarks --bench my_benchmark
   ```

### Adding Test Fixtures

The `FixtureGenerator` can be extended with new project types:

```rust
impl FixtureGenerator {
    pub fn generate_rust_project(&self) -> Result<PathBuf> {
        // Implementation
    }
}
```

## References

- [Criterion.rs](https://github.com/bheisler/criterion.rs) - Rust benchmarking framework
- [SWE-bench](https://www.swebench.com/) - Software engineering benchmarks
- Issue #38 - Original benchmark suite requirements
- Issue #42 - Website integration and performance metrics

## License

MIT
