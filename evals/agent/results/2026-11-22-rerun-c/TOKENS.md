# Token attribution: 2026-11-22-rerun-c

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

`none` rows come from the base run [`2026-11-22-scale`](../2026-11-22-scale/TOKENS.md), the same tasks at the same budgets; `C` rows are this run's.

Tables cover the 27 tasks of the primary analysis (without `django-tests-import-string`, `django-tests-number-format`, `django-tests-split-domain-port`).

## Model and prices

- `claude-sonnet-5-5`: uncached input $2, cache write (5 min) $2.5, cache write (1 h) $4, cache read $0.1, output $10 per million tokens
- Prices: [https://platform.claude.com/docs/en/about-claude/pricing](https://platform.claude.com/docs/en/about-claude/pricing), as of 2026-11-03.

## Reconciliation

The residual is zero by construction of what it compares: it checks that the per-call usage in the transcript adds up to the session totals (no hidden model calls), not how a call's growth is split between categories. The split is checked separately below.

- 60 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 6,699,602; session totals (result events): 6,699,602; largest difference in one session 0.1 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## Cross-check of the split

Each call's measured context growth fitted by least squares on the chars/4 estimates of what arrived before it (288 main-agent calls of this run; R² = 0.9733; a constant of 33.73 tokens per call). If the estimate undercounts every kind of content by the same factor, the proportional split is fair between them:

| Part | Calls with it | Measured tokens per estimated token | Tokens per character |
|---|---:|---:|---:|
| hook | 204 | 1.350 | 0.338 |
| myceliums | 71 | 1.812 | 0.453 |
| read | 21 | 1.675 | 0.419 |
| reply | 288 | 2.919 | 0.730 |
| search | 218 | 1.688 | 0.422 |
| thinking | 156 | 0.993 | (CLI estimate) |

Left out (in fewer than 20 calls, or the same in every call): other.

The *fitted split* columns below re-split every growth with these rates instead of 1 (the attribution's sensitivity to the chars/4 assumption).

## 10 turns

Sessions: `none` 27 (23 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 49,636 | 63,593 | +13,957 | 0.0091 | 0.0183 | – | – | 49,636 | 63,593 |
| myceliums results (MCP + `myc` CLI) | 0 | 9,035 | +9,035 | 0.0000 | 0.0115 | 0 | 2,728 | 0 | 9,065 |
| Hook text (myceliums hooks) | 0 | 1,519 | +1,519 | 0.0000 | 0.0021 | 0 | 512 | 0 | 1,289 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 24,191 | 8,304 | −15,887 | 0.0306 | 0.0126 | 7,278 | 3,004 | 25,178 | 8,485 |
| File reads (Read, cat/sed/head in Bash) | 1,074 | 30 | −1,044 | 0.0010 | 0.0001 | 234 | 15 | 1,116 | 23 |
| Other tool results | 23 | 131 | +108 | 0.0001 | 0.0005 | 23 | 131 | 18 | 102 |
| Model's earlier replies, re-read | 9,003 | 4,853 | −4,150 | 0.0124 | 0.0076 | 2,944 | 1,832 | 7,979 | 4,907 |
| Output (generated, incl. thinking) | 3,745 | 2,745 | −999 | 0.0374 | 0.0275 | – | – | 3,745 | 2,745 |
| **Total** | **87,671** | **90,210** | **+2,539** | **0.0906** | **0.0802** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0459 / 0.0451; Cache read 0.0072 / 0.0076; Output 0.0374 / 0.0275 (`none` / `C`); CLI estimate 0.0906 / 0.0802.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 102,918 [78,121, 138,420], `C` 101,486 [78,207, 133,837], ratio 0.99× [0.77×, 1.24×]; cost `none` 0.1064 [0.0811, 0.1446] USD, `C` 0.0902 [0.0686, 0.1211] USD, ratio 0.85× [0.67×, 1.06×].

## 40 turns

Sessions: `none` 27 (26 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 55,786 | 86,642 | +30,857 | 0.0114 | 0.0186 | – | – | 55,786 | 86,642 |
| myceliums results (MCP + `myc` CLI) | 0 | 16,877 | +16,877 | 0.0000 | 0.0146 | 0 | 3,298 | 0 | 16,966 |
| Hook text (myceliums hooks) | 0 | 2,491 | +2,491 | 0.0000 | 0.0025 | 0 | 589 | 0 | 2,150 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 25,604 | 16,791 | −8,813 | 0.0295 | 0.0179 | 6,914 | 4,156 | 26,434 | 17,451 |
| File reads (Read, cat/sed/head in Bash) | 6,568 | 3,596 | −2,972 | 0.0054 | 0.0046 | 1,204 | 1,087 | 6,778 | 3,512 |
| Edits | 14 | 0 | −14 | 0.0000 | 0.0000 | 2 | 0 | 8 | 0 |
| Other tool results | 81 | 124 | +42 | 0.0001 | 0.0002 | 36 | 58 | 60 | 82 |
| Model's earlier replies, re-read | 12,481 | 12,730 | +249 | 0.0137 | 0.0114 | 3,199 | 2,606 | 11,469 | 12,447 |
| Output (generated, incl. thinking) | 4,147 | 3,579 | −568 | 0.0415 | 0.0358 | – | – | 4,147 | 3,579 |
| **Total** | **104,682** | **142,831** | **+38,148** | **0.1016** | **0.1056** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0514 / 0.0573; Cache read 0.0088 / 0.0125; Output 0.0415 / 0.0358 (`none` / `C`); CLI estimate 0.1016 / 0.1056.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 108,709 [84,093, 142,201], `C` 160,684 [107,409, 252,287], ratio 1.48× [1.08×, 1.96×]; cost `none` 0.1055 [0.0863, 0.1307] USD, `C` 0.1189 [0.0877, 0.1674] USD, ratio 1.13× [0.92×, 1.39×].

## All budgets together

Sessions: `none` 54 (49 solved), `C` 54 (48 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 52,711 | 75,118 | +22,407 | 0.0102 | 0.0184 | – | – | 52,711 | 75,118 |
| myceliums results (MCP + `myc` CLI) | 0 | 12,956 | +12,956 | 0.0000 | 0.0130 | 0 | 3,013 | 0 | 13,016 |
| Hook text (myceliums hooks) | 0 | 2,005 | +2,005 | 0.0000 | 0.0023 | 0 | 551 | 0 | 1,720 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 24,898 | 12,547 | −12,350 | 0.0301 | 0.0152 | 7,096 | 3,580 | 25,806 | 12,968 |
| File reads (Read, cat/sed/head in Bash) | 3,821 | 1,813 | −2,008 | 0.0032 | 0.0023 | 719 | 551 | 3,947 | 1,767 |
| Edits | 7 | 0 | −7 | 0.0000 | 0.0000 | 1 | 0 | 4 | 0 |
| Other tool results | 52 | 127 | +75 | 0.0001 | 0.0004 | 30 | 95 | 39 | 92 |
| Model's earlier replies, re-read | 10,742 | 8,791 | −1,951 | 0.0130 | 0.0095 | 3,072 | 2,219 | 9,724 | 8,677 |
| Output (generated, incl. thinking) | 3,946 | 3,162 | −784 | 0.0395 | 0.0316 | – | – | 3,946 | 3,162 |
| **Total** | **96,177** | **116,520** | **+20,343** | **0.0961** | **0.0929** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0486 / 0.0512; Cache read 0.0080 / 0.0101; Output 0.0395 / 0.0316 (`none` / `C`); CLI estimate 0.0961 / 0.0929.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 105,991 [82,168, 139,053], `C` 131,085 [94,742, 190,467], ratio 1.24× [0.97×, 1.53×]; cost `none` 0.1059 [0.0846, 0.1354] USD, `C` 0.1045 [0.0796, 0.1411] USD, ratio 0.99× [0.83×, 1.16×].

## Search audit

Per condition, over its sessions: Grep/Glob calls, Bash search commands (blocked by a hook / ran), Bash calls running an interpreter, Read calls, blind reads (a file whose name appeared nowhere before), and sessions flagged (any Grep/Glob, search that ran, interpreter that ran, or ≥ 5 blind reads). In an arm with search this measures how much the agent searched; in D every flag is a hole in the isolation.

| Condition | Sessions | Grep/Glob | Bash search blocked / ran | Interpreter calls ran | Reads | Blind reads | Flagged |
|---|---:|---:|---:|---:|---:|---:|---:|
| `C` | 54 | 46 | 0 / 157 | 14 | 9 | 0 | 49 |

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
