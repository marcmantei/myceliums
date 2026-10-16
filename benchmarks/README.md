# Benchmark data

This directory holds three different kinds of numbers. None of them is a
measured token saving of an AI agent: no benchmark here puts an agent in the
loop, and none checks that the approaches it compares return an equivalent
answer.

A benchmark that does score results against labelled expectations is the
retrieval evaluation (hand-labelled golden set, recall@k and MRR) — see
[`crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`](../crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md).

## Output-size comparison — `run.sh`, `results/`, `latest.json`

`run.sh` clones the repositories listed in `repos.toml` and, for each
question, compares two byte counts, each divided by four as a token estimate:

- **without Myceliums**: the output of a `find` over the source files, a
  `grep -r` for the question's pattern, and the contents of up to 20 matching
  files (the first 200 lines of any file over 8,000 bytes);
- **with Myceliums**: the output of one `myc query`, `myc search` (first 20
  lines) or `myc processes` command.

This is a script comparison of output size. No agent is in the loop, and
nothing checks that the `myc` output answers the question as well as the grep
baseline does. `"status": "PASS"` means only that the `myc` output was at
least 80% smaller, and `"queries_validated"` in the aggregate counts
questions, not checked answers. The 99.8% in `latest.json` is the same size
reduction, computed over the summed counts of all six files in
`results/v0.2.0/`.

Not all of those files are backed by a run:

| Repository | Questions | Timestamp in the file | Evidence that `run.sh` produced it |
|---|---:|---|---|
| fastapi | 7 | 2026-03-14T18:10:46Z | per-second timestamp, as `run.sh` writes it |
| zod | 6 | 2026-03-14T18:04:19Z | per-second timestamp, as `run.sh` writes it |
| django, express, gin, tokio | 21 | 2026-04-09T12:00:00Z, identical for all four | none |

The django, express, gin and tokio files were added in a single change in the
predecessor repository. Their identical timestamp is later than that change
was merged, and its test plan left the end-to-end run of `run.sh` unchecked.
`latest.json` and `results/v0.2.0/aggregate.json` carry the same 12:00:00Z
timestamp. There is no evidence that the script ran for these four
repositories, so treat their numbers, and the aggregate that includes them, as
unconfirmed until they are re-run.

To produce the files yourself:

```bash
cargo build --release -p myc
./benchmarks/run.sh --repo zod    # one repository
./benchmarks/run.sh               # every repository in repos.toml
```

## Synthetic large-project timings — `RESULTS.md`, `large-repo-benchmark.sh`

Indexing and query timings on a generated 5,010-file project. See
[RESULTS.md](./RESULTS.md) for the setup and how to reproduce it.

## Scenario assumptions — `METHODOLOGY.md`, `metrics/`

[METHODOLOGY.md](./METHODOLOGY.md) describes the five scenarios of the
`verified_metrics` Criterion bench. Despite its name, that bench measures
nothing: every baseline and "with Myceliums" value is a constant in
[`crates/myceliums-benchmarks/src/scenarios.rs`](../crates/myceliums-benchmarks/src/scenarios.rs),
commented there as an estimate and returned with `is_verified: false`. Its
output is a set of scenario assumptions, not a result. `metrics/` is empty.
