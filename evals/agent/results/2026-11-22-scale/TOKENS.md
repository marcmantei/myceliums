# Token attribution: 2026-11-22-scale

Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). Per-session rows: `tokens.jsonl`.

Tables cover the 27 tasks of the primary analysis (without `django-tests-import-string`, `django-tests-number-format`, `django-tests-split-domain-port`).

## Reconciliation

- 180 sessions of this run, attribution method usage-delta.
- Input tokens attributed to categories: 14,655,993; session totals (result events): 14,655,994; largest difference in one session 0.2 tokens (`tokens.jsonl` rounds each category to 0.1 token).
- Residual (session totals − summed per-call usage): cache_creation_input_tokens +0, cache_read_input_tokens +0, input_tokens +0; sessions with a non-zero residual: 0.
- Cost priced per class with the table of 2026-11-03 vs the CLI's `total_cost_usd`: largest difference per session 5e-07 USD.

## 4 turns

Sessions: `none` 27 (6 solved), `C` 27 (10 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 28,123 | 43,531 | +15,408 | 0.0091 | 0.0158 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 3,222 | +3,222 | 0.0000 | 0.0065 | 0 | 1,581 |
| Hook text (myceliums hooks) | 0 | 348 | +348 | 0.0000 | 0.0008 | 0 | 205 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 3,777 | 1,872 | −1,906 | 0.0094 | 0.0049 | 2,324 | 1,214 |
| File reads (Read, cat/sed/head in Bash) | 214 | 360 | +145 | 0.0004 | 0.0014 | 102 | 360 |
| Other tool results | 55 | 23 | −31 | 0.0001 | 0.0001 | 36 | 23 |
| Model's earlier replies, re-read | 1,274 | 1,067 | −207 | 0.0033 | 0.0027 | 826 | 662 |
| Output (generated, incl. thinking) | 1,354 | 1,353 | −1 | 0.0135 | 0.0135 | – | – |
| **Total** | **34,797** | **51,774** | **+16,977** | **0.0360** | **0.0458** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0196 / 0.0279; Cache read 0.0029 / 0.0043; Output 0.0135 / 0.0135 (`none` / `C`); CLI estimate 0.0360 / 0.0458.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 156,587 [89,348, 470,947], `C` 139,791 [88,700, 273,435], ratio 0.89× [0.35×, 1.76×]; cost `none` 0.1620 [0.0956, 0.4584] USD, `C` 0.1237 [0.0791, 0.2358] USD, ratio 0.76× [0.32×, 1.41×] (13 resamples without a solved task skipped).

## 10 turns

Sessions: `none` 27 (23 solved), `C` 27 (22 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 49,636 | 67,405 | +17,769 | 0.0091 | 0.0168 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 8,442 | +8,442 | 0.0000 | 0.0093 | 0 | 2,173 |
| Hook text (myceliums hooks) | 0 | 1,215 | +1,215 | 0.0000 | 0.0016 | 0 | 385 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 23,285 | 7,490 | −15,795 | 0.0299 | 0.0104 | 7,137 | 2,474 |
| File reads (Read, cat/sed/head in Bash) | 1,980 | 1,217 | −763 | 0.0017 | 0.0013 | 375 | 306 |
| Other tool results | 23 | 0 | −23 | 0.0001 | 0.0000 | 23 | 0 |
| Model's earlier replies, re-read | 9,003 | 5,422 | −3,581 | 0.0124 | 0.0071 | 2,944 | 1,683 |
| Output (generated, incl. thinking) | 3,745 | 2,412 | −1,333 | 0.0374 | 0.0241 | – | – |
| **Total** | **87,671** | **93,603** | **+5,932** | **0.0906** | **0.0706** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0459 / 0.0383; Cache read 0.0072 / 0.0082; Output 0.0374 / 0.0241 (`none` / `C`); CLI estimate 0.0906 / 0.0706.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 102,918 [78,121, 138,420], `C` 114,877 [81,553, 165,501], ratio 1.12× [0.84×, 1.46×]; cost `none` 0.1064 [0.0811, 0.1446] USD, `C` 0.0867 [0.0632, 0.1233] USD, ratio 0.82× [0.62×, 1.06×].

## 40 turns

Sessions: `none` 27 (26 solved), `C` 27 (24 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 55,786 | 88,161 | +32,375 | 0.0114 | 0.0183 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 10,806 | +10,806 | 0.0000 | 0.0098 | 0 | 2,240 |
| Hook text (myceliums hooks) | 0 | 2,258 | +2,258 | 0.0000 | 0.0024 | 0 | 551 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 23,697 | 15,185 | −8,512 | 0.0279 | 0.0165 | 6,542 | 3,847 |
| File reads (Read, cat/sed/head in Bash) | 8,284 | 4,662 | −3,622 | 0.0069 | 0.0042 | 1,546 | 950 |
| Edits | 14 | 0 | −14 | 0.0000 | 0.0000 | 2 | 0 |
| Other tool results | 273 | 199 | −74 | 0.0003 | 0.0002 | 66 | 44 |
| Model's earlier replies, re-read | 12,481 | 10,803 | −1,678 | 0.0137 | 0.0123 | 3,199 | 2,885 |
| Output (generated, incl. thinking) | 4,147 | 3,619 | −529 | 0.0415 | 0.0362 | – | – |
| **Total** | **104,682** | **135,692** | **+31,010** | **0.1016** | **0.0999** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0514 / 0.0518; Cache read 0.0088 / 0.0119; Output 0.0415 / 0.0362 (`none` / `C`); CLI estimate 0.1016 / 0.0999.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 108,708 [84,093, 142,201], `C` 152,654 [111,689, 209,458], ratio 1.40× [1.17×, 1.73×]; cost `none` 0.1055 [0.0863, 0.1307] USD, `C` 0.1124 [0.0828, 0.1559] USD, ratio 1.07× [0.88×, 1.31×].

## All budgets together

Sessions: `none` 81 (55 solved), `C` 81 (56 solved). Means per session.

| Category | `none` tokens | `C` tokens | `C` − `none` | `none` USD | `C` USD | result size `none` | result size `C` |
|---|---:|---:|---:|---:|---:|---:|---:|
| Base context (system prompt, tool definitions, task prompt) | 44,515 | 66,366 | +21,851 | 0.0099 | 0.0170 | – | – |
| myceliums results (MCP + `myc` CLI) | 0 | 7,490 | +7,490 | 0.0000 | 0.0085 | 0 | 1,998 |
| Hook text (myceliums hooks) | 0 | 1,274 | +1,274 | 0.0000 | 0.0016 | 0 | 380 |
| Search results (Grep, Glob, grep/rg/find in Bash) | 16,920 | 8,182 | −8,738 | 0.0224 | 0.0106 | 5,334 | 2,511 |
| File reads (Read, cat/sed/head in Bash) | 3,493 | 2,080 | −1,413 | 0.0030 | 0.0023 | 674 | 538 |
| Edits | 5 | 0 | −5 | 0.0000 | 0.0000 | 1 | 0 |
| Other tool results | 117 | 74 | −43 | 0.0002 | 0.0001 | 42 | 22 |
| Model's earlier replies, re-read | 7,586 | 5,764 | −1,822 | 0.0098 | 0.0074 | 2,323 | 1,743 |
| Output (generated, incl. thinking) | 3,082 | 2,461 | −621 | 0.0308 | 0.0246 | – | – |
| **Total** | **75,717** | **93,690** | **+17,973** | **0.0761** | **0.0721** | | |

Cost by class (USD per session): Uncached input 0.0000 / 0.0000; Cache write (1 h) 0.0389 / 0.0394; Cache read 0.0063 / 0.0081; Output 0.0308 / 0.0246 (`none` / `C`); CLI estimate 0.0761 / 0.0721.

Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over 27 tasks): tokens `none` 111,510 [85,765, 146,673], `C` 135,516 [100,237, 183,534], ratio 1.22× [1.01×, 1.45×]; cost `none` 0.1120 [0.0888, 0.1441] USD, `C` 0.1043 [0.0782, 0.1415] USD, ratio 0.93× [0.78×, 1.10×].

*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later call) and output tokens; *result size* is a result's own size when it entered the context (its share of the growth between two calls).
