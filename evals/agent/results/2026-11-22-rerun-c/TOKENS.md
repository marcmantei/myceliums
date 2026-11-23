# Token attribution: 2026-11-22-rerun-c

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

`none` rows come from the base run [`2026-11-22-scale`](../2026-11-22-scale/TOKENS.md), the same tasks at the same budgets; `C` rows are this run's.

Tables cover the 27 tasks of the primary analysis (without `django-tests-import-string`, `django-tests-number-format`, `django-tests-split-domain-port`).

## Reconciliation

- 60 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 6,699,602; session totals (result events): 6,699,602; largest difference in one session 0.1 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## 10 turns

Sessions: `none` 27 (23 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 49,636 | 63,593 | +13,957 | 0.0091 | 0.0183 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 9,035 | +9,035 | 0.0000 | 0.0115 | 0 | 2,728 |
| Hook text (myceliums hooks) | 0 | 1,519 | +1,519 | 0.0000 | 0.0021 | 0 | 512 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 23,285 | 8,277 | −15,008 | 0.0299 | 0.0125 | 7,137 | 2,991 |
| File reads (Read, cat/sed/head in Bash) | 1,980 | 30 | −1,950 | 0.0017 | 0.0001 | 375 | 15 |
| Other tool results | 23 | 158 | +135 | 0.0001 | 0.0006 | 23 | 144 |
| Model's earlier replies, re-read | 9,003 | 4,853 | −4,150 | 0.0124 | 0.0076 | 2,944 | 1,832 |
| Output (generated, incl. thinking) | 3,745 | 2,745 | −999 | 0.0374 | 0.0275 | – | – |
| **Total** | **87,671** | **90,210** | **+2,539** | **0.0906** | **0.0802** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0459 / 0.0451; Cache read 0.0072 / 0.0076; Output 0.0374 / 0.0275 (`none` / `C`); CLI estimate 0.0906 / 0.0802.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 102,918 [78,121, 138,420], `C` 101,486 [78,207, 133,837], ratio 0.99× [0.77×, 1.24×]; cost `none` 0.1064 [0.0811, 0.1446] USD, `C` 0.0902 [0.0686, 0.1211] USD, ratio 0.85× [0.67×, 1.06×].

## 40 turns

Sessions: `none` 27 (26 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 55,786 | 86,642 | +30,857 | 0.0114 | 0.0186 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 16,877 | +16,877 | 0.0000 | 0.0146 | 0 | 3,298 |
| Hook text (myceliums hooks) | 0 | 2,491 | +2,491 | 0.0000 | 0.0025 | 0 | 589 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 23,697 | 16,562 | −7,136 | 0.0279 | 0.0175 | 6,542 | 4,061 |
| File reads (Read, cat/sed/head in Bash) | 8,284 | 3,821 | −4,463 | 0.0069 | 0.0050 | 1,546 | 1,177 |
| Edits | 14 | 0 | −14 | 0.0000 | 0.0000 | 2 | 0 |
| Other tool results | 273 | 128 | −145 | 0.0003 | 0.0003 | 66 | 63 |
| Model's earlier replies, re-read | 12,481 | 12,730 | +249 | 0.0137 | 0.0114 | 3,199 | 2,606 |
| Output (generated, incl. thinking) | 4,147 | 3,579 | −568 | 0.0415 | 0.0358 | – | – |
| **Total** | **104,682** | **142,831** | **+38,148** | **0.1016** | **0.1056** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0514 / 0.0573; Cache read 0.0088 / 0.0125; Output 0.0415 / 0.0358 (`none` / `C`); CLI estimate 0.1016 / 0.1056.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 108,708 [84,093, 142,201], `C` 160,684 [107,409, 252,287], ratio 1.48× [1.08×, 1.96×]; cost `none` 0.1055 [0.0863, 0.1307] USD, `C` 0.1189 [0.0877, 0.1674] USD, ratio 1.13× [0.92×, 1.39×].

## All budgets together

Sessions: `none` 54 (49 solved), `C` 54 (48 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 52,711 | 75,118 | +22,407 | 0.0102 | 0.0184 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 12,956 | +12,956 | 0.0000 | 0.0130 | 0 | 3,013 |
| Hook text (myceliums hooks) | 0 | 2,005 | +2,005 | 0.0000 | 0.0023 | 0 | 551 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 23,491 | 12,419 | −11,072 | 0.0289 | 0.0150 | 6,840 | 3,526 |
| File reads (Read, cat/sed/head in Bash) | 5,132 | 1,925 | −3,206 | 0.0043 | 0.0025 | 960 | 596 |
| Edits | 7 | 0 | −7 | 0.0000 | 0.0000 | 1 | 0 |
| Other tool results | 148 | 143 | −5 | 0.0002 | 0.0004 | 45 | 104 |
| Model's earlier replies, re-read | 10,742 | 8,791 | −1,951 | 0.0130 | 0.0095 | 3,072 | 2,219 |
| Output (generated, incl. thinking) | 3,946 | 3,162 | −784 | 0.0395 | 0.0316 | – | – |
| **Total** | **96,177** | **116,520** | **+20,343** | **0.0961** | **0.0929** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0486 / 0.0512; Cache read 0.0080 / 0.0101; Output 0.0395 / 0.0316 (`none` / `C`); CLI estimate 0.0961 / 0.0929.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 105,991 [82,168, 139,053], `C` 131,085 [94,742, 190,467], ratio 1.24× [0.97×, 1.53×]; cost `none` 0.1059 [0.0846, 0.1354] USD, `C` 0.1045 [0.0796, 0.1411] USD, ratio 0.99× [0.83×, 1.16×].

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
