#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Myceliums Large-Repo Benchmark
# =============================================================================
#
# Proves that myceliums scales to real-world large codebases (5,000+ files).
#
# Modes:
#   1. Synthetic — generates a 5,000+ file multi-language project via the Rust
#      fixture generator in myceliums-benchmarks, then benchmarks myc against it.
#   2. Real-world — clones well-known OSS repos and benchmarks myc on each.
#
# Usage:
#   ./benchmarks/large-repo-benchmark.sh                     # synthetic only
#   ./benchmarks/large-repo-benchmark.sh --real               # synthetic + real repos
#   ./benchmarks/large-repo-benchmark.sh --real-only           # real repos only
#   ./benchmarks/large-repo-benchmark.sh --repo django/django  # single real repo
#   ./benchmarks/large-repo-benchmark.sh --help
#
# Output:
#   benchmarks/RESULTS.md  — human-readable markdown report
#   /tmp/myceliums-large-bench/results.json — machine-readable results
#
# Requirements:
#   - cargo (to build myc and the fixture generator binary)
#   - git (for cloning real-world repos)
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO="${CARGO:-/Users/siralot/.cargo/bin/cargo}"
MYC="$PROJECT_ROOT/target/release/myc"
BENCH_TMP="/tmp/myceliums-large-bench"
RESULTS_JSON="$BENCH_TMP/results.json"
RESULTS_MD="$SCRIPT_DIR/RESULTS.md"

# Real-world repos to benchmark (owner/name  branch  language)
REAL_REPOS=(
    "django/django|main|python"
    "microsoft/TypeScript|main|typescript"
    "golang/go|master|go"
    "expressjs/express|master|javascript"
    "gin-gonic/gin|master|go"
    "tokio-rs/tokio|master|rust"
)

# ---- Args ------------------------------------------------------------------
MODE="synthetic"    # synthetic | real | both | single
SINGLE_REPO=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --real)       MODE="both"; shift ;;
        --real-only)  MODE="real"; shift ;;
        --repo)       MODE="single"; SINGLE_REPO="$2"; shift 2 ;;
        --help|-h)
            echo "Usage: $0 [--real] [--real-only] [--repo owner/repo] [--help]"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ---- Helpers ----------------------------------------------------------------
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; BOLD='\033[1m'; NC='\033[0m'

log()  { printf "${BLUE}>>>${NC} %s\n" "$*"; }
ok()   { printf "${GREEN}  OK${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}  WARN${NC} %s\n" "$*"; }

# Cross-platform millisecond timer (macOS + Linux)
now_ms() {
    if command -v gdate &>/dev/null; then
        gdate +%s%3N
    elif date +%s%3N &>/dev/null 2>&1; then
        date +%s%3N
    else
        # Fallback: seconds * 1000
        echo $(( $(date +%s) * 1000 ))
    fi
}

elapsed_ms() {
    local start=$1 end
    end=$(now_ms)
    echo $(( end - start ))
}

human_time() {
    local ms=$1
    if [[ $ms -lt 1000 ]]; then
        echo "${ms}ms"
    elif [[ $ms -lt 60000 ]]; then
        echo "$(( ms / 1000 )).$(( (ms % 1000) / 100 ))s"
    else
        local secs=$(( ms / 1000 ))
        echo "$(( secs / 60 ))m $(( secs % 60 ))s"
    fi
}

# ---- Build ------------------------------------------------------------------
build_myc() {
    log "Building myc (release)..."
    "$CARGO" build --release --manifest-path "$PROJECT_ROOT/Cargo.toml" -p myc 2>&1 | tail -3
    ok "myc built"
}

# ---- Synthetic benchmark ----------------------------------------------------
run_synthetic() {
    log "Generating synthetic large project (5,000+ files, 6 languages)..."
    local synth_dir="$BENCH_TMP/synthetic-large-project"
    rm -rf "$synth_dir"

    # Use a small Rust program that calls the generator from myceliums-benchmarks.
    # We write it to a temp file and run it with `cargo run`.
    local gen_bin="$BENCH_TMP/gen_large_repo.rs"
    mkdir -p "$BENCH_TMP"
    cat > "$gen_bin" << 'RUSTEOF'
use myceliums_benchmarks::large_repo::LargeRepoGenerator;
use std::env;

fn main() {
    let out_dir = env::args().nth(1).expect("usage: gen_large_repo <output-dir>");
    let gen = LargeRepoGenerator::new(out_dir.into());
    let summary = gen.generate().expect("failed to generate large repo");
    println!("Generated {} files in {}", summary.total_files, summary.root.display());
}
RUSTEOF

    # Build and run the generator via cargo
    "$CARGO" build --release --manifest-path "$PROJECT_ROOT/Cargo.toml" -p myceliums-benchmarks 2>&1 | tail -3

    # Instead of compiling a standalone binary, we use a benchmark binary.
    # Create a tiny Cargo project that depends on myceliums-benchmarks.
    local gen_project="$BENCH_TMP/gen-project"
    rm -rf "$gen_project"
    mkdir -p "$gen_project/src"
    cat > "$gen_project/Cargo.toml" << EOF
[package]
name = "gen-large-repo"
version = "0.1.0"
edition = "2021"

[dependencies]
myceliums-benchmarks = { path = "$PROJECT_ROOT/crates/myceliums-benchmarks" }
EOF
    cp "$gen_bin" "$gen_project/src/main.rs"

    log "Compiling fixture generator..."
    "$CARGO" build --release --manifest-path "$gen_project/Cargo.toml" 2>&1 | tail -3
    ok "Fixture generator compiled"

    log "Running fixture generator..."
    "$gen_project/target/release/gen-large-repo" "$synth_dir"

    local file_count
    file_count=$(find "$synth_dir" -type f | wc -l | tr -d ' ')
    ok "Synthetic project: $file_count files"

    # ---- Analysis benchmarks ----
    log "Benchmarking: myc analyze --skip-embeddings"
    local t0
    t0=$(now_ms)
    local analyze_output
    analyze_output=$("$MYC" analyze "$synth_dir" --skip-embeddings --force 2>&1) || true
    local time_skip_embed
    time_skip_embed=$(elapsed_ms "$t0")
    ok "Analysis (skip-embeddings): $(human_time $time_skip_embed)"

    # Extract stats from output
    local symbols rels repo_id
    symbols=$(echo "$analyze_output" | grep -i "symbols:" | head -1 | awk '{print $NF}' || echo "0")
    rels=$(echo "$analyze_output" | grep -i "relationships:" | head -1 | awk '{print $NF}' || echo "0")
    repo_id=$(echo "$analyze_output" | grep -i "repository id:" | head -1 | awk '{print $NF}' || echo "")

    if [[ -z "$repo_id" ]]; then
        warn "Could not extract repo ID. Analysis output:"
        echo "$analyze_output" | head -20
        return 1
    fi

    ok "Symbols: $symbols | Relationships: $rels | Repo ID: $repo_id"

    # ---- Cached analysis ----
    log "Benchmarking: myc analyze (cached, no --force)"
    t0=$(now_ms)
    "$MYC" analyze "$synth_dir" --skip-embeddings 2>&1 >/dev/null || true
    local time_cached
    time_cached=$(elapsed_ms "$t0")
    ok "Cached analysis: $(human_time $time_cached)"

    # ---- Query benchmarks ----
    log "Benchmarking: BM25 search"
    t0=$(now_ms)
    "$MYC" search "process user" --repo "$repo_id" --limit 10 2>&1 >/dev/null || true
    local time_search
    time_search=$(elapsed_ms "$t0")
    ok "BM25 search: $(human_time $time_search)"

    log "Benchmarking: Cypher query (list classes)"
    t0=$(now_ms)
    "$MYC" query "MATCH (s) WHERE s.kind = 'Class' RETURN s.name LIMIT 30" --repo "$repo_id" 2>&1 >/dev/null || true
    local time_cypher
    time_cypher=$(elapsed_ms "$t0")
    ok "Cypher query: $(human_time $time_cypher)"

    log "Benchmarking: impact detection"
    t0=$(now_ms)
    "$MYC" impact --repo "$repo_id" --diff "--- a/packages/ts-core/src/api/api_0.ts
+++ b/packages/ts-core/src/api/api_0.ts
@@ -1,3 +1,3 @@
-import { User, Config } from \"../types\";
+import { User, Config, ApiError } from \"../types\";
" 2>&1 >/dev/null || true
    local time_impact
    time_impact=$(elapsed_ms "$t0")
    ok "Impact detection: $(human_time $time_impact)"

    log "Benchmarking: process tracing"
    t0=$(now_ms)
    "$MYC" processes "$repo_id" --limit 5 2>&1 >/dev/null || true
    local time_processes
    time_processes=$(elapsed_ms "$t0")
    ok "Process tracing: $(human_time $time_processes)"

    # ---- Store results ----
    SYNTH_FILE_COUNT="$file_count"
    SYNTH_SYMBOLS="$symbols"
    SYNTH_RELS="$rels"
    SYNTH_TIME_SKIP_EMBED="$time_skip_embed"
    SYNTH_TIME_CACHED="$time_cached"
    SYNTH_TIME_SEARCH="$time_search"
    SYNTH_TIME_CYPHER="$time_cypher"
    SYNTH_TIME_IMPACT="$time_impact"
    SYNTH_TIME_PROCESSES="$time_processes"

    # Cleanup
    "$MYC" delete "$repo_id" 2>/dev/null || true
}

# ---- Real-world repo benchmark ---------------------------------------------
declare -a REAL_RESULTS=()

run_real_repo() {
    local spec="$1"
    IFS='|' read -r repo branch lang <<< "$spec"
    local repo_name="${repo##*/}"
    local clone_dir="$BENCH_TMP/repos/$repo_name"

    log "Real-world benchmark: $repo ($lang)"

    # Clone if needed
    if [[ ! -d "$clone_dir" ]]; then
        log "Cloning $repo (shallow)..."
        mkdir -p "$BENCH_TMP/repos"
        git clone --depth 1 --branch "$branch" "https://github.com/$repo.git" "$clone_dir" 2>/dev/null || {
            git clone --depth 1 "https://github.com/$repo.git" "$clone_dir" 2>/dev/null || {
                warn "Clone failed for $repo"
                return 1
            }
        }
    fi

    local file_count
    file_count=$(find "$clone_dir" -type f | wc -l | tr -d ' ')
    ok "$repo: $file_count files"

    # Analyze
    log "Analyzing $repo (skip-embeddings)..."
    local t0
    t0=$(now_ms)
    local output
    output=$("$MYC" analyze "$clone_dir" --skip-embeddings --force 2>&1) || true
    local time_analyze
    time_analyze=$(elapsed_ms "$t0")

    local symbols rels repo_id
    symbols=$(echo "$output" | grep -i "symbols:" | head -1 | awk '{print $NF}' || echo "0")
    rels=$(echo "$output" | grep -i "relationships:" | head -1 | awk '{print $NF}' || echo "0")
    repo_id=$(echo "$output" | grep -i "repository id:" | head -1 | awk '{print $NF}' || echo "")

    if [[ -z "$repo_id" ]]; then
        warn "Analysis failed for $repo"
        echo "$output" | head -10
        return 1
    fi

    ok "$repo: analyzed in $(human_time $time_analyze) — symbols=$symbols rels=$rels"

    # Search
    local t1
    t1=$(now_ms)
    "$MYC" search "handle request" --repo "$repo_id" --limit 10 2>&1 >/dev/null || true
    local time_search
    time_search=$(elapsed_ms "$t1")

    REAL_RESULTS+=("$repo|$file_count|$symbols|$rels|$time_analyze|$time_search")

    # Cleanup
    "$MYC" delete "$repo_id" 2>/dev/null || true
}

# ---- Write markdown report --------------------------------------------------
write_report() {
    log "Writing results to $RESULTS_MD"

    local date_str
    date_str=$(date -u +%Y-%m-%d)

    cat > "$RESULTS_MD" << EOF
# Large-Repo Benchmark Results

**Date:** $date_str
**Machine:** $(uname -sm), $(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo "?") cores
**myc version:** $("$MYC" --version 2>/dev/null || echo "unknown")
EOF

    if [[ -n "${SYNTH_FILE_COUNT:-}" ]]; then
        cat >> "$RESULTS_MD" << EOF

## Synthetic Large Project (5,000+ files, 6 languages)

Languages: TypeScript, Python, Go, Rust, Java, C#

| Metric | Value |
|---|---|
| Files | $SYNTH_FILE_COUNT |
| Symbols | $SYNTH_SYMBOLS |
| Relationships | $SYNTH_RELS |
| Analysis (skip-embeddings) | $(human_time "$SYNTH_TIME_SKIP_EMBED") |
| Cached analysis | $(human_time "$SYNTH_TIME_CACHED") |
| BM25 search | $(human_time "$SYNTH_TIME_SEARCH") |
| Cypher query | $(human_time "$SYNTH_TIME_CYPHER") |
| Impact detection | $(human_time "$SYNTH_TIME_IMPACT") |
| Process tracing | $(human_time "$SYNTH_TIME_PROCESSES") |
EOF
    fi

    if [[ ${#REAL_RESULTS[@]} -gt 0 ]]; then
        cat >> "$RESULTS_MD" << 'EOF'

## Real-World Repositories

| Repository | Files | Symbols | Relationships | Analysis time | Search time |
|---|---|---|---|---|---|
EOF
        for entry in "${REAL_RESULTS[@]}"; do
            IFS='|' read -r repo files syms rels t_analyze t_search <<< "$entry"
            printf "| %s | %s | %s | %s | %s | %s |\n" \
                "$repo" "$files" "$syms" "$rels" \
                "$(human_time "$t_analyze")" "$(human_time "$t_search")" >> "$RESULTS_MD"
        done
    fi

    cat >> "$RESULTS_MD" << 'EOF'

## Methodology

- **Analysis (skip-embeddings):** Full parse + graph build without vector embedding generation.
  This is the typical mode used during interactive sessions. BM25 search, Cypher queries,
  impact detection, and process tracing all work without embeddings.
- **Cached analysis:** Re-running `myc analyze` when the cache is still valid (no file changes).
- **BM25 search:** Keyword search across all indexed symbols.
- **Cypher query:** Structured graph query (e.g., find all classes).
- **Impact detection:** Given a diff, trace which symbols and files are affected.
- **Process tracing:** Discover end-to-end execution flows through the codebase.

### Synthetic project structure

The synthetic project contains 5,000+ files spread across 6 languages with:
- Realistic directory structures (20 modules per language package)
- Cross-file imports and function calls
- Classes, interfaces, functions, and methods
- Varying file sizes and complexity levels
EOF

    ok "Report written to $RESULTS_MD"
}

# =============================================================================
# Main
# =============================================================================
mkdir -p "$BENCH_TMP"

printf "\n${BOLD}================================================================${NC}\n"
printf "${BOLD}  MYCELIUMS LARGE-REPO BENCHMARK${NC}\n"
printf "${BOLD}================================================================${NC}\n"
printf "  Date:   $(date -u +%Y-%m-%dT%H:%M:%SZ)\n"
printf "  Mode:   $MODE\n\n"

build_myc

# Initialize result variables
SYNTH_FILE_COUNT=""
SYNTH_SYMBOLS=""
SYNTH_RELS=""
SYNTH_TIME_SKIP_EMBED=""
SYNTH_TIME_CACHED=""
SYNTH_TIME_SEARCH=""
SYNTH_TIME_CYPHER=""
SYNTH_TIME_IMPACT=""
SYNTH_TIME_PROCESSES=""

case "$MODE" in
    synthetic)
        run_synthetic
        ;;
    real)
        for spec in "${REAL_REPOS[@]}"; do
            run_real_repo "$spec" || true
        done
        ;;
    both)
        run_synthetic
        for spec in "${REAL_REPOS[@]}"; do
            run_real_repo "$spec" || true
        done
        ;;
    single)
        # Find matching repo spec
        found=false
        for spec in "${REAL_REPOS[@]}"; do
            if [[ "$spec" == "$SINGLE_REPO"* ]]; then
                run_real_repo "$spec"
                found=true
                break
            fi
        done
        if ! $found; then
            # Treat as a custom repo
            run_real_repo "$SINGLE_REPO|main|unknown" || true
        fi
        ;;
esac

write_report

printf "\n${GREEN}${BOLD}Done!${NC}\n"
printf "  Results: ${RESULTS_MD}\n\n"
