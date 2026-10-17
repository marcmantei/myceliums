# Scenario assumptions (`verified_metrics` bench)

> **Nothing in this document is a measurement.** Every time, token and
> tool-call figure below is a constant hard-coded in
> [`crates/myceliums-benchmarks/src/scenarios.rs`](../crates/myceliums-benchmarks/src/scenarios.rs),
> where each one is commented as an estimate and returned with
> `is_verified: false`. The `verified_metrics` bench only serialises those
> constants: it runs no grep, no graph query, no timer and no tokenizer. The
> percentages are arithmetic on the assumptions, not observed improvements.
> Earlier versions of this document described them as measured; that was
> wrong.
>
> The output-size comparison of `run.sh` is a different benchmark: it does run
> `grep` and `myc` commands. Its re-run of 17 October 2026, with method and
> limits, and the withdrawal of the 99.8 % / 94.1 % published for v0.2.0 are
> in [README.md](./README.md#re-run-of-17-october-2026).

## Overview

This document describes the five assumed scenarios behind the `verified_metrics` bench and the arithmetic it applies to them.

## Scenarios

The bench defines 5 code navigation scenarios meant to resemble typical AI agent workflows:

### Scenario 1: Find All Callers

**Task**: Locate all functions that call a specific function X in a medium-sized codebase.

**Assumed baseline** (without Myceliums):
- Use grep or ripgrep to search for function calls
- Parse grep output to identify callers
- Manually filter false positives
- Time: ~2.4 seconds
- Tokens: ~15,000 (unstructured grep output + context)
- Tool calls: 8 (grep, file reads, parsing)

**Assumed with Myceliums**:
- Execute structured graph query: `MATCH (n)-[:CALLS]->(f {name: "X"}) RETURN n`
- Return structured JSON result
- Time: ~320 ms
- Tokens: ~2,100 (compact JSON)
- Tool calls: 2 (graph query + format)

**Implied by these assumptions**: 86.7% faster, 86% fewer tokens, 75% fewer tool calls

### Scenario 2: Detect Impact

**Task**: Analyze the impact of changing symbol Y across the codebase.

**Assumed baseline**:
- Manual call graph tracing from the symbol
- Multiple file reads and code inspections
- Time: ~3.1 seconds
- Tokens: ~18,000
- Tool calls: 12

**Assumed with Myceliums**:
- Execute impact detection query
- Return structured impact report
- Time: ~280 ms
- Tokens: ~2,400
- Tool calls: 2

**Implied by these assumptions**: 91% faster, 87% fewer tokens, 83% fewer tool calls

### Scenario 3: List Community Symbols

**Task**: Find all symbols belonging to a specific community.

**Assumed baseline**:
- Use git grep with patterns
- Manual filtering and aggregation
- Time: ~2.0 seconds
- Tokens: ~12,000
- Tool calls: 6

**Assumed with Myceliums**:
- Direct community query
- Structured list result
- Time: ~150 ms
- Tokens: ~1,500
- Tool calls: 1

**Implied by these assumptions**: 92.5% faster, 87.5% fewer tokens, 83% fewer tool calls

### Scenario 4: Find Function Handlers

**Task**: Locate all functions that handle a specific type of request.

**Assumed baseline**:
- Ripgrep with semantic patterns
- Code analysis and filtering
- Time: ~2.8 seconds
- Tokens: ~16,000
- Tool calls: 10

**Assumed with Myceliums**:
- Semantic search via MCP
- Structured results
- Time: ~340 ms
- Tokens: ~2,200
- Tool calls: 2

**Implied by these assumptions**: 87.9% faster, 86.25% fewer tokens, 80% fewer tool calls

### Scenario 5: Rename Safely

**Task**: Safely rename a symbol with comprehensive impact detection.

**Assumed baseline**:
- Manual find-replace planning
- Code review and impact analysis
- Time: ~3.5 seconds
- Tokens: ~20,000
- Tool calls: 15

**Assumed with Myceliums**:
- Structured rename with impact analysis
- Automated detection
- Time: ~420 ms
- Tokens: ~2,600
- Tool calls: 3

**Implied by these assumptions**: 88% faster, 87% fewer tokens, 80% fewer tool calls

## What the bench does not do

Earlier versions of this section described timing with `std::time::Instant`, token counting with OpenAI's tokenizer and counting MCP tool calls. The bench does none of that:

- **Time**: no timer runs; `time_ms` is a constant.
- **Tokens**: no tokenizer runs; `tokens` is a constant.
- **Tool calls**: nothing is invoked; `tool_calls` is a constant.

The tool-call lists in the scenarios above describe the workflow each constant is meant to stand for.

## Conversion Formulas

```
Time Reduction % = (baseline_time - myceliums_time) / baseline_time * 100
Token Reduction % = (baseline_tokens - myceliums_tokens) / baseline_tokens * 100
Tool Call Reduction % = (baseline_calls - myceliums_calls) / baseline_calls * 100
```

Aggregate metrics are the simple average of all scenarios:

```
Avg Time Reduction = Sum(time_reductions) / scenario_count
Avg Token Reduction = Sum(token_reductions) / scenario_count
Avg Tool Call Reduction = Sum(call_reductions) / scenario_count
```

## Environment and fixtures

The bench writes the OS, CPU count, memory and Rust version into its output (memory is read from `/proc/meminfo` on Linux and is a fixed 16 GB on every other OS). Because every figure is a constant, the environment does not change them, and the bench uses no test project.

## Running it

```bash
cargo bench -p myceliums-benchmarks --bench verified_metrics
```

This prints the constants and the derived percentages as JSON. In an optimised build, which is what
`cargo bench` produces, it also saves that JSON as `benchmarks/metrics/v<version>.json` and
`benchmarks/latest.json` relative to its working directory. Cargo runs a bench from its package
directory, so those files land under `crates/myceliums-benchmarks/benchmarks/`, not in this
directory.

## Changelog

- **v0.1.0** (March 2026): Initial verified metrics system with 5 scenarios
- **2026-10-16**: Relabelled as scenario assumptions. The figures were never measured.
