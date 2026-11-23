# Token attribution: 182-spring

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

## Model and prices

- `claude-sonnet-5-5`: uncached input $2, cache write (5 min) $2.5, cache write (1 h) $4, cache read $0.1, output $10 per million tokens
- Prices: [https://platform.claude.com/docs/en/about-claude/pricing](https://platform.claude.com/docs/en/about-claude/pricing), as of 2026-11-03.

## Reconciliation

The residual is zero by construction of what it compares: it checks that the per-call usage in the transcript adds up to the session totals (no hidden model calls), not how a call's growth is split between categories. The split is checked separately below.

- 270 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 19,156,618; session totals (result events): 19,156,618; largest difference in one session 0.1 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## Cross-check of the split

Each call's measured context growth fitted by least squares on the chars/4 estimates of what arrived before it (955 main-agent calls of this run; R² = 0.9923; a constant of 33.08 tokens per call). If the estimate undercounts every kind of content by the same factor, the proportional split is fair between them:

| Part | Calls with it | Measured tokens per estimated token | Tokens per character |
|---|---:|---:|---:|
| hook | 137 | 1.799 | 0.450 |
| myceliums | 331 | 1.682 | 0.421 |
| read | 148 | 1.584 | 0.396 |
| reply | 955 | 2.858 | 0.715 |
| search | 532 | 1.692 | 0.423 |
| thinking | 378 | 0.831 | (CLI estimate) |

Left out (in fewer than 20 calls, or the same in every call): other.

The *fitted split* columns below re-split every growth with these rates instead of 1 (the attribution's sensitivity to the chars/4 assumption).

## All sessions: `C` against `none`

Sessions: `none` 90 (89 solved), `C` 90 (90 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 34,077 | 38,405 | +4,328 | 0.0076 | 0.0076 | – | – | 34,077 | 38,405 |
| myceliums results (MCP + `myc` CLI) | 0 | 478 | +478 | 0.0000 | 0.0005 | 0 | 145 | 0 | 455 |
| Hook text (myceliums hooks) | 0 | 953 | +953 | 0.0000 | 0.0014 | 0 | 388 | 0 | 961 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 7,638 | 4,870 | −2,768 | 0.0117 | 0.0085 | 3,052 | 2,376 | 7,471 | 4,729 |
| File reads (Read, cat/sed/head in Bash) | 1,040 | 319 | −721 | 0.0012 | 0.0006 | 351 | 151 | 983 | 312 |
| Other tool results | 23 | 0 | −23 | 0.0000 | 0.0000 | 11 | 0 | 14 | 0 |
| Model's earlier replies, re-read | 1,975 | 1,382 | −593 | 0.0030 | 0.0025 | 748 | 629 | 2,209 | 1,545 |
| Output (generated, incl. thinking) | 1,540 | 1,396 | −144 | 0.0154 | 0.0140 | – | – | 1,540 | 1,396 |
| **Total** | **46,293** | **47,802** | **+1,509** | **0.0390** | **0.0351** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0196 / 0.0169; Cache read 0.0040 / 0.0042; Output 0.0154 / 0.0140 (`none` / `C`); CLI estimate 0.0390 / 0.0351.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 30 tasks): tokens `none` 46,814 [35,097, 60,848], `C` 47,802 [38,457, 58,725], ratio 1.02× [0.83×, 1.23×]; cost `none` 0.0395 [0.0297, 0.0512] USD, `C` 0.0351 [0.0273, 0.0441] USD, ratio 0.89× [0.74×, 1.03×].

## All sessions: `D` against `none`

Sessions: `none` 90 (89 solved), `D` 90 (89 solved). Means per session.

| Category | `none` tokens | `D` tokens | `D` − `none` | `none` USD | `D` USD | result size `none` | result size `D` | fitted split `none` | fitted split `D` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 34,077 | 67,863 | +33,786 | 0.0076 | 0.0124 | – | – | 34,077 | 67,863 |
| myceliums results (MCP + `myc` CLI) | 0 | 37,268 | +37,268 | 0.0000 | 0.0345 | 0 | 7,948 | 0 | 37,959 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 7,638 | 1,522 | −6,116 | 0.0117 | 0.0010 | 3,052 | 279 | 7,471 | 1,341 |
| File reads (Read, cat/sed/head in Bash) | 1,040 | 8,278 | +7,238 | 0.0012 | 0.0112 | 351 | 2,654 | 983 | 8,193 |
| Other tool results | 23 | 12 | −12 | 0.0000 | 0.0000 | 11 | 4 | 14 | 10 |
| Model's earlier replies, re-read | 1,975 | 6,750 | +4,775 | 0.0030 | 0.0075 | 748 | 1,759 | 2,209 | 6,325 |
| Output (generated, incl. thinking) | 1,540 | 2,421 | +881 | 0.0154 | 0.0242 | – | – | 1,540 | 2,421 |
| **Total** | **46,293** | **124,113** | **+77,820** | **0.0390** | **0.0908** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0196 / 0.0558; Cache read 0.0040 / 0.0108; Output 0.0154 / 0.0242 (`none` / `D`); CLI estimate 0.0390 / 0.0908.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 30 tasks): tokens `none` 46,814 [35,097, 60,848], `D` 125,508 [87,517, 169,534], ratio 2.68× [1.69×, 4.17×]; cost `none` 0.0395 [0.0297, 0.0512] USD, `D` 0.0918 [0.0667, 0.1193] USD, ratio 2.33× [1.56×, 3.42×].

## Search audit

Per condition, over its sessions: Grep/Glob calls, Bash search commands (blocked by a hook / ran), Bash calls running an interpreter, Read calls, blind reads (a file whose name appeared nowhere before), and sessions flagged (any Grep/Glob, search that ran, interpreter that ran, or ≥ 5 blind reads). In an arm with search this measures how much the agent searched; in D every flag is a hole in the isolation.

| Condition | Sessions | Grep/Glob | Bash search blocked / ran | Interpreter calls ran | Reads | Blind reads | Flagged |
|---|---:|---:|---:|---:|---:|---:|---:|
| `C` | 90 | 89 | 0 / 129 | 0 | 12 | 1 | 86 |
| `D` | 90 | 0 | 93 / 0 | 0 | 273 | 8 | 0 |
| `none` | 90 | 135 | 0 / 112 | 0 | 21 | 0 | 84 |

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
