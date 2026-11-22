# Label review (#164)

Every gold/label set here was drafted by an agent (`reviewed_by: null`). This
directory makes the human review fast: one list of claims, an independent AI
pre-screen, a local review page, and a script that writes the decisions back.
Python 3.10+ standard library only.

## The three commands

```bash
python3 evals/review/export.py   # 1. build items.jsonl from every dataset (prints counts per dataset)
python3 evals/review/serve.py    # 2. review: open the printed http://127.0.0.1:<port>/ link
python3 evals/review/apply.py    # 3. write the decisions back (add --dry-run to look first)
```

Between 1 and 2 the pre-screen files go into `prescreen/` (format below). The
review can be stopped any time (Ctrl-C) and resumed by running `serve.py`
again: every key press is saved at once.

## Keys

| key | does |
|---|---|
| J | correct |
| N | wrong (then a comment box opens, pre-filled with the pre-screen's suggested fix if there is one) |
| U | unsure |
| K | comment (Enter saves, Shift+Enter new line, Esc cancels) |
| ← → | previous / next item |
| S | summary page |

## What is reviewed

`export.py` writes one item per claim to `items.jsonl` (generated, git-ignored:
it holds code snippets from the pinned third-party checkouts). Ids come from
the labels' own keys, so they are stable across re-exports.

| dataset | source | items |
|---|---|---|
| `retrieval-queries` | `crates/myceliums-benchmarks/real-repos/queries.json` | one per (query, labelled symbol), one "anything important missing?" per query |
| `retrieval-test-seeking` | `real-repos/test-seeking.json` | same |
| `graph-gold` | `graph-gold/<language>.json` | every symbol, CALLS edge, dynamic call and trap |
| `graph-held-out` | `graph-gold/held-out/real-repos.json` | one per call site |
| `agent-tasks`, `agent-tasks-hard`, `agent-tasks-hard-v1`, `agent-tasks-outcome` | `evals/agent/tasks/*.json` (#99, #125, #153) | every gold and optional answer, one "is the gold set complete?" per task |

Snippets come from the pinned checkouts in `~/.cache/myceliums-eval-repos`
(`real-repos/fetch.sh`) and `~/.cache/myceliums-agent-eval*/repos`
(`evals/agent/agent_eval.py fetch`); a checkout at another commit is not used.
`export.py --check` exits 1 when `items.jsonl` is out of date.

## Pre-screen format

`prescreen/<dataset>.jsonl`, one JSON object per line, written by an agent
other than the one that drafted the labels:

```json
{"id": "<item id>", "verdict": "confirmed|doubtful|wrong", "reason": "one or two sentences",
 "evidence": ["path/file.py:123"], "suggested_fix": "replace: path/file.py::Class.method — why"}
```

`suggested_fix` is optional and uses the comment directives below, so the
reviewer can accept it with N, Enter. An item without a pre-screen line counts
as doubtful. A later line for the same id replaces an earlier one.

## Queue, sample and acceptance rule

The queue holds every doubtful and wrong item plus a seeded random 15 % sample
of the confirmed items of each dataset. A dataset's sample is drawn only once
its pre-screen covers every item; until then the summary shows it as
"pre-screen incomplete" and nothing in it can be accepted by sample. If the
confirmed items change after the draw (a re-run pre-screen), the sample is
redrawn when none of its items has a decision yet; when some have,
`serve.py` and `apply.py` refuse to run and say how to start that sample over. `sample.json` records the seed
(default 164), the rate and the drawn ids; the sample is a prefix of a seeded
permutation of the confirmed ids, so it is reproducible.

Rule, per dataset: the confirmed items outside the sample are **accepted**
when the sample is fully decided and the upper bound of the Wilson 95 %
interval of its error rate is below 10 %. Unsure counts as an error. If the
bound is 10 % or more, the page widens the sample by another 15 % of the
confirmed items and they appear in the queue. With no errors the bound drops
below 10 % at 35 decided items, so a small dataset is in effect reviewed in
full. Widening after looking at the result makes this a sequential check: the
Wilson bound is a guide, not an exact 95 % guarantee. The summary page (S) shows per dataset reviewed/total, the sample's
error rate with its Wilson interval and the status.

## Comment directives (how N becomes a fix)

N without a comment (Esc in the comment box) removes a list entry (a
retrieval label, a CALLS edge, a gold symbol, a task answer). A pre-filled
`suggested_fix` is applied only if it is kept and saved with Enter; Esc
discards it. A comment line in one of these forms corrects instead:

| item kind | directive |
|---|---|
| retrieval label | `replace: path/file.py::Class.method — why` |
| retrieval "missing?" | `add: path/file.py::Class.method — why` (one line each) |
| graph CALLS edge | `replace: path/file::Symbol` (the right target; must be a gold symbol of the same file set, and the new edge must not exist yet) |
| held-out call site | `targets: path/file.py::name:LINE, …` or `targets: none` |
| task gold answer | `optional — why` (move it to the optional answers) |

Anything else in the comment is kept as the reason. Items that cannot be fixed
mechanically (a wrong trap or dynamic call, a missing task answer, a held-out
pair without `targets:`) are listed by `apply.py` for a fix by hand, and their
unit stays unreviewed.

## What apply.py writes

- `reviewed_by` per unit (retrieval query, graph-gold language file, held-out
  set, agent task): `human:marc` when every item was decided, `human:marc
  (sampled acceptance, <rule>)` when some confirmed items were accepted by the
  sample; `null` stays where an item is open, unsure or needs a hand fix. An
  existing `reviewed_by` is kept. (`human:<handle>` is the form the datasets'
  own validation and `real-repo-review` use.)
- corrected or removed labels, with a note in the query's or task's rationale;
- the protocol and its numbers in each dataset's README, between
  `<!-- label-review:start -->` and `<!-- label-review:end -->`;
- a list of baselines measured on the old labels. It re-records nothing. A
  changed `graph-gold/<language>.json` needs `graph-eval --update-baseline` in
  the same PR, or the Retrieval Quality gate fails.

### Owner policy `owner-2026-11-22`

```bash
python3 evals/review/apply.py --policy owner-2026-11-22 --dry-run
python3 evals/review/apply.py --policy owner-2026-11-22
```

Marc's policy for the items he did not decide himself:

- pre-screen-confirmed items outside the sample are accepted as correct on
  the strength of the sample he checked;
- every other item he did not decide, and every item he marked unsure, is
  decided by an independent second-check agent. Its decisions are in
  `agent-check/decisions-*.jsonl`, one JSON object per line:
  `{"id", "decision": correct|wrong|unsure, "reason", "evidence", "suggested_fix"?, "reviewer": "agent:second-check"}`.
  A record whose reviewer is not an `agent:` identity is refused. Marc's own
  correct or wrong decision always wins over an agent's.

Every item gets one provenance class. `provenance.jsonl` records it per item
as `{id, dataset, final, class, by}`:

| class | `by` | meaning |
|---|---|---|
| human | `human:marc` | Marc decided the item |
| prescreen | `agent:prescreen, accepted by marc (sample: N human-checked confirmed items, E errors)` | accepted by the policy; no person looked at it |
| second-check | `agent:second-check, per marc's policy` | decided by the second-check agent; no person looked at it |

A wrong decision is applied as before, from Marc's comment or from the agent's
`suggested_fix`. Fixes record their class in the rationale note. An item whose
final decision is unsure, or that is still undecided, is listed and keeps its
unit unreviewed.

A unit's `reviewed_by` names the classes of its items:

- `human:marc` only when Marc decided every item;
- the single class string when the items share one class;
- otherwise `mixed, policy owner-2026-11-22: <class> ×n; …`.

Retrieval queries are the exception. `real-repo-eval` reports any
`reviewed_by` as "reviewed by a human" and refuses agent reviewers, so a query
is marked only when Marc decided all of its items; otherwise its provenance
lives only in `provenance.jsonl`.

The policy needs a complete pre-screen and the sample it was drawn from. The
summary printed by `apply.py` and the protocol it writes into the dataset
READMEs count items per class.

Decisions are tied to the claim they were made on (`claim_digest`); a decision
on a claim that has since changed is ignored and the item is open again.

## Files

| file | committed |
|---|---|
| `export.py`, `serve.py`, `apply.py`, `common.py`, `static/`, `tests/` | yes |
| `prescreen/*.jsonl`, `sample.json`, `decisions.jsonl`, `agent-check/`, `provenance.jsonl` | yes, once written: they are the review record |
| `items.jsonl` | no (generated; contains third-party snippets) |

Tests: `python3 -m unittest discover -s evals/review/tests -v` (run by the
Agent Eval workflow).
