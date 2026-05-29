#!/usr/bin/env bash
# Script to run benchmarks and generate reports

set -e

BENCH_NAME="${1:-all}"
OUTPUT_DIR="${2:-.}"

echo "Running Myceliums Benchmark Suite"
echo "=================================="
echo "Target: $BENCH_NAME"
echo "Output: $OUTPUT_DIR"
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR/results"

# Run benchmarks
if [ "$BENCH_NAME" = "all" ]; then
    echo "Running all benchmarks..."
    cargo bench -p myceliums-benchmarks -- --output-format bencher 2>&1 | tee "$OUTPUT_DIR/benchmark.log"
else
    echo "Running $BENCH_NAME benchmarks..."
    cargo bench -p myceliums-benchmarks --bench "$BENCH_NAME" -- --output-format bencher 2>&1 | tee "$OUTPUT_DIR/benchmark.log"
fi

# Copy Criterion results
if [ -d "target/criterion" ]; then
    echo ""
    echo "Copying Criterion results..."
    cp -r target/criterion "$OUTPUT_DIR/results/" || true
fi

# Generate timestamp
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)

# Create JSON report template
cat > "$OUTPUT_DIR/results/benchmark-report.json" << EOF
{
  "version": "$VERSION",
  "timestamp": "$TIMESTAMP",
  "results": [],
  "notes": "Run 'cargo bench -p myceliums-benchmarks' to update results"
}
EOF

echo ""
echo "✅ Benchmark run complete!"
echo ""
echo "Results saved to:"
echo "  - $OUTPUT_DIR/results/criterion/"
echo "  - $OUTPUT_DIR/results/benchmark-report.json"
echo ""
echo "View HTML report:"
echo "  open $OUTPUT_DIR/results/criterion/report/index.html"
echo ""
