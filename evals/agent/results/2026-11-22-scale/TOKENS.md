# Token attribution: 2026-11-22-scale

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

Tables cover the 27 tasks of the primary analysis (without `django-tests-import-string`, `django-tests-number-format`, `django-tests-split-domain-port`).

## Model and prices

- `claude-sonnet-5-5`: uncached input $2, cache write (5 min) $2.5, cache write (1 h) $4, cache read $0.1, output $10 per million tokens
- Prices: [https://platform.claude.com/docs/en/about-claude/pricing](https://platform.claude.com/docs/en/about-claude/pricing), as of 2026-11-03.

## Reconciliation

The residual is zero by construction of what it compares: it checks that the per-call usage in the transcript adds up to the session totals (no hidden model calls), not how a call's growth is split between categories. The split is checked separately below.

- 180 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 14,655,993; session totals (result events): 14,655,994; largest difference in one session 0.1 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## Cross-check of the split

Each call's measured context growth fitted by least squares on the chars/4 estimates of what arrived before it (850 main-agent calls of this run; R² = 0.9802; a constant of 43.45 tokens per call). If the estimate undercounts every kind of content by the same factor, the proportional split is fair between them:

| Part | Calls with it | Measured tokens per estimated token | Tokens per character |
|---|---:|---:|---:|
| hook | 265 | 1.670 | 0.417 |
| myceliums | 118 | 1.783 | 0.446 |
| read | 37 | 1.736 | 0.434 |
| reply | 850 | 1.925 | 0.481 |
| search | 728 | 1.711 | 0.428 |
| thinking | 448 | 1.071 | (CLI estimate) |

Left out (in fewer than 20 calls, or the same in every call): edit, other.

The *fitted split* columns below re-split every growth with these rates instead of 1 (the attribution's sensitivity to the chars/4 assumption).

## 4 turns

Sessions: `none` 27 (6 solved), `C` 27 (10 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 28,123 | 43,531 | +15,408 | 0.0091 | 0.0158 | – | – | 28,123 | 43,531 |
| myceliums results (MCP + `myc` CLI) | 0 | 3,222 | +3,222 | 0.0000 | 0.0065 | 0 | 1,581 | 0 | 3,259 |
| Hook text (myceliums hooks) | 0 | 348 | +348 | 0.0000 | 0.0008 | 0 | 205 | 0 | 349 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 3,942 | 1,875 | −2,067 | 0.0098 | 0.0049 | 2,415 | 1,218 | 4,025 | 1,914 |
| File reads (Read, cat/sed/head in Bash) | 66 | 356 | +290 | 0.0001 | 0.0014 | 28 | 356 | 68 | 362 |
| Other tool results | 38 | 23 | −14 | 0.0001 | 0.0001 | 19 | 23 | 27 | 17 |
| Model's earlier replies, re-read | 1,274 | 1,067 | −207 | 0.0033 | 0.0027 | 826 | 662 | 1,200 | 989 |
| Output (generated, incl. thinking) | 1,354 | 1,353 | −1 | 0.0135 | 0.0135 | – | – | 1,354 | 1,353 |
| **Total** | **34,797** | **51,774** | **+16,977** | **0.0360** | **0.0458** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0196 / 0.0279; Cache read 0.0029 / 0.0043; Output 0.0135 / 0.0135 (`none` / `C`); CLI estimate 0.0360 / 0.0458.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 156,587 [89,348, 470,947], `C` 139,791 [88,700, 273,435], ratio 0.89× [0.35×, 1.76×]; cost `none` 0.1620 [0.0956, 0.4584] USD, `C` 0.1237 [0.0791, 0.2358] USD, ratio 0.76× [0.32×, 1.41×] (13 resamples without a solved task skipped).

## 10 turns

Sessions: `none` 27 (23 solved), `C` 27 (22 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 49,636 | 67,405 | +17,769 | 0.0091 | 0.0168 | – | – | 49,636 | 67,405 |
| myceliums results (MCP + `myc` CLI) | 0 | 8,442 | +8,442 | 0.0000 | 0.0093 | 0 | 2,173 | 0 | 8,809 |
| Hook text (myceliums hooks) | 0 | 1,215 | +1,215 | 0.0000 | 0.0016 | 0 | 385 | 0 | 1,261 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 24,191 | 7,974 | −16,217 | 0.0306 | 0.0110 | 7,278 | 2,603 | 25,178 | 8,241 |
| File reads (Read, cat/sed/head in Bash) | 1,074 | 734 | −340 | 0.0010 | 0.0008 | 234 | 177 | 1,116 | 800 |
| Other tool results | 23 | 0 | −23 | 0.0001 | 0.0000 | 23 | 0 | 18 | 0 |
| Model's earlier replies, re-read | 9,003 | 5,422 | −3,581 | 0.0124 | 0.0071 | 2,944 | 1,683 | 7,979 | 4,676 |
| Output (generated, incl. thinking) | 3,745 | 2,412 | −1,333 | 0.0374 | 0.0241 | – | – | 3,745 | 2,412 |
| **Total** | **87,671** | **93,603** | **+5,932** | **0.0906** | **0.0706** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0459 / 0.0383; Cache read 0.0072 / 0.0082; Output 0.0374 / 0.0241 (`none` / `C`); CLI estimate 0.0906 / 0.0706.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 102,918 [78,121, 138,420], `C` 114,877 [81,553, 165,501], ratio 1.12× [0.84×, 1.46×]; cost `none` 0.1064 [0.0811, 0.1446] USD, `C` 0.0867 [0.0632, 0.1233] USD, ratio 0.82× [0.62×, 1.06×].

## 40 turns

Sessions: `none` 27 (26 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 55,786 | 88,161 | +32,375 | 0.0114 | 0.0183 | – | – | 55,786 | 88,161 |
| myceliums results (MCP + `myc` CLI) | 0 | 10,806 | +10,806 | 0.0000 | 0.0098 | 0 | 2,240 | 0 | 11,230 |
| Hook text (myceliums hooks) | 0 | 2,258 | +2,258 | 0.0000 | 0.0024 | 0 | 551 | 0 | 2,370 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 25,604 | 16,750 | −8,854 | 0.0295 | 0.0177 | 6,914 | 4,114 | 26,434 | 17,536 |
| File reads (Read, cat/sed/head in Bash) | 6,568 | 3,283 | −3,285 | 0.0054 | 0.0032 | 1,204 | 724 | 6,778 | 3,395 |
| Edits | 14 | 0 | −14 | 0.0000 | 0.0000 | 2 | 0 | 8 | 0 |
| Other tool results | 81 | 12 | −69 | 0.0001 | 0.0000 | 36 | 1 | 60 | 10 |
| Model's earlier replies, re-read | 12,481 | 10,803 | −1,678 | 0.0137 | 0.0123 | 3,199 | 2,885 | 11,469 | 9,372 |
| Output (generated, incl. thinking) | 4,147 | 3,619 | −529 | 0.0415 | 0.0362 | – | – | 4,147 | 3,619 |
| **Total** | **104,682** | **135,692** | **+31,010** | **0.1016** | **0.0999** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0514 / 0.0518; Cache read 0.0088 / 0.0119; Output 0.0415 / 0.0362 (`none` / `C`); CLI estimate 0.1016 / 0.0999.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 108,709 [84,093, 142,201], `C` 152,654 [111,689, 209,458], ratio 1.40× [1.17×, 1.73×]; cost `none` 0.1055 [0.0863, 0.1307] USD, `C` 0.1124 [0.0828, 0.1559] USD, ratio 1.07× [0.88×, 1.31×].

## All budgets together

Sessions: `none` 81 (55 solved), `C` 81 (56 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` | fitted split `none` | fitted split `C` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 44,515 | 66,366 | +21,851 | 0.0099 | 0.0170 | – | – | 44,515 | 66,366 |
| myceliums results (MCP + `myc` CLI) | 0 | 7,490 | +7,490 | 0.0000 | 0.0085 | 0 | 1,998 | 0 | 7,766 |
| Hook text (myceliums hooks) | 0 | 1,274 | +1,274 | 0.0000 | 0.0016 | 0 | 380 | 0 | 1,327 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 17,913 | 8,866 | −9,046 | 0.0233 | 0.0112 | 5,536 | 2,645 | 18,546 | 9,230 |
| File reads (Read, cat/sed/head in Bash) | 2,569 | 1,458 | −1,112 | 0.0022 | 0.0018 | 488 | 419 | 2,654 | 1,519 |
| Edits | 5 | 0 | −5 | 0.0000 | 0.0000 | 1 | 0 | 3 | 0 |
| Other tool results | 47 | 12 | −36 | 0.0001 | 0.0000 | 26 | 8 | 35 | 9 |
| Model's earlier replies, re-read | 7,586 | 5,764 | −1,822 | 0.0098 | 0.0074 | 2,323 | 1,743 | 6,883 | 5,012 |
| Output (generated, incl. thinking) | 3,082 | 2,461 | −621 | 0.0308 | 0.0246 | – | – | 3,082 | 2,461 |
| **Total** | **75,717** | **93,690** | **+17,973** | **0.0761** | **0.0721** | | | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0389 / 0.0394; Cache read 0.0063 / 0.0081; Output 0.0308 / 0.0246 (`none` / `C`); CLI estimate 0.0761 / 0.0721.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 111,510 [85,765, 146,673], `C` 135,516 [100,237, 183,534], ratio 1.22× [1.01×, 1.45×]; cost `none` 0.1120 [0.0888, 0.1441] USD, `C` 0.1043 [0.0782, 0.1415] USD, ratio 0.93× [0.78×, 1.10×].

## Search audit

Per condition, over its sessions: Grep/Glob calls, Bash search commands (blocked by a hook / ran), Bash calls running an interpreter, Read calls, blind reads (a file whose name appeared nowhere before), and sessions flagged (any Grep/Glob, search that ran, interpreter that ran, or ≥ 5 blind reads). In an arm with search this measures how much the agent searched; in D every flag is a hole in the isolation.

| Condition | Sessions | Grep/Glob | Bash search blocked / ran | Interpreter calls ran | Reads | Blind reads | Flagged |
|---|---:|---:|---:|---:|---:|---:|---:|
| `C` | 81 | 53 | 0 / 223 | 4 | 15 | 0 | 72 |
| `none` | 81 | 71 | 0 / 354 | 24 | 3 | 0 | 81 |

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
