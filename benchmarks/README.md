# Benchmark data

This directory holds three different kinds of numbers. None of them is a
measured token saving of an AI agent: no benchmark here puts an agent in the
loop, and none checks that the approaches it compares return an equivalent
answer.

A benchmark that does score results against labelled expectations is the
retrieval evaluation (hand-labelled golden set, recall@k and MRR) — see
[`crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`](../crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md).

## Output-size comparison — `run.sh`, `results/`, `latest.json`, `rerun-2026-10-17/`

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
`results/v0.2.0/`. It is withdrawn, see
[below](#the-v020-figures-are-withdrawn).

### Re-run of 17 October 2026

All 34 questions were run again on 17 October 2026 (00:27–01:43 CEST, that
is 22:27–23:43 UTC on 16 October) with `myc 0.3.2`, built from myceliums
`c5dff11`, on these commits: django `d4300847`, express `a419aa73`, gin
`0f09c3a9`, tokio `cdd9dae3`, fastapi `bb7f3142` and zod `680d19f7`. The
checked-in `run.sh` cannot do that, because it skips Go, JavaScript and
Rust. The re-run used a copy patched to open that language gate, to keep its
output out of the repository, to capture what each side printed, and to
analyse without embeddings, which none of the 34 questions reads. Both sides
are counted as characters ÷ 4; no agent is involved. Setup, patch, raw data
and spot check: [`rerun-2026-10-17/`](./rerun-2026-10-17/).

- **Text.** Summed over the 34 questions, the `myc` output was **98.9 %
  smaller** than the find + grep + read baseline (24,677 against 2,266,087
  estimated tokens). Per question, the median was **~370× less text** across
  the 30 questions where `myc` printed a result (~440× if the 4 empty results
  are counted), and the reduction ranged from 83.9 % to 99.99 %. Against the
  grep output alone, without the file listing and the file reads, the `myc`
  output was smaller in 33 of 34 questions (median ~51×).
- **Calls.** 34 against 573: one call per question instead of 3 to 22. The
  script fixes this ratio (2 calls plus at most 20 file reads), so it is not
  a property of myc. Pooled that is 94.07 %; `run.sh` truncates and writes
  94.0 %.

**Limits.**

- No AI agent was involved, and characters ÷ 4 is an estimate, not a
  tokenizer count.
- The script does not check whether either side contains the answer. 4 of the
  34 `myc` queries printed only `(no results)` (express callers, classes and
  blast_radius, tokio call_graph; only express classes is correctly empty),
  and the script still counted each as at least 99.9 % saved and `PASS`.
- Answer quality was only spot-checked, by hand, on 16 questions: 13 picked
  with knowledge of most outputs, 3 drawn at random. The `myc` output fully
  answered 4, partly answered 6 and did not answer 6. The grep baseline fully
  answered 4, partly answered 10 and did not answer 2. The 3 random questions
  came out better for `myc` (1 fully, 2 partly, none unanswered), so 6 of 16
  is not a rate for all 34.
- The baseline is not a realistic agent either: 27 % of its text is the file
  listing, which is the same for every question of a repository; it reads
  the alphabetically first 20 matching files; and it stops at line 200 of
  any file over 8,000 bytes. That overstates the cost of the alternative for
  some questions and understates it for others.
- The checked-in `run.sh` had to be patched before it would run three of the
  six repositories at all.

### The v0.2.0 figures are withdrawn

The **99.8 % token reduction and 94.1 % tool-call reduction** published for
v0.2.0 (`latest.json`, `results/v0.2.0/aggregate.json`) **are withdrawn**.
Their arithmetic is correct: both are ratios of the sums over all 34
questions, truncated to one decimal, not averages. But 21 of the 34 values
behind them cannot be reproduced with `run.sh`. The files stay in the
repository unchanged, as the record of what was published.

| Repository | Questions | Timestamp in the file | Evidence that `run.sh` produced it | Baseline re-run on the commit current at that timestamp, against the published value |
|---|---:|---|---|---|
| fastapi | 7 | 2026-03-14T18:10:46Z | per-second timestamp and one-line `questions` array, as `run.sh` writes them | +0.7 % to +1.0 % per question; file count and all call counts match |
| zod | 6 | 2026-03-14T18:04:19Z | as for fastapi | +0.3 % to +0.9 % per question; file count and all call counts match |
| django, express, gin, tokio | 21 | 2026-04-09T12:00:00Z, identical for all four | none | −79 % to +381 % per question; file counts do not match (published / in the repository: django 6,492 / 2,894 `.py`, express 498 / 213 files, gin 312 / 99 `.go`, tokio 1,024 / 774 `.rs`) |

The django, express, gin and tokio files were added in a single change in the
predecessor repository. Their identical timestamp is later than that change
was merged, and its test plan left the end-to-end run of `run.sh` unchecked.
`latest.json` and `results/v0.2.0/aggregate.json` carry the same 12:00:00Z
timestamp. The four files also list one question per indented line, where
`run.sh` writes the `questions` array on one line, and the aggregate lists
the repositories in an order that `generate_aggregate` does not produce. The
checked-in script skips their languages altogether. fastapi and zod are real
`run.sh` output, but 11 of their 13 values are identical in all their
counts to v0.1.0, measured 3.5 hours earlier on the same day, so it is not
established that they were measured with myc 0.2.0.

To produce the files yourself:

```bash
cargo build --release -p myc
./benchmarks/run.sh --repo zod    # one repository
./benchmarks/run.sh               # every repository in repos.toml
```

As checked in, this runs only the TypeScript and Python repositories.
[`rerun-2026-10-17/`](./rerun-2026-10-17/) has the patch that opens the
other languages and the steps of the re-run.

## Synthetic large-project timings — `RESULTS.md`, `large-repo-benchmark.sh`

Indexing and query timings on a generated 5,010-file project. See
[RESULTS.md](./RESULTS.md) for the setup and how to reproduce it.

## Scenario assumptions — `METHODOLOGY.md`, `metrics/`

[METHODOLOGY.md](./METHODOLOGY.md) describes the five scenarios of the
`verified_metrics` Criterion bench. Despite its name, that bench measures
nothing: every baseline and "with Myceliums" value is a constant in
[`crates/myceliums-benchmarks/src/scenarios.rs`](../crates/myceliums-benchmarks/src/scenarios.rs),
commented there as an estimate and returned with `is_verified: false`. Its
output is a set of scenario assumptions, not a result. `metrics/` is empty;
the bench does not write into this directory (see
[METHODOLOGY.md](./METHODOLOGY.md#running-it)).
