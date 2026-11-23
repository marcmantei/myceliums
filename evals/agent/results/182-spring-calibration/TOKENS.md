# Token attribution: 182-spring-calibration

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

## Model and prices

- `claude-sonnet-5-5`: uncached input $2, cache write (5 min) $2.5, cache write (1 h) $4, cache read $0.1, output $10 per million tokens
- Prices: [https://platform.claude.com/docs/en/about-claude/pricing](https://platform.claude.com/docs/en/about-claude/pricing), as of 2026-11-03.

## Reconciliation

The residual is zero by construction of what it compares: it checks that the per-call usage in the transcript adds up to the session totals (no hidden model calls), not how a call's growth is split between categories. The split is checked separately below.

- 30 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 1,346,425; session totals (result events): 1,346,425; largest difference in one session 0.1 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## Cross-check of the split

Each call's measured context growth fitted by least squares on the chars/4 estimates of what arrived before it (84 main-agent calls of this run; R² = 0.894; a constant of 292.2 tokens per call). If the estimate undercounts every kind of content by the same factor, the proportional split is fair between them:

| Part | Calls with it | Measured tokens per estimated token | Tokens per character |
|---|---:|---:|---:|
| reply | 84 | 1.735 | 0.434 |
| search | 78 | 1.599 | 0.400 |
| thinking | 29 | 0.364 | (CLI estimate) |

Left out (in fewer than 20 calls, or the same in every call): read.

The *fitted split* columns below re-split every growth with these rates instead of 1 (the attribution's sensitivity to the chars/4 assumption).

## All sessions

Sessions: `none` 30 (29 solved), `none` 30 (29 solved). Means per session.

| Category | `none` tokens | `none` tokens | `none` − `none` | `none` USD | `none` USD | result size `none` | result size `none` | fitted split `none` | fitted split `none` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 33,817 | 33,817 | +0 | 0.0164 | 0.0164 | – | – | 33,817 | 33,817 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 7,797 | 7,797 | +0 | 0.0122 | 0.0122 | 2,929 | 2,929 | 8,397 | 8,397 |
| File reads (Read, cat/sed/head in Bash) | 958 | 958 | +0 | 0.0013 | 0.0013 | 296 | 296 | 861 | 861 |
| Model's earlier replies, re-read | 2,309 | 2,309 | +0 | 0.0033 | 0.0033 | 775 | 775 | 1,805 | 1,805 |
| Output (generated, incl. thinking) | 1,597 | 1,597 | +0 | 0.0160 | 0.0160 | – | – | 1,597 | 1,597 |
| **Total** | **46,477** | **46,477** | **+0** | **0.0490** | **0.0490** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0293 / 0.0293; Cache read 0.0038 / 0.0038; Output 0.0160 / 0.0160 (`none` / `none`); CLI estimate 0.0490 / 0.0490.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 30 tasks): tokens `none` – [–, –], `none` 48,080 [–, –], ratio – [–, –]; cost `none` – [–, –] USD, `none` 0.0507 [–, –] USD, ratio – [–, –] (10000 resamples without a solved task skipped).

## Search audit

Per condition, over its sessions: Grep/Glob calls, Bash search commands (blocked by a hook / ran), Bash calls running an interpreter, Read calls, blind reads (a file whose name appeared nowhere before), and sessions flagged (any Grep/Glob, search that ran, interpreter that ran, or ≥ 5 blind reads). In an arm with search this measures how much the agent searched; in D every flag is a hole in the isolation.

| Condition | Sessions | Grep/Glob | Bash search blocked / ran | Interpreter calls ran | Reads | Blind reads | Flagged |
|---|---:|---:|---:|---:|---:|---:|---:|
| `none` | 30 | 44 | 0 / 39 | 0 | 7 | 0 | 29 |

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
