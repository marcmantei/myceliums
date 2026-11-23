# Replayed myceliums calls: #163/#171 sessions, before and after #181

Calls from the transcripts of 2026-11-22-rerun-c-t10, 2026-11-22-rerun-c-t40, 2026-11-22-scale-t10, 2026-11-22-scale-t4, 2026-11-22-scale-t40, replayed with `agent_eval.py replay` (`aeval/replay.py`) against two `myc` builds on the same indexes: before `origin/main fbbf24f`, after `feat-181-compact-output`. Indexes: Django 2d4add1 and Vue 7de9e41, built with `myc analyze --skip-embeddings` and the index exclusions of results/2026-11-22-scale/meta.json.

Token columns use `o200k_base` (tiktoken), an OpenAI tokenizer, **not Claude's**; they show the ratio, not Claude's token count.

## Tool definitions (sent with every model call)

| | chars before | chars after | change | chars/4 before | chars/4 after | o200k before | o200k after |
|---|---:|---:|---:|---:|---:|---:|---:|
| tool definitions (name, description, input schema) | 11,847 | 6,697 | -43 % | 2,961 | 1,674 | 2,807 | 1,592 |
| server instructions | 1,548 | 1,778 | +15 % | 387 | 444 | 374 | 425 |
| `tools/list` JSON | 19,264 | 9,920 | -49 % | 4,816 | 2,480 | 4,627 | 2,396 |

| tool | definition chars before | after |
|---|---:|---:|
| `analyze` | 1,333 | 806 |
| `explain_symbol` | 862 | 532 |
| `find_callees` | 1,241 | 727 |
| `find_callers` | 1,250 | 717 |
| `find_dead_code` | 1,187 | 650 |
| `impact_of_change` | 1,677 | 907 |
| `list_repos` | 657 | 366 |
| `search_code` | 2,025 | 1,062 |
| `trace_flow` | 1,623 | 938 |

## Answers

| tool | chars before | chars after | change | chars/4 before | chars/4 after | o200k before | o200k after |
|---|---:|---:|---:|---:|---:|---:|---:|
| `find_callees` (14 calls) | 78,335 | 33,043 | -58 % | 19,578 | 8,255 | 19,029 | 8,817 |
| `find_callers` (220 calls) | 956,541 | 418,522 | -56 % | 239,051 | 104,549 | 236,957 | 112,741 |
| `impact_of_change` (8 calls) | 29,185 | 20,205 | -31 % | 7,294 | 5,048 | 7,729 | 5,468 |
| `trace_flow` (23 calls) | 40,849 | 33,375 | -18 % | 10,205 | 8,337 | 11,095 | 9,452 |
| **total** | 1,104,910 | 505,145 | -54 % | 276,128 | 126,189 | 274,810 | 136,478 |

## Same answer in substance

Per call: the same hits (file, qualified name, start line, depth or chain position, `via`, `[imports]` and `resolved` marks), totals, `Incomplete` reasons and `Next` step; error results the same text.

| tool | calls | errors (same text) | differ |
|---|---:|---:|---:|
| `find_callees` | 14 | 4 | 0 |
| `find_callers` | 220 | 9 | 0 |
| `impact_of_change` | 8 | 1 | 0 |
| `trace_flow` | 23 | 7 | 0 |
