# Re-run of the output-size comparison, 17 October 2026

This directory is the evidence behind the figures in
[`../README.md`](../README.md#re-run-of-17-october-2026): a re-run of the 34
questions that `repos.toml` defines for the six repositories of the published
v0.2.0 results, a hand spot check of 16 of them, and an independent
re-computation of every number from the captured outputs. Read the limits in
`../README.md` before quoting any figure from here.

## Setup

- **Myceliums**: commit `c5dff11cbfbd3e8ec5b68d31f09b7f8195f3430a`,
  `cargo build --release -p myc`, reporting `myc 0.3.2`.
- **Script**: `benchmarks/run.sh` with [`run-sh.patch`](./run-sh.patch)
  applied (see below), run with `--skip-clone` against shallow clones of these
  commits:

  | Repository | Commit | Commit date | Files (`run.sh` pattern) | Symbols / relationships | Questions |
  |---|---|---|---:|---:|---:|
  | django | `d430084791a61b31cecec132bce3dca19194f7f2` | 2026-10-15 | 2,933 | 46,343 / 395,034 | 5 |
  | express | `a419aa7350389a275ab45fde2e849ed1f02449ef` | 2026-10-09 | 322 (pattern `*`) | 2,011 / 2,119 | 6 |
  | gin | `0f09c3a9b4626d9fe9979cf4d0521d6ae32ab646` | 2026-10-10 | 99 | 2,174 / 6,499 | 5 |
  | tokio | `cdd9dae3aa5056e23dc4d5bc4f852ab8fbf27aff` | 2026-10-16 | 811 | 11,202 / 35,247 | 5 |
  | fastapi | `bb7f314287bf36e04539c572a2256d94ea45f0d6` | 2026-10-16 | 1,168 | 22,668 / 82,241 | 7 |
  | zod | `680d19f7d1e042e55934209caeb79bc843b94f44` | 2026-10-13 | 477 | 12,465 / 19,519 | 6 |

- **When**: 22:27–23:43 UTC on 16 October 2026 (00:27–01:43 CEST on
  17 October), one repository after another, with an isolated `HOME`
  (`myc` otherwise writes its index under `~/.myceliums`). The timestamps in
  `run-sh-output/` are the ones `run.sh` wrote.
- **Repeats**: gin, express and tokio were run a second time with a separate
  `HOME`; every number was identical. A second build from `c5dff11` reproduced
  the 12 `myc` outputs of tokio and fastapi (content and size; only the column
  order of some tables differs), and the "without" side of fastapi, tokio and
  django (17 questions), re-run from the original lines of `run.sh`, matched
  byte for byte.

## The patch

As checked in, `run.sh` skips Go, JavaScript and Rust ("parser not yet
implemented"), so it measures only 13 of the 34 questions. `run-sh.patch` is
the exact difference between `benchmarks/run.sh` at `c5dff11` and the script
the main run used:

1. **Paths**: binary, `benchmarks/` directory and result directory come from
   `NACH_MYC`, `NACH_BENCH` and `NACH_RESULTS`; clones live under
   `/tmp/myc-nachrechnung-repos/<name>`; `latest.json` is written into the
   result directory instead of over `benchmarks/latest.json`.
2. **Language gate**: `javascript|go|rust` added next to `typescript|python`.
   Without this, three of the six repositories do not run at all.
3. **Capture**: when `NACH_RAW` is set, the `myc` output, the grep output, the
   list of files read and the four size counts are written per question. This
   changes no measured value.
4. **`myc analyze --skip-embeddings`**: none of the 34 questions reads
   vectors (`query` is Cypher, `search` without `--hybrid` is BM25, plus
   `processes`). zod run with embeddings gave identical numbers for all six
   questions (1,794 s instead of 290 s).

`get_ext` was left alone, so express runs with the pattern `*`, as `run.sh`
does. With `*.js` instead, express comes to 99.77 % / 90.77 % and the total to
98.90 % / 94.05 %.

## Files

| File | What it is |
|---|---|
| `neulauf.json` | Per question: repository commit, command, grep pattern, both sides' tokens and calls, the composition of the "without" side, the `myc` output status and its first characters, the published v0.2.0 value for the same question, the "grep only" sensitivity and the spot-check verdict. Totals per repository and overall, including the totals over the 30 questions with a non-empty result. |
| `gleichwertigkeit.json` | The spot check of 13 hand-picked questions: criteria, the reference built from the source, and the reasoning for each verdict. |
| `pruefung.json` | Independent re-computation from the primary data only (94 check points, 3 of them deviations that concern the published files, not the re-run), plus the 3 randomly drawn spot-check questions (`gleichwertigkeit_zusatz`, `random.seed(20261017)`) and the 16-question result. |
| `run-sh.patch` | The patch described above. |
| `run-sh-output/` | The result files exactly as the patched `run.sh` wrote them. Its `aggregate.json` reads 98.9 % / 94.0 %, because `run.sh` truncates to one decimal (94.07 %). The `latest.json` it also wrote is byte-identical and left out. |
| `capture/<repo>/` | Per question: `<id>.myc.txt` (the complete `myc` output), `<id>.files.txt` (the files the baseline read) and `<id>.bytes.txt` (characters of the file listing, characters of the grep output, bytes read, number of files read). |

Not included: the grep outputs (about 8 MB; they can be regenerated from the
commits above), the run log (it contains local paths), and the cloned
repositories.

The JSON files were written during the re-run and are kept as written, so
their keys and free text are German. References inside them to `skripte/` and
`BERICHT.md` point to the working directory of the re-run, which is not part
of this repository; the `skripte/run-nachrechnung.diff` named in
`neulauf.json` is the same patch without change 4, and the main run used
`run-sh.patch`. The keys you need:

| Key | Meaning |
|---|---|
| `ohne` / `mit` | without / with Myceliums (grep baseline / one `myc` command) |
| `tokens`, `aufrufe` | estimated tokens (characters ÷ 4), calls |
| `zeichen_glob`, `zeichen_grep`, `bytes_gelesen`, `dateien_gelesen` | characters of the file listing, characters of the grep output, bytes of the files read, number of files read |
| `gepoolt`, `mittel`, `median`, `faktor` | pooled (ratio of the sums), mean, median, factor |
| `ausgabe_status` | `ok`, or `kein_treffer` when `myc` printed only `(no results)` |
| `veroeffentlicht` | the published v0.2.0 value |
| `sensitivitaet_nur_grep` | baseline counted as the grep output alone |
| `gesamt_neu_nur_fragen_mit_treffer` | totals over the 30 questions with a non-empty `myc` output |
| `gleichwertigkeit`: `beantwortet` / `teilweise` / `nicht` | spot check: fully / partly / not answered; `referenz` is the reference, `*_begruendung` the reasoning |

## Repeating it

```bash
# in a scratch checkout of c5dff11
git apply /path/to/run-sh.patch
cargo build --release -p myc

# one shallow clone per repository, at the commit in the table above
mkdir -p /tmp/myc-nachrechnung-repos/zod && cd /tmp/myc-nachrechnung-repos/zod
git init -q && git fetch -q --depth 1 https://github.com/colinhacks/zod.git 680d19f7d1e042e55934209caeb79bc843b94f44 && git checkout -q FETCH_HEAD
cd -

mkdir -p /tmp/rerun/results /tmp/rerun/capture /tmp/rerun/home-zod
NACH_BENCH="$PWD/benchmarks" NACH_MYC="$PWD/target/release/myc" \
NACH_RESULTS=/tmp/rerun/results NACH_RAW=/tmp/rerun/capture HOME=/tmp/rerun/home-zod \
  bash benchmarks/run.sh --repo zod --skip-clone
```
