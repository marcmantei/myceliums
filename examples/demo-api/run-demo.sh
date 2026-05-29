#!/bin/bash

# Demo script for running Myceliums analysis on the demo API
# This script demonstrates:
# 1. Code analysis (myc analyze)
# 2. Impact analysis (myc impact)
# 3. Community detection (myc communities)
# 4. Process tracing (myc processes)
# 5. Graph queries (myc query)

set -e

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="$DEMO_DIR/output"

echo "=================================================="
echo "Myceliums Demo API Analysis"
echo "=================================================="
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "Step 1: Analyzing the demo API codebase..."
echo "Running: myc analyze $DEMO_DIR"
echo ""

myc analyze "$DEMO_DIR" 2>&1 | tee "$OUTPUT_DIR/analyze-output.txt"

echo ""
echo "=================================================="
echo "Step 2: Running impact analysis with sample diff..."
echo "Running: myc impact --diff $DEMO_DIR/sample.diff"
echo ""

# Run impact analysis and save to JSON
myc impact --diff "$DEMO_DIR/sample.diff" --json 2>&1 | tee "$OUTPUT_DIR/impact-result.json"

echo ""
echo "=================================================="
echo "Step 3: Detecting code communities..."
echo "Running: myc communities"
echo ""

myc communities --json 2>&1 | tee "$OUTPUT_DIR/communities-result.json"

echo ""
echo "=================================================="
echo "Step 4: Tracing execution processes..."
echo "Running: myc processes"
echo ""

myc processes --json 2>&1 | tee "$OUTPUT_DIR/processes-result.json"

echo ""
echo "=================================================="
echo "Step 5: Querying the code graph..."
echo "Finding all CALLS relationships..."
echo ""

myc query "MATCH (n)-[:CALLS]->(m) RETURN n.name, m.name LIMIT 20" --json 2>&1 | tee "$OUTPUT_DIR/graph-query.json"

echo ""
echo "=================================================="
echo "Step 6: Finding functions affected by auth changes..."
echo ""

myc query "MATCH (n {file: 'auth.py'})-[:CALLS*0..3]->(m) RETURN DISTINCT m.name" --json 2>&1 | tee "$OUTPUT_DIR/auth-downstream.json"

echo ""
echo "=================================================="
echo "Demo Complete!"
echo "=================================================="
echo ""
echo "Output files saved to: $OUTPUT_DIR"
echo ""
echo "Files generated:"
echo "  - analyze-output.txt      : Initial codebase analysis"
echo "  - impact-result.json      : Impact analysis results"
echo "  - communities-result.json : Code community detection"
echo "  - processes-result.json   : Execution process traces"
echo "  - graph-query.json        : Sample graph queries"
echo "  - auth-downstream.json    : Functions affected by auth changes"
echo ""
echo "To visualize the graph interactively:"
echo "  myc serve"
echo ""
