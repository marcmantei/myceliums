#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Myceliums Real-World Benchmark Suite
# =============================================================================
#
# Validates token reduction by running Myceliums against real open-source repos.
#
# Results structure (versioned for comparison):
#   benchmarks/results/v0.2.0/typescript/zod.json
#   benchmarks/results/v0.2.0/python/fastapi.json
#   benchmarks/results/v0.2.0/aggregate.json
#   benchmarks/latest.json  (symlink-like copy of latest aggregate)
#
# Usage:
#   ./benchmarks/run.sh                                # All repos
#   ./benchmarks/run.sh --language typescript           # All TypeScript repos
#   ./benchmarks/run.sh --repo zod                     # Single repo
#   ./benchmarks/run.sh --language python --repo fastapi
#   ./benchmarks/run.sh --list                         # Show available repos
#   ./benchmarks/run.sh --aggregate                    # Only regenerate aggregate
#   ./benchmarks/run.sh --compare v0.1.0               # Compare current vs version
#   ./benchmarks/run.sh --compare v0.1.0 v0.2.0       # Compare two versions
#
# Requirements:
#   - myc binary (cargo build --release)
#   - git, grep, find, wc
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MYC="$PROJECT_ROOT/target/release/myc"
REPOS_DIR="/tmp/myceliums-benchmarks/repos"
REPOS_TOML="$SCRIPT_DIR/repos.toml"

# Get version from Cargo.toml
VERSION=$(grep 'version = ' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)

# Results are versioned: benchmarks/results/v0.2.0/typescript/zod.json
RESULTS_DIR="$SCRIPT_DIR/results/v${VERSION}"

export PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:$PATH"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

# =============================================================================
# Argument parsing
# =============================================================================
FILTER_LANGUAGE=""
FILTER_REPO=""
LIST_ONLY=false
AGGREGATE_ONLY=false
SKIP_CLONE=false
VERBOSE=false
COMPARE_V1=""
COMPARE_V2=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --language|-l)  FILTER_LANGUAGE="$2"; shift 2 ;;
        --repo|-r)      FILTER_REPO="$2"; shift 2 ;;
        --list)         LIST_ONLY=true; shift ;;
        --aggregate)    AGGREGATE_ONLY=true; shift ;;
        --skip-clone)   SKIP_CLONE=true; shift ;;
        --verbose|-v)   VERBOSE=true; shift ;;
        --compare|-c)
            COMPARE_V1="$2"; shift 2
            if [[ $# -gt 0 && ! "$1" =~ ^-- ]]; then
                COMPARE_V2="$1"; shift
            else
                COMPARE_V2="v${VERSION}"
            fi
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --language, -l LANG     Filter by language (typescript, python)"
            echo "  --repo, -r REPO         Filter by repo name (zod, fastapi, ...)"
            echo "  --list                  List available repos and exit"
            echo "  --aggregate             Only regenerate aggregate from existing results"
            echo "  --compare, -c V1 [V2]   Compare two versions (default V2=current)"
            echo "  --skip-clone            Skip cloning (use existing repos)"
            echo "  --verbose, -v           Show detailed output"
            echo "  --help, -h              Show this help"
            echo ""
            echo "Examples:"
            echo "  $0 --repo zod                     Run benchmark for zod only"
            echo "  $0 --compare v0.1.0               Compare v0.1.0 vs current (v${VERSION})"
            echo "  $0 --compare v0.1.0 v0.2.0        Compare two specific versions"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# =============================================================================
# Parse repos.toml (simple parser — no toml binary needed)
# =============================================================================
declare -a REPO_ENTRIES=()

parse_repos_toml() {
    local current_lang=""
    local current_repo=""
    local current_url=""
    local current_branch=""
    local current_stars=""
    local current_category=""
    local in_questions=false
    local questions_json="[]"

    while IFS= read -r line; do
        # Skip comments and empty lines
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "${line// }" ]] && continue

        # Section header: [language.repo]
        if [[ "$line" =~ ^\[([a-z]+)\.([a-z]+)\] ]]; then
            # Save previous entry if exists
            if [[ -n "$current_lang" && -n "$current_repo" ]]; then
                REPO_ENTRIES+=("$current_lang|$current_repo|$current_url|$current_branch|$current_stars|$current_category")
            fi
            current_lang="${BASH_REMATCH[1]}"
            current_repo="${BASH_REMATCH[2]}"
            current_url=""
            current_branch="main"
            current_stars=""
            current_category=""
            in_questions=false
            continue
        fi

        # Key = value parsing
        if [[ "$line" =~ ^url[[:space:]]*=[[:space:]]*\"(.+)\" ]]; then
            current_url="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ ^branch[[:space:]]*=[[:space:]]*\"(.+)\" ]]; then
            current_branch="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ ^stars[[:space:]]*=[[:space:]]*\"(.+)\" ]]; then
            current_stars="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ ^category[[:space:]]*=[[:space:]]*\"(.+)\" ]]; then
            current_category="${BASH_REMATCH[1]}"
        fi
    done < "$REPOS_TOML"

    # Save last entry
    if [[ -n "$current_lang" && -n "$current_repo" ]]; then
        REPO_ENTRIES+=("$current_lang|$current_repo|$current_url|$current_branch|$current_stars|$current_category")
    fi
}

# =============================================================================
# Parse questions from repos.toml for a specific repo
# =============================================================================
parse_questions() {
    local target_lang="$1"
    local target_repo="$2"
    local in_target=false
    local in_questions=false

    QUESTIONS=()

    while IFS= read -r line; do
        [[ "$line" =~ ^[[:space:]]*# ]] && continue

        if [[ "$line" =~ ^\[${target_lang}\.${target_repo}\] ]]; then
            in_target=true
            continue
        elif [[ "$line" =~ ^\[ ]] && ! [[ "$line" =~ ^\[${target_lang}\.${target_repo}\] ]]; then
            if $in_target; then
                break
            fi
            continue
        fi

        if $in_target; then
            # Extract question lines: { id = "...", label = "...", grep_pattern = "...", cypher = "..." }
            if [[ "$line" =~ id[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]]; then
                local qid="${BASH_REMATCH[1]}"
                local qlabel="" qgrep="" qcypher="" qsearch="" qprocesses=""

                [[ "$line" =~ label[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]] && qlabel="${BASH_REMATCH[1]}"
                [[ "$line" =~ grep_pattern[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]] && qgrep="${BASH_REMATCH[1]}"
                [[ "$line" =~ cypher[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]] && qcypher="${BASH_REMATCH[1]}"
                [[ "$line" =~ search[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]] && qsearch="${BASH_REMATCH[1]}"
                [[ "$line" =~ processes_entry[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]] && qprocesses="${BASH_REMATCH[1]}"

                QUESTIONS+=("$qid|$qlabel|$qgrep|$qcypher|$qsearch|$qprocesses")
            fi
        fi
    done < "$REPOS_TOML"
}

# =============================================================================
# Estimate tokens from byte count (~4 chars/token for code)
# =============================================================================
estimate_tokens() {
    echo $(( $1 / 4 ))
}

# =============================================================================
# Get file extension for language
# =============================================================================
get_ext() {
    case "$1" in
        typescript) echo "*.ts" ;;
        python)     echo "*.py" ;;
        rust)       echo "*.rs" ;;
        cpp)        echo "*.cpp" ;;
        go)         echo "*.go" ;;
        java)       echo "*.java" ;;
        *)          echo "*" ;;
    esac
}

# =============================================================================
# Run a single question benchmark
# =============================================================================
run_question() {
    local repo_path="$1"
    local repo_id="$2"
    local lang="$3"
    local qid="$4"
    local qlabel="$5"
    local qgrep="$6"
    local qcypher="$7"
    local qsearch="$8"
    local qprocesses="${9:-}"

    local ext
    ext=$(get_ext "$lang")

    # --- WITHOUT MYCELIUMS ---
    local total_bytes_without=0
    local tool_calls_without=0

    # Step 1: Glob
    local glob_output
    glob_output=$(find "$repo_path" -name "$ext" -not -path "*/node_modules/*" -not -path "*/.venv/*" -not -path "*/vendor/*" -not -path "*/__pycache__/*" 2>/dev/null || true)
    local glob_bytes=${#glob_output}
    total_bytes_without=$((total_bytes_without + glob_bytes))
    tool_calls_without=$((tool_calls_without + 1))

    # Step 2: Grep
    local grep_output
    grep_output=$(grep -r "$qgrep" "$repo_path" --include="$ext" 2>/dev/null || true)
    local grep_bytes=${#grep_output}
    local grep_files
    grep_files=$(echo "$grep_output" | grep -c "." 2>/dev/null || echo "0")
    total_bytes_without=$((total_bytes_without + grep_bytes))
    tool_calls_without=$((tool_calls_without + 1))

    # Step 3: Read matching files (max 20)
    local read_bytes=0
    local read_count=0
    local files_list
    files_list=$(echo "$grep_output" | cut -d: -f1 | sort -u 2>/dev/null || true)
    while IFS= read -r file; do
        [[ -z "$file" ]] && continue
        [[ ! -f "$file" ]] && continue
        [[ $read_count -ge 20 ]] && break
        local fsize
        fsize=$(wc -c < "$file" 2>/dev/null | tr -d ' ')
        if [[ "$fsize" -gt 8000 ]]; then
            local partial
            partial=$(head -200 "$file" 2>/dev/null | wc -c | tr -d ' ')
            read_bytes=$((read_bytes + partial))
        else
            read_bytes=$((read_bytes + fsize))
        fi
        read_count=$((read_count + 1))
    done <<< "$files_list"
    total_bytes_without=$((total_bytes_without + read_bytes))
    tool_calls_without=$((tool_calls_without + read_count))

    local tokens_without
    tokens_without=$(estimate_tokens $total_bytes_without)

    # --- WITH MYCELIUMS ---
    local myc_output=""
    local tool_calls_with=1

    if [[ -n "$qcypher" ]]; then
        myc_output=$("$MYC" query "$qcypher" --repo "$repo_id" 2>&1 || true)
    elif [[ -n "$qsearch" ]]; then
        myc_output=$("$MYC" search "$qsearch" --repo "$repo_id" 2>&1 | head -20 || true)
    elif [[ -n "$qprocesses" ]]; then
        myc_output=$("$MYC" processes "$repo_id" --entry "$qprocesses" 2>&1 || true)
    fi

    local myc_bytes=${#myc_output}
    local tokens_with
    tokens_with=$(estimate_tokens $myc_bytes)
    [[ $tokens_with -eq 0 ]] && tokens_with=1

    # Reduction
    local token_reduction=0
    if [[ $tokens_without -gt 0 ]]; then
        token_reduction=$(( (tokens_without - tokens_with) * 1000 / tokens_without ))
    fi
    local token_reduction_pct="$(( token_reduction / 10 )).$(( token_reduction % 10 ))"

    local call_reduction=0
    if [[ $tool_calls_without -gt 0 ]]; then
        call_reduction=$(( (tool_calls_without - tool_calls_with) * 1000 / tool_calls_without ))
    fi
    local call_reduction_pct="$(( call_reduction / 10 )).$(( call_reduction % 10 ))"

    # Status indicator
    local status="PASS"
    local color="$GREEN"
    if [[ $(( token_reduction / 10 )) -lt 80 ]]; then
        status="WARN"
        color="$YELLOW"
    fi
    if [[ $(( token_reduction / 10 )) -lt 50 ]]; then
        status="FAIL"
        color="$RED"
    fi

    printf "  %-40s %s%-4s${NC}  tokens: %6d → %4d (%s%%)  calls: %2d → %d\n" \
        "$qlabel" "$color" "$status" "$tokens_without" "$tokens_with" "$token_reduction_pct" "$tool_calls_without" "$tool_calls_with"

    # Return JSON line
    echo "{\"id\":\"$qid\",\"label\":\"$qlabel\",\"tokens_without\":$tokens_without,\"tokens_with\":$tokens_with,\"token_reduction_pct\":$token_reduction_pct,\"tool_calls_without\":$tool_calls_without,\"tool_calls_with\":$tool_calls_with,\"call_reduction_pct\":$call_reduction_pct,\"status\":\"$status\"}"
}

# =============================================================================
# Run benchmark for a single repo
# =============================================================================
benchmark_repo() {
    local lang="$1"
    local repo="$2"
    local url="$3"
    local branch="$4"
    local stars="$5"
    local category="$6"

    local repo_path="$REPOS_DIR/$lang/$repo"
    local result_dir="$RESULTS_DIR/$lang"
    local result_file="$result_dir/$repo.json"

    mkdir -p "$result_dir"

    echo ""
    printf "${BOLD}${BLUE}━━━ %s/%s${NC} (%s, %s stars)\n" "$lang" "$repo" "$category" "$stars"

    # Clone if needed
    if [[ ! -d "$repo_path" ]] && ! $SKIP_CLONE; then
        printf "  Cloning %s ...\n" "$url"
        mkdir -p "$REPOS_DIR/$lang"
        git clone --depth 1 --branch "$branch" "$url" "$repo_path" 2>/dev/null || {
            # Retry without branch (some repos use different default)
            git clone --depth 1 "$url" "$repo_path" 2>/dev/null || {
                printf "  ${RED}SKIP: Clone failed${NC}\n"
                return 1
            }
        }
    elif [[ ! -d "$repo_path" ]]; then
        printf "  ${YELLOW}SKIP: Not cloned (use without --skip-clone)${NC}\n"
        return 1
    fi

    # Count files
    local ext
    ext=$(get_ext "$lang")
    local file_count
    file_count=$(find "$repo_path" -name "$ext" -not -path "*/node_modules/*" -not -path "*/.venv/*" -not -path "*/vendor/*" 2>/dev/null | wc -l | tr -d ' ')
    printf "  Files: %s\n" "$file_count"

    # Analyze with Myceliums
    printf "  Analyzing with Myceliums...\n"
    local analyze_output
    analyze_output=$("$MYC" analyze "$repo_path" 2>&1 || true)

    # Extract repo ID
    local repo_id
    repo_id=$(echo "$analyze_output" | grep "Repository ID:" | awk '{print $NF}')
    if [[ -z "$repo_id" ]]; then
        printf "  ${RED}SKIP: Analysis failed${NC}\n"
        if $VERBOSE; then echo "$analyze_output"; fi
        return 1
    fi

    # Extract stats
    local symbols relationships
    symbols=$(echo "$analyze_output" | grep "Symbols:" | awk '{print $NF}')
    relationships=$(echo "$analyze_output" | grep "Relationships:" | awk '{print $NF}')
    printf "  Symbols: %s | Relationships: %s | ID: %s\n" "$symbols" "$relationships" "$repo_id"

    # Parse and run questions
    parse_questions "$lang" "$repo"
    echo ""

    local questions_json="["
    local first=true
    local total_tokens_without=0
    local total_tokens_with=0
    local total_calls_without=0
    local total_calls_with=0
    local question_count=0

    for q in "${QUESTIONS[@]}"; do
        IFS='|' read -r qid qlabel qgrep qcypher qsearch qprocesses <<< "$q"

        local result
        result=$(run_question "$repo_path" "$repo_id" "$lang" "$qid" "$qlabel" "$qgrep" "$qcypher" "$qsearch" "$qprocesses")

        # Last line is JSON
        local json_line
        json_line=$(echo "$result" | tail -1)

        if ! $first; then questions_json+=","; fi
        questions_json+="$json_line"
        first=false

        # Accumulate totals
        local tw tw2 cw cw2
        tw=$(echo "$json_line" | grep -o '"tokens_without":[0-9]*' | cut -d: -f2)
        tw2=$(echo "$json_line" | grep -o '"tokens_with":[0-9]*' | cut -d: -f2)
        cw=$(echo "$json_line" | grep -o '"tool_calls_without":[0-9]*' | cut -d: -f2)
        cw2=$(echo "$json_line" | grep -o '"tool_calls_with":[0-9]*' | cut -d: -f2)
        total_tokens_without=$((total_tokens_without + tw))
        total_tokens_with=$((total_tokens_with + tw2))
        total_calls_without=$((total_calls_without + cw))
        total_calls_with=$((total_calls_with + cw2))
        question_count=$((question_count + 1))
    done
    questions_json+="]"

    # Calculate aggregate
    local agg_token_reduction=0
    if [[ $total_tokens_without -gt 0 ]]; then
        agg_token_reduction=$(( (total_tokens_without - total_tokens_with) * 1000 / total_tokens_without ))
    fi
    local agg_token_pct="$(( agg_token_reduction / 10 )).$(( agg_token_reduction % 10 ))"

    local agg_call_reduction=0
    if [[ $total_calls_without -gt 0 ]]; then
        agg_call_reduction=$(( (total_calls_without - total_calls_with) * 1000 / total_calls_without ))
    fi
    local agg_call_pct="$(( agg_call_reduction / 10 )).$(( agg_call_reduction % 10 ))"

    echo ""
    printf "  ${BOLD}TOTAL: ${GREEN}%s%% token reduction${NC}, ${GREEN}%s%% fewer tool calls${NC} (%d questions)\n" \
        "$agg_token_pct" "$agg_call_pct" "$question_count"

    # Write result JSON
    cat > "$result_file" << ENDJSON
{
  "repo": "$repo",
  "language": "$lang",
  "url": "$url",
  "category": "$category",
  "stars": "$stars",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "version": "$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)",
  "stats": {
    "files": $file_count,
    "symbols": ${symbols:-0},
    "relationships": ${relationships:-0}
  },
  "questions": $questions_json,
  "aggregate": {
    "total_tokens_without": $total_tokens_without,
    "total_tokens_with": $total_tokens_with,
    "token_reduction_pct": $agg_token_pct,
    "total_calls_without": $total_calls_without,
    "total_calls_with": $total_calls_with,
    "call_reduction_pct": $agg_call_pct,
    "questions_tested": $question_count
  }
}
ENDJSON

    printf "  Result saved: %s\n" "$result_file"

    # Cleanup analysis data to save disk
    "$MYC" delete "$repo_id" 2>/dev/null || true
}

# =============================================================================
# Generate aggregate from all result files
# =============================================================================
generate_aggregate() {
    local all_results="$RESULTS_DIR"
    local aggregate_file="$RESULTS_DIR/aggregate.json"
    local latest_file="$SCRIPT_DIR/latest.json"

    echo ""
    printf "${BOLD}${BLUE}━━━ Generating aggregate results${NC}\n"

    local total_tw=0 total_tm=0 total_cw=0 total_cm=0
    local repo_count=0 question_count=0
    local repos_json="["
    local first=true

    for result in "$all_results"/*/*.json; do
        [[ ! -f "$result" ]] && continue

        local rname rurl rlang rtw rtm rcw rcm rqc
        rname=$(grep -o '"repo": *"[^"]*"' "$result" | head -1 | cut -d'"' -f4)
        rurl=$(grep -o '"url": *"[^"]*"' "$result" | head -1 | cut -d'"' -f4)
        rlang=$(grep -o '"language": *"[^"]*"' "$result" | head -1 | cut -d'"' -f4)
        # These keys appear in both questions array and aggregate — take last (aggregate)
        rtw=$(grep -o '"total_tokens_without": *[0-9]*' "$result" | tail -1 | sed 's/.*: *//')
        rtm=$(grep -o '"total_tokens_with": *[0-9]*' "$result" | tail -1 | sed 's/.*: *//')
        rcw=$(grep -o '"total_calls_without": *[0-9]*' "$result" | tail -1 | sed 's/.*: *//')
        rcm=$(grep -o '"total_calls_with": *[0-9]*' "$result" | tail -1 | sed 's/.*: *//')
        rqc=$(grep -o '"questions_tested": *[0-9]*' "$result" | tail -1 | sed 's/.*: *//')

        total_tw=$((total_tw + rtw))
        total_tm=$((total_tm + rtm))
        total_cw=$((total_cw + rcw))
        total_cm=$((total_cm + rcm))
        question_count=$((question_count + rqc))
        repo_count=$((repo_count + 1))

        # Read file stats (from "stats" block only — take first match)
        local rfiles rsymbols rrels
        rfiles=$(grep -o '"files": *[0-9]*' "$result" | head -1 | sed 's/.*: *//')
        rsymbols=$(grep -o '"symbols": *[0-9]*' "$result" | head -1 | sed 's/.*: *//')
        rrels=$(grep -o '"relationships": *[0-9]*' "$result" | head -1 | sed 's/.*: *//')

        if ! $first; then repos_json+=","; fi
        repos_json+="{\"name\":\"$rname\",\"url\":\"$rurl\",\"language\":\"$rlang\",\"files\":$rfiles,\"symbols\":$rsymbols,\"relationships\":$rrels}"
        first=false
    done
    repos_json+="]"

    # Calculate aggregates
    local agg_token_pct="0.0"
    if [[ $total_tw -gt 0 ]]; then
        local tr=$(( (total_tw - total_tm) * 1000 / total_tw ))
        agg_token_pct="$(( tr / 10 )).$(( tr % 10 ))"
    fi

    local agg_call_pct="0.0"
    if [[ $total_cw -gt 0 ]]; then
        local cr=$(( (total_cw - total_cm) * 1000 / total_cw ))
        agg_call_pct="$(( cr / 10 )).$(( cr % 10 ))"
    fi

    local version
    version=$(grep 'version = ' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)

    cat > "$aggregate_file" << ENDJSON
{
  "version": "$version",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "method": "real-world validation against open-source repositories",
  "repositories": $repos_json,
  "aggregate": {
    "total_tokens_without": $total_tw,
    "total_tokens_with": $total_tm,
    "avg_token_reduction_percent": $agg_token_pct,
    "total_calls_without": $total_cw,
    "total_calls_with": $total_cm,
    "avg_tool_call_reduction_percent": $agg_call_pct,
    "repos_tested": $repo_count,
    "queries_validated": $question_count
  }
}
ENDJSON

    printf "  Repos tested: %d\n" "$repo_count"
    printf "  Questions validated: %d\n" "$question_count"
    printf "  ${GREEN}Token reduction: %s%%${NC}\n" "$agg_token_pct"
    printf "  ${GREEN}Tool call reduction: %s%%${NC}\n" "$agg_call_pct"
    printf "  Saved: %s\n" "$aggregate_file"

    # Also copy to latest.json (used by website)
    cp "$aggregate_file" "$latest_file"
    printf "  Copied to: %s\n" "$latest_file"
}

# =============================================================================
# Compare two versions
# =============================================================================
compare_versions() {
    local v1="$1"
    local v2="$2"

    local dir1="$SCRIPT_DIR/results/$v1"
    local dir2="$SCRIPT_DIR/results/$v2"

    if [[ ! -d "$dir1" ]]; then
        echo "Error: No results found for $v1 (expected $dir1)"
        echo "Available versions:"
        ls -d "$SCRIPT_DIR/results"/v* 2>/dev/null | sed 's/.*\//  /'
        exit 1
    fi
    if [[ ! -d "$dir2" ]]; then
        echo "Error: No results found for $v2 (expected $dir2)"
        echo "Available versions:"
        ls -d "$SCRIPT_DIR/results"/v* 2>/dev/null | sed 's/.*\//  /'
        exit 1
    fi

    printf "\n${BOLD}══════════════════════════════════════════════${NC}\n"
    printf "${BOLD}  BENCHMARK COMPARISON: %s vs %s${NC}\n" "$v1" "$v2"
    printf "${BOLD}══════════════════════════════════════════════${NC}\n\n"

    # Compare per-repo results
    for result2 in "$dir2"/*/*.json; do
        [[ ! -f "$result2" ]] && continue
        [[ "$(basename "$result2")" == "aggregate.json" ]] && continue

        local rel_path="${result2#$dir2/}"
        local result1="$dir1/$rel_path"
        local rname
        rname=$(grep -o '"repo": *"[^"]*"' "$result2" | head -1 | cut -d'"' -f4)
        local rlang
        rlang=$(grep -o '"language": *"[^"]*"' "$result2" | head -1 | cut -d'"' -f4)

        printf "${BOLD}${BLUE}━━━ %s/%s${NC}\n" "$rlang" "$rname"

        if [[ ! -f "$result1" ]]; then
            printf "  ${YELLOW}(no $v1 data — new repo)${NC}\n\n"
            continue
        fi

        # Read v1 aggregate
        local v1_tw v1_tm v1_cw v1_cm
        v1_tw=$(grep -o '"total_tokens_without": *[0-9]*' "$result1" | tail -1 | sed 's/.*: *//')
        v1_tm=$(grep -o '"total_tokens_with": *[0-9]*' "$result1" | tail -1 | sed 's/.*: *//')
        v1_cw=$(grep -o '"total_calls_without": *[0-9]*' "$result1" | tail -1 | sed 's/.*: *//')
        v1_cm=$(grep -o '"total_calls_with": *[0-9]*' "$result1" | tail -1 | sed 's/.*: *//')
        local v1_tpct
        v1_tpct=$(grep -o '"token_reduction_pct": *[0-9.]*' "$result1" | tail -1 | sed 's/.*: *//')
        local v1_cpct
        v1_cpct=$(grep -o '"call_reduction_pct": *[0-9.]*' "$result1" | tail -1 | sed 's/.*: *//')

        # Read v2 aggregate
        local v2_tw v2_tm v2_cw v2_cm
        v2_tw=$(grep -o '"total_tokens_without": *[0-9]*' "$result2" | tail -1 | sed 's/.*: *//')
        v2_tm=$(grep -o '"total_tokens_with": *[0-9]*' "$result2" | tail -1 | sed 's/.*: *//')
        v2_cw=$(grep -o '"total_calls_without": *[0-9]*' "$result2" | tail -1 | sed 's/.*: *//')
        v2_cm=$(grep -o '"total_calls_with": *[0-9]*' "$result2" | tail -1 | sed 's/.*: *//')
        local v2_tpct
        v2_tpct=$(grep -o '"token_reduction_pct": *[0-9.]*' "$result2" | tail -1 | sed 's/.*: *//')
        local v2_cpct
        v2_cpct=$(grep -o '"call_reduction_pct": *[0-9.]*' "$result2" | tail -1 | sed 's/.*: *//')

        printf "  %-25s %10s %10s %10s\n" "" "$v1" "$v2" "Change"
        printf "  %-25s %10s %10s" "Tokens (with myceliums):" "$v1_tm" "$v2_tm"
        if [[ -n "$v1_tm" && -n "$v2_tm" && "$v1_tm" -gt 0 ]]; then
            local delta=$(( v2_tm - v1_tm ))
            local pct_change
            if [[ $v1_tm -gt 0 ]]; then
                pct_change=$(( delta * 100 / v1_tm ))
            else
                pct_change=0
            fi
            if [[ $delta -lt 0 ]]; then
                printf " ${GREEN}%+d (%+d%%)${NC}" "$delta" "$pct_change"
            elif [[ $delta -gt 0 ]]; then
                printf " ${RED}%+d (%+d%%)${NC}" "$delta" "$pct_change"
            else
                printf " ${YELLOW}unchanged${NC}"
            fi
        fi
        echo ""

        printf "  %-25s %10s %10s" "Token reduction:" "${v1_tpct}%" "${v2_tpct}%"
        echo ""

        printf "  %-25s %10s %10s" "Tool calls (with):" "$v1_cm" "$v2_cm"
        if [[ -n "$v1_cm" && -n "$v2_cm" ]]; then
            local cdelta=$(( v2_cm - v1_cm ))
            if [[ $cdelta -lt 0 ]]; then
                printf " ${GREEN}%+d${NC}" "$cdelta"
            elif [[ $cdelta -gt 0 ]]; then
                printf " ${RED}%+d${NC}" "$cdelta"
            fi
        fi
        echo ""

        printf "  %-25s %10s %10s" "Call reduction:" "${v1_cpct}%" "${v2_cpct}%"
        echo ""
        echo ""

        # Per-question comparison
        if $VERBOSE; then
            printf "  ${BOLD}Per-question breakdown:${NC}\n"
            # Extract question IDs from v2
            local qids
            qids=$(grep -o '"id":"[^"]*"' "$result2" | cut -d'"' -f4)
            while IFS= read -r qid; do
                [[ -z "$qid" ]] && continue
                # Find matching question in both files
                local q1_tw q1_tm q2_tw q2_tm q_label
                q_label=$(grep -o "\"id\":\"${qid}\"[^}]*\"label\":\"[^\"]*\"" "$result2" | grep -o '"label":"[^"]*"' | cut -d'"' -f4)
                q1_tm=$(grep -o "\"id\":\"${qid}\"[^}]*\"tokens_with\":[0-9]*" "$result1" 2>/dev/null | grep -o '"tokens_with":[0-9]*' | cut -d: -f2)
                q2_tm=$(grep -o "\"id\":\"${qid}\"[^}]*\"tokens_with\":[0-9]*" "$result2" | grep -o '"tokens_with":[0-9]*' | cut -d: -f2)

                if [[ -n "$q1_tm" && -n "$q2_tm" ]]; then
                    local qdelta=$(( q2_tm - q1_tm ))
                    if [[ $qdelta -lt 0 ]]; then
                        printf "    %-38s %5s → %5s ${GREEN}%+d${NC}\n" "$q_label" "$q1_tm" "$q2_tm" "$qdelta"
                    elif [[ $qdelta -gt 0 ]]; then
                        printf "    %-38s %5s → %5s ${RED}%+d${NC}\n" "$q_label" "$q1_tm" "$q2_tm" "$qdelta"
                    else
                        printf "    %-38s %5s → %5s  0\n" "$q_label" "$q1_tm" "$q2_tm"
                    fi
                fi
            done <<< "$qids"
            echo ""
        fi
    done

    # Aggregate comparison
    local agg1="$dir1/aggregate.json"
    local agg2="$dir2/aggregate.json"

    # Fall back to per-version results if no aggregate.json exists
    # (v0.1.0 used flat structure)
    if [[ ! -f "$agg1" ]]; then
        # Try to find aggregate data from repo results
        local a1_tw=0 a1_tm=0 a1_cw=0 a1_cm=0 a1_qc=0
        for r in "$dir1"/*/*.json "$dir1"/*.json; do
            [[ ! -f "$r" ]] && continue
            [[ "$(basename "$r")" == "aggregate.json" ]] && continue
            local rtw rtm rcw rcm rqc
            rtw=$(grep -o '"total_tokens_without": *[0-9]*' "$r" | tail -1 | sed 's/.*: *//' || echo "0")
            rtm=$(grep -o '"total_tokens_with": *[0-9]*' "$r" | tail -1 | sed 's/.*: *//' || echo "0")
            rcw=$(grep -o '"total_calls_without": *[0-9]*' "$r" | tail -1 | sed 's/.*: *//' || echo "0")
            rcm=$(grep -o '"total_calls_with": *[0-9]*' "$r" | tail -1 | sed 's/.*: *//' || echo "0")
            rqc=$(grep -o '"questions_tested": *[0-9]*' "$r" | tail -1 | sed 's/.*: *//' || echo "0")
            [[ -n "$rtw" ]] && a1_tw=$((a1_tw + rtw))
            [[ -n "$rtm" ]] && a1_tm=$((a1_tm + rtm))
            [[ -n "$rcw" ]] && a1_cw=$((a1_cw + rcw))
            [[ -n "$rcm" ]] && a1_cm=$((a1_cm + rcm))
            [[ -n "$rqc" ]] && a1_qc=$((a1_qc + rqc))
        done
        printf "${BOLD}━━━ AGGREGATE COMPARISON${NC}\n"
        printf "  %-25s %10s %10s\n" "" "$v1" "$v2"

        local a2_tw a2_tm a2_cw a2_cm a2_qc
        a2_tw=$(grep -o '"total_tokens_without": *[0-9]*' "$agg2" 2>/dev/null | tail -1 | sed 's/.*: *//' || echo "0")
        a2_tm=$(grep -o '"total_tokens_with": *[0-9]*' "$agg2" 2>/dev/null | tail -1 | sed 's/.*: *//' || echo "0")
        a2_cw=$(grep -o '"total_calls_without": *[0-9]*' "$agg2" 2>/dev/null | tail -1 | sed 's/.*: *//' || echo "0")
        a2_cm=$(grep -o '"total_calls_with": *[0-9]*' "$agg2" 2>/dev/null | tail -1 | sed 's/.*: *//' || echo "0")

        printf "  %-25s %10s %10s\n" "Total tokens (with):" "$a1_tm" "$a2_tm"
        printf "  %-25s %10s %10s\n" "Total calls (with):" "$a1_cm" "$a2_cm"
    else
        printf "${BOLD}━━━ AGGREGATE COMPARISON${NC}\n"
        printf "  (see per-repo details above)\n"
    fi

    echo ""
}

# =============================================================================
# Main
# =============================================================================

# Check binary
if [[ ! -x "$MYC" ]]; then
    echo "Error: myc binary not found at $MYC"
    echo "Run: cargo build --release"
    exit 1
fi

parse_repos_toml

# Compare mode
if [[ -n "$COMPARE_V1" ]]; then
    compare_versions "$COMPARE_V1" "$COMPARE_V2"
    exit 0
fi

# List mode
if $LIST_ONLY; then
    printf "\n${BOLD}Available benchmark repos:${NC}\n\n"
    for entry in "${REPO_ENTRIES[@]}"; do
        IFS='|' read -r lang repo url branch stars category <<< "$entry"
        local_status="not cloned"
        [[ -d "$REPOS_DIR/$lang/$repo" ]] && local_status="cloned"
        result_status="no results"
        [[ -f "$RESULTS_DIR/$lang/$repo.json" ]] && result_status="has results"
        printf "  %-12s %-12s %-14s %-12s %s\n" "$lang" "$repo" "($stars, $category)" "[$local_status]" "[$result_status]"
    done

    # Show available versions
    printf "\n${BOLD}Available result versions:${NC}\n\n"
    for vdir in "$SCRIPT_DIR/results"/v*; do
        [[ ! -d "$vdir" ]] && continue
        local vname
        vname=$(basename "$vdir")
        local repo_count
        repo_count=$(find "$vdir" -name "*.json" -not -name "aggregate.json" 2>/dev/null | wc -l | tr -d ' ')
        printf "  %-12s %d repo(s)\n" "$vname" "$repo_count"
    done
    echo ""
    exit 0
fi

# Aggregate only mode
if $AGGREGATE_ONLY; then
    generate_aggregate
    exit 0
fi

# Header
printf "\n${BOLD}══════════════════════════════════════════════${NC}\n"
printf "${BOLD}  MYCELIUMS REAL-WORLD BENCHMARK SUITE${NC}\n"
printf "${BOLD}══════════════════════════════════════════════${NC}\n"
printf "  Date:     $(date -u +%Y-%m-%dT%H:%M:%SZ)\n"
printf "  Binary:   $MYC\n"
printf "  Filter:   lang=%s repo=%s\n" "${FILTER_LANGUAGE:-all}" "${FILTER_REPO:-all}"

mkdir -p "$REPOS_DIR" "$RESULTS_DIR"

# Run benchmarks
ran_count=0
for entry in "${REPO_ENTRIES[@]}"; do
    IFS='|' read -r lang repo url branch stars category <<< "$entry"

    # Apply filters
    if [[ -n "$FILTER_LANGUAGE" && "$lang" != "$FILTER_LANGUAGE" ]]; then
        continue
    fi
    if [[ -n "$FILTER_REPO" && "$repo" != "$FILTER_REPO" ]]; then
        continue
    fi

    # Check parser support
    case "$lang" in
        typescript|python) ;;
        *)
            printf "\n  ${YELLOW}SKIP %s/%s — %s parser not yet implemented${NC}\n" "$lang" "$repo" "$lang"
            continue
            ;;
    esac

    benchmark_repo "$lang" "$repo" "$url" "$branch" "$stars" "$category" || true
    ran_count=$((ran_count + 1))
done

if [[ $ran_count -eq 0 ]]; then
    echo ""
    echo "No repos matched the filter. Use --list to see available repos."
    exit 1
fi

# Generate aggregate
generate_aggregate

printf "\n${BOLD}${GREEN}Done!${NC} Ran %d repo(s).\n\n" "$ran_count"
