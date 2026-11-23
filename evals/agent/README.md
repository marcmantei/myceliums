# Agent benchmark

Does myceliums help a coding agent? This directory measures it directly: a
headless Claude Code agent answers code-understanding questions about pinned
open-source repositories, once with only its built-in read and search tools
and once with the myceliums MCP server added. Answers are graded
automatically against gold sets derived with grep, reading and compiler
tooling and verified by an agent, then checked in #164 under an owner policy
(see [Review of the labels](#review-of-the-labels-164)). A second, smaller eval checks
whether a model picks the right myceliums tool for a request.

Results: [`results/`](results/). Each run directory holds the compact per-session
results (`results.jsonl`), the run metadata (`meta.json`) and the generated
report (`REPORT.md`); raw transcripts stay in the git-ignored `runs/`.

**Pilot, 12 November 2026** (myc 0.3.2, Claude Code 2.1.322 with
`claude-sonnet-5-5` at medium effort, 180 graded sessions):
[report](results/2026-11-12-pilot/REPORT.md). Both conditions were at the
ceiling (success 100 % without, 99 % with myceliums; mean F1 0.998 vs 0.993,
difference −0.005 [−0.017, +0.004]; re-graded after the #164 label review,
which moved F1 by at most +0.002 and no success). The agent called a myceliums tool in 0
of 90 sessions that had the server; those sessions used 25 % more input
tokens [+17 %, +34 %] and took 20 % longer [+13 %, +28 %]. Tool selection:
[53 of 54 intents right in each of two passes](results/2026-11-12-toolsel/TOOLSEL.md)
(`claude-haiku-5-5`).

**Variant matrix, 20 November 2026** (#125; myc at `66611980a487` with the 9
core tools loaded up front, Claude Code 2.1.329, 116 graded sessions on 20
harder multi-hop tasks): [report](results/2026-11-20-variants/REPORT.md),
[method and summary](#variant-matrix-125). The agent called myceliums in 80 %
of sessions with the MCP server alone (A), 100 % with what
`myc setup-claude --no-agent` installs (C) and 97 % with the subagent added
(E), which it never used. All three were near the ceiling (success
98-100 %); accuracy, tokens and cost did not differ measurably between them.
Re-graded after the #164 label review: six sessions changed grade, A's mean
F1 rose from 0.984 to 0.985, and no success changed.

**Outcome eval, 21 November 2026** (#153; myc at `951d041ff9e0`, Claude
Code 2.1.330, 22 graded sessions on 11 multi-hop Django tasks, one repetition):
[report](results/2026-11-21-outcome/REPORT.md), [method and
summary](#outcome-eval-153). On static call-graph questions (callers three
levels up, blast radius, callers of one of several same-named functions)
with a budget of three rounds of tool calls (`--max-turns 4`, chosen in
calibration because the agent without myceliums failed there), the agent
without myceliums succeeded on 4 of 11 tasks (36 %), the agent with what
`myc setup-claude --no-agent` installs (C) on 11 of 11: success +64 pp [+36,
+91] (exact sign test over the 7 discordant tasks, all in C's favour:
p = 0.016), mean F1 +0.63 [+0.35, +0.90]. Every failure without myceliums
was a session that ran out of turns before answering; in calibration with up
to seven rounds (and an earlier, partly wrong budget text) it had solved 5
of 6 of these tasks, so the success gain is a budget effect. Under the budget
C also cost 35 % less [24 %, 46 %] and took half the wall time; these ratios
hold under the budget only. A pilot: n = 11 tasks, wide intervals, one
repository and model, no shell, static call-graph questions whose rules a call
graph computes by construction; #163 below measures the same comparison with
a shell, more turns and two repositories.

**Outcome eval at scale, 22 November 2026** (#163; myc at `951d041ff9e0`,
Claude Code 2.1.330, 180 graded sessions: 30 tasks over Django and Vue × 2
conditions × 3 turn budgets, one repetition): [report](results/2026-11-22-scale/REPORT.md),
[method and summary](#outcome-eval-at-scale-163). With `Bash` in both
conditions, none (no myceliums) against C (what `myc setup-claude
--no-agent` installs). Primary analysis: 27 tasks, without the three Django
`tests` tasks, which the reused Django index (no `tests/`) cannot answer.
Intervals and p-values are not corrected for the three budgets:

| Budget | Success none → C | Δ success [95 % CI], McNemar | Mean F1 none → C (Δ [95 % CI]) | Cost C ÷ none | Wall time C ÷ none |
|---|---:|---:|---:|---:|---:|
| 4 turns | 22 % → 37 % | +15 pp [−4, +33], 6 : 2, p = 0.29 | 0.222 → 0.485 (+0.263 [+0.078, +0.452]) | 1.27× [0.99×, 1.59×] | 0.96× [0.78×, 1.18×] |
| 10 turns | 85 % → 81 % | −4 pp [−15, +7], 1 : 2, p = 1 | 0.880 → 0.832 (−0.048 [−0.139, +0.017]) | 0.78× [0.63×, 0.93×] | 0.69× [0.55×, 0.83×] |
| 40 turns, not stated | 96 % → 89 % | −7 pp [−19, +0], 0 : 2, p = 0.5 | 0.971 → 0.930 (−0.042 [−0.088, −0.008]) | 0.98× [0.83×, 1.13×] | 0.84× [0.71×, 0.97×] |

With a shell and 10 or more turns, myceliums did not raise success: the agent
without it, working with grep pipelines in Bash, solved 85-96 % of the tasks,
and at 40 turns C was slightly lower (−7 pp, not significant; mean F1 −0.042).
Under 4 turns the success gain is not significant and mean F1 rose by 0.263.
C was faster from 10 turns on and cheaper at 10 turns. Over all 30 tasks:
4 turns 23 % → 40 %, 10 turns 87 % → 83 %, 40 turns 97 % → 87 %. The agent
called the MCP tools in 68 of 81 C sessions and never ran the `myc` CLI from
Bash.

**Re-run of C after #169, 22 November 2026** (#171; myc at `d1edc390e0c4`,
Claude Code 2.1.330, 60 graded sessions: condition C on the 30 tasks of #163
at 10 and 40 turns, one repetition, fresh indexes):
[report](results/2026-11-22-rerun-c/REPORT.md), [method and
summary](#re-run-of-c-after-169-171). Compared, paired by task, with the #163
sessions of none and of the old C. Primary analysis: the same 27 tasks, because
the Django index again has no `tests/` (#172). At 10 turns C solved 89 %
(none 85 %, old C 81 %; +4 pp [−7, +15], p = 1 against none); at 40 turns 89 %
(none 96 %, old C 89 %; −7 pp [−19, +0], p = 0.5). The arms ran about nine
hours apart.

**Token attribution, 23 November 2026** (#178, no new sessions):
[method and numbers](#token-attribution-and-cost-178). Read afterwards from
the #163 and #171 transcripts. The per-call usage adds up to the CLI's totals
in every session (a check that no model call is missing, not of the split);
a least-squares fit of each call's growth confirms that myceliums results and
grep output cost the same per character (0.446 and 0.428 tokens), so the
split treats them alike. With myceliums the agent read about half the grep
output (−9.0k tokens per session in #163) and about as much myceliums output
and hook text instead (+8.8k); C's sessions were larger because its bigger
system prompt is re-read on every call (+21.9k), but that is cached input, so
cost per solved task was 0.93× [0.78×, 1.10×] of none's. The harness now also
has a myceliums-only arm (D) with an allowlisted shell and a search audit,
edit tasks graded by a prechecked test command and changed files, and a
realistic profile (100-turn cap, 3 repetitions).

## What is measured

**Tasks** ([`tasks/tasks.json`](tasks/tasks.json)): 30 questions over three
pinned repositories ([`repos.json`](repos.json)), ten each:

| Repository | Language | Pinned at | Shape |
|---|---|---|---|
| fzf | Go | v0.75.0 | application, ~27k lines of Go outside tests, one 9k-line file |
| hono | TypeScript | v4.13.18 | library of many small modules, ~27k lines outside tests |
| FastAPI | Python | 0.162.5 | library (~23k lines) inside a repository dominated by tests and docs |

| Type | Tasks | Example |
|---|---:|---|
| callers | 8 | Which functions call the `AsString` method of `Item`? |
| callees | 4 | Which functions defined here does `solve_dependencies` call directly? |
| impact | 5 | If `IsTty` changed signature, which files must be updated? |
| where | 6 | Which function builds the `Set-Cookie` header value? |
| dead_code | 3 | Which functions in these files are never called? |
| flow | 4 | Which functions lie on the call chain from `Matcher.Loop` to `Pattern.MatchItem`? |

Every answer is a set of symbol names or file paths. The gold sets were
derived from the pinned checkouts with grep, by reading the code, and with
compiler tooling: Go call graphs and type-checked uses from
`golang.org/x/tools` (`go/packages`, `callgraph/cha`) across GOOS
darwin/linux/windows, the TypeScript language service (`findReferences`,
call hierarchy), and Python's `ast` module. **No gold item comes from
myceliums output** — that would make the eval circular. Each task carries a
rationale and, per gold item, evidence lines (`file`, `line`, expected text)
that `check-tasks` re-verifies, so a pin bump without relabelling fails
loudly. Items an agent may defensibly add (thin wrappers, recursion) are listed
as `optional` and are neutral in grading.

The labels were written and verified by an AI agent (Claude), then checked in
#164 under Marc's owner policy. Marc decided some items himself; the others
were accepted on the strength of his sample or decided by a second-check
agent. Counts per class are under
[Review of the labels](#review-of-the-labels-164), and each task's
`reviewed_by` names its classes.

**Grading** ([`aeval/grading.py`](aeval/grading.py)): the agent must end with
a fenced JSON block `{"answer": [...]}`. Items are normalised (paths made
repository-relative; `(*T).m`, `T.m`, `pkg.T.m()` and `file.go:12 T.m` all name
`T.m`) and matched one-to-one against the gold set by maximum bipartite
matching. Precision, recall and F1 follow; a session succeeds when F1 ≥ 0.8.
A bare method name (`m` for `T.m`) is accepted, which is lenient in the same
way for both conditions. A missing or unparseable answer scores 0.

**Per session, from the CLI's stream-json output:** answer, F1, tool calls by
tool, turns, input tokens (uncached + cache writes + cache reads), output
tokens, cost as estimated by the CLI, API time and wall time.

**Report** ([`aeval/report.py`](aeval/report.py)): repetitions are averaged
within a task first, so a task is the unit of analysis; 95 % paired bootstrap
intervals (10,000 resamples over tasks) for each condition and for the
difference, overall and per task type.

## The two conditions

Both conditions run `claude -p` with the same prompt
([`aeval/runner.py`](aeval/runner.py)), the same model, effort and turn limit,
and the same built-in tools: `Read`, `Grep`, `Glob` and `ToolSearch` (no
shell, no edits, no web). The prompt never mentions myceliums.

* **without** — `--mcp-config '{"mcpServers": {}}' --strict-mcp-config`.
* **with** — the same plus one MCP server, `myc mcp`, built from this
  repository, and the permission rule `mcp__myceliums` so its tools run without
  a prompt. Claude Code defers MCP tool definitions by default: the agent sees
  the 54 tool names and the server's instructions, and loads a tool's schema
  through `ToolSearch` before calling it. That is how a Claude Code user meets
  myceliums today.

**Isolation.** Each session starts in a fresh copy of the pinned checkout at a
fixed path (`$AEVAL_WORK_ROOT/<repo>`, default `/tmp/aeval/<repo>`); the
myceliums index is keyed by path, so the prebuilt index can be reused there.
For *with*, a fresh copy of the prebuilt index goes to a per-session temp
directory and the server runs with `MYCELIUMS_DATA_DIR` and `HOME` pointing
into it — the user's `~/.myceliums` is never read or written. The session
environment is rebuilt from a short allow-list (no inherited agent variables);
user, project and local settings are not loaded (`--setting-sources ""`),
CLAUDE.md and auto-memory are disabled, slash commands and skills are off,
claude.ai connectors are off, and sessions are not persisted. The runner
checks every transcript's `init` event — exact built-in tool set, exactly one
connected `myceliums` server in *with*, none in *without*, no skills or slash
commands — and stops the whole run on the first violation.

**Index.** `agent_eval.py index` runs `myc analyze <path> --strict-embeddings`
once per repository (default configuration, embeddings included, i.e. the
README's quick start), with one exception: for FastAPI, `docs/` (1,709
Markdown files in 13 languages) is excluded through an index-time
`.myceliums.toml` (`index_exclude_extra` in `repos.json`). With it, analysis
produced 23,941 symbols and community detection had not finished after 13
minutes on the benchmark machine, before any embedding. The session
checkouts never contain that file, and the agent can still read `docs/`.
Each *with* session refreshes the copied index's `analyzed_at`, so a
cache-aware `analyze` call by the agent is a cache hit rather than a
re-index.

**Toolset.** The *with* condition runs `myc mcp` with its default toolset.
The pilot (myc 0.3.2) offered all 55 tools, deferred behind tool search. Since
#97 the default is the 9 task-named core tools, marked to load up front
(`_meta["anthropic/alwaysLoad"]`); runs of different versions are therefore
not comparable without saying so. The variant matrix below (#125) runs that
toolset.

**Not included.** `myc setup-claude` also installs a SessionStart hook that
re-indexes the project and prints a status line into the session, and a
PostToolUse hook that formats myceliums results. The *with* condition adds
only the MCP server, so neither hook is measured here.

## Running it

Python 3.10+ standard library only; it is not part of `cargo test`. You need
the `claude` CLI logged in, `git`, and a release build of `myc`.

```bash
cargo build --release -p myc
mkdir -p ~/.cache/myceliums-agent-eval/bin
cp target/release/myc ~/.cache/myceliums-agent-eval/bin/myc

python3 evals/agent/agent_eval.py fetch          # shallow-fetch the pinned commits
python3 evals/agent/agent_eval.py index          # one index per repo (slow: embeddings on CPU)
python3 evals/agent/agent_eval.py check-tasks    # schema + every evidence line
python3 evals/agent/agent_eval.py run --label my-run            # 30 tasks x 2 conditions x 3 reps
python3 evals/agent/agent_eval.py export --label my-run --myc-commit "$(git rev-parse --short=12 HEAD)"
python3 evals/agent/agent_eval.py report my-run  # writes results/my-run/REPORT.md

python3 evals/agent/agent_eval.py toolsel-run --label my-toolsel   # 56 intents x 2 passes, haiku, all tools
python3 evals/agent/agent_eval.py toolsel-run --label my-core --toolsets core   # 19 intents, core tools
python3 evals/agent/agent_eval.py toolsel-report my-toolsel
```

`run` is resumable: rerun the same command and finished sessions are skipped.
It refuses to resume with a different model, effort, limit or seed. Useful
flags: `--task ID` and `--rep N` to run a subset, `--max-sessions N` as a hard
cap, `--concurrency` (default 3; sessions of the same repository never
overlap), `--timeout` (default 900 s per session). Transient API errors
(429/5xx/overload, crashes) are retried twice with back-off; timeouts and
turn-limit hits are graded as they are. `--max-utilization` (default 0.97)
stops scheduling when the account's reported rate-limit utilisation reaches
the threshold, so a pilot cannot exhaust a shared subscription. The run order
is repetition-major, so a run stopped early still holds complete, paired
repetitions.

Each session costs real usage on the logged-in account: a full run is 180
agent sessions; the tool-selection eval is 108 single-turn sessions on a
small model. Environment variables: `AEVAL_CACHE` (default
`~/.cache/myceliums-agent-eval`), `AEVAL_WORK_ROOT` (default `/tmp/aeval`),
`FASTEMBED_CACHE_DIR` (embedding model cache for indexing and the server).

### No CI job runs the agents

Running the agents needs Claude credentials, so no workflow does it. The
[`agent-eval` workflow](../../.github/workflows/agent-eval.yml) runs only the
secret-free parts on pull requests: the harness's unit tests, the task-file
schema check, and a check that every committed `REPORT.md` / `TOOLSEL.md` /
`TOKENS.md` is exactly what the generator produces from the committed results.

## Variant matrix (#125)

The pilot's tasks were too easy and its agent never called myceliums. The
variant matrix asks the follow-up questions: on harder, multi-hop tasks, which
integration layer of `myc setup-claude` makes the agent use myceliums, and
does that change accuracy, tokens, cost or time?

**Tasks.** Two hard sets of 20 tasks each over the same pinned checkouts, four
per type:

| Type | Question |
|---|---|
| `transitive_callers` | every caller up to N levels up the call chain |
| `blast_radius` | the files holding a call to X or to a function up to N levels above it |
| `ambiguous` | callers (up to N levels) of one of several same-named symbols (`Chars.Length` vs `Merger.Length`, `Context.header` vs `HonoRequest.header`, Starlette's vs FastAPI's `HTTPException`) |
| `tests` | the tests (fzf: `Test…` functions; hono: test files) that reach X through a chain of direct calls of any length |
| `flow` | every function on at least one call chain from A to B |

Each question defines "call" precisely (direct calls of named functions;
closures count for their enclosing function; calls through interfaces,
function values or other objects' attributes do not). The gold sets were
computed by [`groundtruth/hard_tasks.py`](groundtruth/hard_tasks.py) from
call graphs built with compiler tooling, never from myceliums:
`gotool edges` (go/packages + SSA, every function including closures, union
of GOOS darwin/linux/windows), [`ts_edges.js`](groundtruth/ts_edges.js) (the
TypeScript type checker resolves each call through imports, re-exports and
receiver types) and [`py_edges.py`](groundtruth/py_edges.py) (import-aware
AST resolution; every Python gold set was also checked against `py_refs.py`
and grep for missed callers). Every gold item carries one evidence line that
`check-tasks` re-verifies, and a note with its level or next hop.

* [`tasks/tasks-hard-v1.json`](tasks/tasks-hard-v1.json): the first set
  (8 to 18 items per task). The calibration run
  ([report](results/2026-11-20-calibration/REPORT.md)) gave ten of its tasks,
  two per type, to the agent without myceliums: all ten scored F1 = 1.00, in
  22 s and 9 tool calls on average. The set was at the ceiling.
* [`tasks/tasks-hard.json`](tasks/tasks-hard.json): the set the matrix ran on,
  the same question types one to two levels deeper or wider (9 to 44 items per
  task). The calibration allowance (10 sessions) was spent on v1, so this set
  was not calibrated before the matrix.

**Conditions.** All of them run the same prompt, model, effort, limits and
built-in tools as the pilot, but load project settings
(`--setting-sources project`) and CLAUDE.md files, so what `myc setup-claude`
writes into the working copy takes effect:

| Condition | Session |
|---|---|
| `none` | no MCP server (calibration only) |
| `A` | the myceliums MCP server, default core toolset (9 tools with `_meta["anthropic/alwaysLoad"]`) |
| `C` | A + `myc setup-claude --scope project --no-agent` run in the working copy: routing block (in `CLAUDE.md`; since #200 in `.claude/rules/myceliums.md`); SessionStart, PostToolUse and PreToolUse grep-redirect hooks in `.claude/settings.json` |
| `E` | C + the `code-explorer` subagent (`myc setup-claude --scope project`) and the `Agent` tool; Claude Code's built-in subagents are denied (`--disallowedTools Agent(Explore),…`), so E adds exactly that one subagent |

B (routing only) and D (C without the core tools loaded up front) were left
out to fit the session budget. The MCP server and every hook reach the
per-session index copy through a `myc` wrapper first on the session's PATH,
which sets `MYCELIUMS_DATA_DIR` and `HOME` and logs each hook's input,
output and reason for silence (`MYC_HOOK_DEBUG`) next to the transcript; the
agent sees only what Claude Code passes on. `--include-hook-events` puts the
hook runs into the transcript too.

**Checking a setup without spending usage.** `agent_eval.py probe` starts a
session exactly like `run` does, but against [`aeval/mockapi.py`](aeval/mockapi.py),
a local stand-in for the Messages API with a fixed script (a Grep call, a
delegation to `code-explorer`, a `find_callers` call by the subagent) and a
dummy API key, so no model is called. It records the request bodies (never
the headers) and reports what the session was given. For 2.1.329 it showed:
the project `CLAUDE.md` (routing block, before #200) reaches the context in C and E and
`~/.claude/CLAUDE.md` in no condition; the 9 core tools are sent with full
schemas in A, C and E (none deferred); the grep hook's hint reaches the
context; the SessionStart hook's `systemMessage` does not (it is shown to the
user, not the model); in E the Agent tool offers `code-explorer` only, and the
subagent gets the 9 myceliums tools and `Read`. The init event names the
Agent tool `Task`.

```bash
export AEVAL_CACHE=~/.cache/myceliums-agent-eval-125 AEVAL_WORK_ROOT=/private/tmp/aeval125
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-hard.json check-tasks
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-hard.json probe --label p1 \
    --task fzf-tc-tochars-d4 --conditions A,C,E --grep ToChars --callers ToChars
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-hard.json run --label my-matrix \
    --conditions A,C,E --reps 2 --max-retries 0 --max-sessions 120
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-hard.json export --label my-matrix
python3 evals/agent/agent_eval.py report my-matrix
```

**Which instruction files reach Claude Code (#200).**
[`probe_instructions.py`](probe_instructions.py) starts one session per layout of
`AGENTS.md`/`CLAUDE.md` files (14) and setup state against the same mock API, in a
temporary HOME, and reports which of the texts the requests contain:

```bash
python3 evals/agent/probe_instructions.py --myc target/debug/myc
```

`legacy` emulates the routing block in `CLAUDE.md` as setup wrote it before #200;
the script fails when `setup`, `repair` (setup over `legacy`) or `user` loses a text
that the project alone delivers, or does not deliver the routing block once.

**Results, 20 November 2026** (myc 0.3.2 at `66611980a487`, Claude Code
2.1.329, `claude-sonnet-5-5`, `--effort medium`; 116 graded sessions over the
20 v2 tasks; [report](results/2026-11-20-variants/REPORT.md)). Means over
tasks, brackets 95 % paired bootstrap intervals against A:

| Per session | A | C | E |
|---|---:|---:|---:|
| Sessions with ≥ 1 myceliums call | 80 % (32 of 40) | 100 % (38 of 38) | 97 % (37 of 38) |
| … difference to A | | +20 pp [+5, +38] | +18 pp [+5, +32] |
| First tool called (most common) | `Grep` (27 of 40) | `find_callers` (28 of 38) | `find_callers` (28 of 38) |
| Success (F1 ≥ 0.8) | 100 % | 98 % | 100 % |
| Mean F1 (difference to A) | 0.985 | 0.983 (−0.001 [−0.027, +0.022]) | 0.989 (+0.005 [−0.010, +0.020]) |
| Input tokens (ratio to A) | 166,868 | 159,638 (0.96× [0.74×, 1.24×]) | 160,217 (0.96× [0.75×, 1.24×]) |
| Prompt of the first model call | 10,162 | +348 | +2,212 |
| Cost, USD (ratio to A) | 0.147 | 0.144 (0.98× [0.82×, 1.18×]) | 0.136 (0.93× [0.78×, 1.13×]) |
| Tool calls (difference to A) | 11.2 | 9.3 (−1.9 [−4.4, +0.8]) | 8.6 (−2.5 [−4.9, −0.3]) |
| Wall time, s (difference to A) | 35.3 | 49.2 (+13.8 [−4.6, +46.2]) | 34.1 (−1.2 [−6.0, +3.8]) |

* **With the core tools loaded up front, the agent uses myceliums.** Even A,
  the MCP server alone, called it in 80 % of sessions, against 0 of 90 in the
  pilot (54 deferred tools, easier tasks, Claude Code 2.1.322). The matrix
  has no deferred-tools control (variant D), so it does not isolate how much
  of that jump is #97's doing. The `setup-claude` layers (C) raised the rate
  to 100 % and made a myceliums tool, not Grep, the first call.
* **No accuracy effect on these tasks.** All three variants were at or near
  the ceiling (success 98-100 %); the F1 differences are within ±0.03. By
  type, `tests` tasks gained F1 in C (+0.047 [+0.009, +0.098]) and E (+0.034
  [+0.005, +0.080]) and `flow` tasks lost a little in C (−0.018 [−0.034,
  −0.004]); with four tasks per type, read these as direction only.
* **Cost and tokens did not change measurably.** Input tokens and cost were 4
  and 2-7 % lower in C and E, with intervals spanning ±25 %. E used 2.5 fewer
  tool calls (−23 %). The routing block adds 348 tokens to every request, the
  Agent tool with the subagent 2,212.
* **The subagent was never used.** In 0 of 38 E sessions did the main agent
  delegate to `code-explorer`; it called the same myceliums tools itself.
* **The grep hook fired as designed.** In C it ran on 175 Grep calls and added
  a hint to 120 (69 %); the rest were not identifier-shaped or not in the
  index. The hint names CLI commands (`myc callers X`), which these sessions
  could not run (no shell).
* **A side effect of setup-claude.** The files `myc setup-claude` writes
  after `myc analyze` are uncommitted and newer than the index, so the MCP
  `analyze` tool's freshness check calls the index stale and re-indexes when
  the agent calls it: once it timed out after 300 s (hono, C; the +13.8 s
  mean wall time of C is mostly that session), once it re-indexed without
  embeddings (hono, E). The SessionStart hook's commit-based check reported
  the same index as ready.

Session accounting: 10 calibration sessions; 123 matrix launches, of which
116 are graded and 7 were cut off without a result (3 when the benchmark
machine went to sleep, 4 when the run was stopped externally); 4 planned
repetition-2 sessions of two hono tasks were not run to stay near the budget
of 120. Every task has at least one repetition in every variant. Three
further sessions ran against the mock API (`probe`, no model call).

`touch runs/<label>/STOP` ends a run cleanly (running sessions finish and
are recorded). The report compares every variant with A by a paired
bootstrap over tasks: myceliums tool-call rate (sessions with at least one
call, calls per session, calls by the subagent), success, F1, tokens, cost,
wall time, per task type and per task, and what the hooks did.

## Outcome eval (#153)

The pilot and the variant matrix measured usage well and outcomes hardly at
all: every condition was at the ceiling. The outcome eval asks whether
myceliums changes the result, on tasks and under a budget where the agent
without it is below the ceiling.

**Repository.** Django 5.1.4 (`2d4add11fd57`, the commit the annotated tag
`5.1.4` points to; pinned in [`repos.json`](repos.json) next to the pilot's
three), a framework with many same-named functions and methods
across its apps. Only the `django/` package is indexed (12,443 symbols, 828
files, every symbol embedded with the default `jina-embeddings-v2-base-code`,
template 2, chunking; `myc analyze --strict-embeddings` took 1,447 s). With
`tests/` and `docs/` included, analysis produced 41,842 symbols and its
community detection had not finished after 9 minutes. The index was built once
and copied fresh for every session; the session checkout is the whole
repository, and every question says to consider `django/` only.

**Tasks** ([`tasks/tasks-outcome.json`](tasks/tasks-outcome.json)), four per
type, chosen from the call graph by their shape before any myceliums output
was looked at:

| Type | Question | Gold items |
|---|---|---:|
| `transitive_callers` | every caller up to three levels up (`get_permission_codename`, `get_format`, `safe_join`, `resolve_url`) | 25-36 |
| `blast_radius` | the files holding a call to X or to a direct caller of X (`iri_to_uri`, `get_current_timezone`, the `TemplateDoesNotExist` constructor, `get_script_prefix`) | 16-24 files |
| `ambiguous` | callers two or three levels up of one of several same-named functions, the others named (`get_connection` in `db/transaction.py`, not the mail one; `resolve_relation`, `resolve`, `load_backend`) | 16-23 |

The gold sets come from
[`groundtruth/outcome_tasks.py`](groundtruth/outcome_tasks.py) over the call
graph of [`py_edges.py`](groundtruth/py_edges.py) with `--local-imports
--star-imports` (Python's `ast` module: calls by name through imports,
function-local imports and `from x import *` re-exports, `self`/`cls`/`super()`
through the enclosing class and its bases, instantiation by name), never from
myceliums. Each question spells out these rules. `outcome_tasks.py --audit`
lists every AST call site of every name on each chain that the graph does not
link: 39 sites, all calls through `self` or `super()` that the enclosing class
neither defines nor inherits (subclass dispatch, which the rule excludes),
recursion, or another class's method of the same name. Every gold item has an
evidence line that `check-tasks` re-verifies.

**Conditions.** `none`: no myceliums at all (no MCP server, no CLAUDE.md, no
hooks), the built-in `Read`, `Grep` and `Glob` only — no Bash or other shell,
in either condition (the harness also allows `ToolSearch`, which the CLI drops
when there is no MCP server). `C`: the MCP server with its
9 core tools loaded up front plus what `myc setup-claude --scope project
--no-agent` writes into the working copy (routing block, SessionStart hook,
which since #152 puts a context line into the model's context, PostToolUse
formatter, PreToolUse grep hook). Everything else as in the variant matrix:
same prompt, model (`claude-sonnet-5-5`), `--effort medium`, isolation and
transcript checks. `probe` (mock API, no model call) confirmed that C's
first request carries the 9 tool schemas, the routing block and the hook's
context line (`index: 2d4add1 = HEAD`, so no session re-indexed), and that
`none` carries none of them.

**Budget.** `--max-turns 4` in both conditions, and `run --prompt-budget`
states it in the prompt. The CLI counts the agent's replies: a reply with
tool calls is one turn however many calls run in parallel in it, and the
session ends right after the fourth such reply, so three rounds of tool calls
fit before the final answer (checked with `probe`: `--max-turns 1` ends after
the first tool round, `2` lets the final text through). `num_turns` in the
result counts tool calls + 1 and is not what the limit compares.

**Calibration** (10 sessions, `none`, on 9 of the 12 tasks): at
`--max-turns 8`, 5 of 6 tasks succeeded
([report](results/2026-11-21-outcome-calibration/REPORT.md); the agent used
two to five rounds of tool calls), which is the ceiling. That run's prompt
still said that parallel calls count one turn each, which the probe later
disproved, so it is not the main run's prompt. At `--max-turns 4` with the
corrected text, 1 of 4 succeeded
([report](results/2026-11-21-outcome-calibration-t4/REPORT.md)). The main run
uses 4: the budget was chosen because the agent without myceliums failed at
it on three calibration tasks that are also in the main run, which is what
the issue asked for (a baseline below the ceiling) but also means the
baseline's success rate is partly selected.

**Tasks frozen before any myceliums output.** The task file
(`tasks-outcome.json`, sha256 `3a1b3848ce66…`) was committed in `9619d24`
at 20:56:11 UTC and has not changed since: all three result directories
record that sha256, and the questions in all 32 graded session prompts,
from the first calibration session (20:47:13 UTC), are the committed ones.
The Django index finished at 20:59:17 UTC; the first session with myceliums
(a mock-API probe) started at 20:59:41 UTC and the main run at 21:00:18
UTC. Before that, myc had only been run to count symbols (a graph-only
`analyze` and `myc stats`), never to list callers.

**Results, 21 November 2026** (myc 0.3.2 at `951d041ff9e0`, Claude Code
2.1.330; 22 graded sessions, 11 tasks × 2 conditions × 1 repetition;
[report](results/2026-11-21-outcome/REPORT.md)). Means over tasks, brackets
95 % paired bootstrap intervals over the 11 tasks. Success, as an exact
sign (McNemar) test: 7 tasks are discordant, all 7 failed without myceliums
and succeeded with it, none the other way; two-sided p = 2 × 0.5⁷ = 0.016.

| Per session | none | C | C − none (C ÷ none) |
|---|---:|---:|---:|
| Sessions with ≥ 1 myceliums call | 0 % | 100 % (11 of 11) | |
| Success (F1 ≥ 0.8) | 36 % (4 of 11) | 100 % (11 of 11) | +64 pp [+36, +91] |
| Mean F1 | 0.362 | 0.988 | +0.626 [+0.352, +0.897] |
| Input tokens | 36,557 | 34,127 | 0.93× [0.74×, 1.17×] |
| Output tokens | 3,198 | 1,514 | 0.47× [0.37×, 0.58×] |
| Cost, USD (CLI estimate) | 0.076 | 0.049 | 0.65× [0.54×, 0.76×] |
| Tool calls | 7.4 | 2.5 | −4.8 [−6.3, −3.3] |
| Wall time, s | 24.0 | 12.6 | 0.52× [0.43×, 0.62×] |
| Prompt of the first model call | 5,684 | 11,102 | +5,418 |

| Type | Tasks | Success none | Success C | Mean F1 none | Mean F1 C |
|---|---:|---:|---:|---:|---:|
| transitive callers | 3 | 0 of 3 | 3 of 3 | 0.000 | 0.995 |
| blast radius | 4 | 4 of 4 | 4 of 4 | 0.995 | 0.994 |
| ambiguous | 4 | 0 of 4 | 4 of 4 | 0.000 | 0.977 |

* **Under a tight budget, myceliums changed the outcome.** In C the agent
  called `find_callers` once per session, with `depth` 2 or 3, and scored F1
  0.94-1.00; in 8 of 11 sessions that first reply (often with one Grep next to
  it) was its only round of tool calls. Without myceliums the agent started a
  fourth round of greps and reads in every transitive-caller and ambiguous
  task, overran the stated budget and ended without an answer (F1 = 0) in all
  seven. No session in either condition failed with a wrong answer.
* **The success gain depends on the budget.** In calibration with up to
  seven rounds (a different, partly wrong budget text) the agent without
  myceliums solved 5 of 6 of these tasks; myceliums gets the answer in one
  call instead of four or five rounds, and the budget turns that into
  success or failure. C never ran without a budget, so whether myceliums
  saves time or cost there is not measured. The only comparison of two
  successful sessions is descriptive: on the four blast-radius tasks both
  conditions solved, C used 2.25 tool calls instead of 4.25, 12.5 s instead
  of 21.8 s and 0.047 USD instead of 0.069 (n = 4, still under the budget).
* **Under the budget, cost fell and input tokens did not change measurably.**
  C's first request is 5,418 tokens larger (the tool schemas, the routing
  block, the context line), which the shorter sessions offset: input tokens
  0.93× [0.74×, 1.17×], output tokens and cost clearly lower. These ratios
  hold only under the budget: the failed sessions without myceliums were cut
  off at the limit, which also caps their cost and time.

Session accounting: 10 calibration sessions; 23 of the 24 approved main-run
sessions launched, 22 graded. One C session (`django-amb-transaction-get-connection`)
was killed with its run chunk by an external stop after its first tool call
and run again; `django-tc-resolve-url` was therefore not run, since its pair
would have exceeded the budget. Probes against the mock API used no model.

```bash
export AEVAL_CACHE=~/.cache/myceliums-agent-eval-153 AEVAL_WORK_ROOT=/private/tmp/aeval153
python3 evals/agent/agent_eval.py fetch --repo django
python3 evals/agent/agent_eval.py index --repo django      # ~25 min on CPU: embeds every symbol
python3 -I evals/agent/groundtruth/py_edges.py --local-imports --star-imports \
    $AEVAL_CACHE/repos/django django > $AEVAL_CACHE/edges/django/edges.tsv
python3 -I evals/agent/groundtruth/outcome_tasks.py $AEVAL_CACHE/edges $AEVAL_CACHE/repos \
    > evals/agent/tasks/tasks-outcome.json                 # --audit: the grep audit
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-outcome.json check-tasks
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-outcome.json run --label my-outcome \
    --conditions none,C --reps 1 --max-turns 4 --prompt-budget --max-retries 0 --max-sessions 24
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-outcome.json export --label my-outcome
python3 evals/agent/agent_eval.py report my-outcome
```

## Outcome eval at scale (#163)

The #153 pilot showed an outcome effect under one tight budget, on one
repository, without a shell, and left open what happens with more turns. #163
asks: does myceliums change outcomes when the agent has a shell and when the
budget is not tight, and how does the effect depend on the budget?

**Repositories.** Django 5.1.4 (as in #153) and Vue 3.5.46 (`vuejs/core` at
`7de9e41b61b2`, the commit of the tag `v3.5.46`; MIT), a TypeScript monorepo
of 13 packages with their tests in `packages/*/__tests__`. The Django index is
the one #153 built (the same binary, so the same embedder fingerprint; it
loads and reports 12,443 symbols, all embedded). That index covers only the
`django/` package, not `tests/` (`index_exclude_extra` in its `meta.json`), so
in C myceliums had no data for the three Django `tests` tasks: they are left
out of the primary analysis (27 tasks), and the 30-task numbers are given
alongside. The Vue index covers
`packages/` with the tests and without `packages-private/` (playgrounds and
type-only tests) and the changelogs: 15,750 symbols in 501 files, every one
embedded (`myc analyze --strict-embeddings`, 1,163 s).

**Tasks** ([`tasks/tasks-scale.json`](tasks/tasks-scale.json); sha256
`fd0f92ee8148…` at the freeze): 30, 15 per repository. After the run, the #163
review made one file optional in `vue-blast-to-raw` (`reactive.ts`, whose only
call is `toRaw` calling itself; recursion is optional under the owner policy
of #164/#168), and `agent_eval.py regrade` re-graded the stored answers: four
sessions, both conditions, went from F1 0.97 to 1.00, no success changed. The
labels are drafted by an agent and not reviewed: the #164 review tool exports
this set (`agent-tasks-scale`), but it has no pre-screen, and `apply.py`
leaves it out.

| Type | Tasks | Answer |
|---|---:|---|
| `transitive_callers` | 6 | every caller two or three levels up (21 to 36 items) |
| `blast_radius` | 4 | the files holding a call to X or to a direct caller of X (18 to 26 files) |
| `ambiguous` | 6 | callers of one of several same-named functions (Django `make_style`, `get_template`, `format`; Vue reactivity's `warn` and `watch`, compiler-core's `warnDeprecation`) |
| `flow` | 4 | every function on a call chain from A to B (10 to 27) |
| `tests` | 6 | the test files that reach X (Django: directly or through one function in `django/`; Vue: through chains of any length) |
| `config` | 4 | not about calls: Django's form widget classes (a class hierarchy across six files) and the settings Django reads that have no default; the Vue files that read `app.config` options and the interfaces extending the template compiler's AST `Node` |

The gold sets come from [`groundtruth/scale_tasks.py`](groundtruth/scale_tasks.py)
over Python's `ast` module ([`py_edges.py`](groundtruth/py_edges.py) as in
#153, for the Django test questions over `django/` and every test app) and the
TypeScript 6.0 checker ([`vue_edges.js`](groundtruth/vue_edges.js), the #125
extraction with Vue's `@vue/*` path mapping; [`vue_config.js`](groundtruth/vue_config.js);
[`vue_hierarchy.js`](groundtruth/vue_hierarchy.js)), never from myceliums.
Each question spells out its rules; each gold item has an evidence line that
`check-tasks` re-verifies. `scale_tasks.py build --calibration` reproduces the
candidate file the calibration ran on byte for byte.

**Conditions.** Both have `Bash` next to `Read`, `Grep` and `Glob`
(`run --bash`); the session's PATH is the system's, so the shell has
`python3`, `git` and `grep` but not `node` or `rg`. `none`: nothing of
myceliums, and `MYCELIUMS_DATA_DIR` points to an empty directory in case the
agent finds a `myc` binary on disk (none did). `C`: the MCP server with its 9
core tools loaded up front, what `myc setup-claude --scope project --no-agent`
writes (routing block, SessionStart context line, PostToolUse formatter, the
grep hook, which also fires on `grep`/`rg` in Bash), and `myc` first on PATH,
bound to the session's index copy, so the agent can also run the CLI from
Bash. A myceliums call is either an MCP tool call or a Bash call that runs
`myc`. `probe` (mock API, no model) confirmed for both repositories: in C the
9 tool schemas, the routing block, the context line (`index: <commit> =
HEAD`), the grep hint on a Grep and on a Bash `grep`, and `myc callers` from
Bash answering from the session's index; in none no `myc` and none of the
rest. Prompt, model (`claude-sonnet-5-5`), `--effort medium` and isolation as
in #153.

**Budgets.** `--max-turns` 4 and 10, stated in the prompt with #153's text,
and 40 as the unlimited arm: a cap the prompt does not mention, so the agent
works as it would without a budget; no session reached it. Timeouts 300, 360
and 480 s; no session timed out. One run per budget
(`2026-11-22-scale-t4`, `-t10`, `-t40`), merged by `export-scale`.

**Calibration** (15 sessions, condition none, 10 turns, on 15 of the 35
candidates; [report](results/2026-11-22-scale-calibration/REPORT.md)). With
a shell the agent combined grep pipelines (once a Python `ast` script) and
solved most call-graph candidates. All five candidates that are not about
calls were solved in one or two Bash calls (5 to 11 s) and rejected as
trivially solved; none was impossible. The four `config` tasks in the set
replaced them afterwards and are not calibrated. Of the ten calibrated tasks
in the set, eight succeeded (target 30-80 %); the other 20 tasks are not
calibrated.

**Tasks frozen before any myceliums output.** The task file was committed at
10:04:05 UTC (the freeze commit's author date; first pushed as `9c53600`, rebased
onto main without changes to the file), after the calibration (09:54-09:59 UTC, condition
none only). The Vue index was built afterwards (indexed 10:24:08 UTC), the
reused Django index was first opened after the freeze, the first session with
myceliums (a mock-API probe) started at 10:24:52 UTC and the main run at
10:25:18 UTC.

**Results, 22 November 2026** (myc 0.3.2 at `951d041ff9e0`, Claude Code
2.1.330; 180 graded sessions; [report](results/2026-11-22-scale/REPORT.md)).
Primary analysis: means over the 27 tasks without the three Django `tests`
tasks. Brackets: 95 % paired bootstrap intervals over tasks; per budget,
success is compared with McNemar's exact test over the tasks that succeeded in
one condition only. **No interval or p-value here is corrected for multiple
comparisons** (three budgets, the pooled comparison, several metrics,
repositories and task types).

| Per session | 4 turns: none | C | 10 turns: none | C | 40 turns: none | C |
|---|---:|---:|---:|---:|---:|---:|
| Success (F1 ≥ 0.8) | 22 % | 37 % | 85 % | 81 % | 96 % | 89 % |
| … C − none | | +15 pp [−4, +33] | | −4 pp [−15, +7] | | −7 pp [−19, +0] |
| … McNemar (C better : none better) | | 6 : 2, p = 0.29 | | 1 : 2, p = 1 | | 0 : 2, p = 0.5 |
| Mean F1 | 0.222 | 0.485 | 0.880 | 0.832 | 0.971 | 0.930 |
| … C − none | | +0.263 [+0.078, +0.452] | | −0.048 [−0.139, +0.017] | | −0.042 [−0.088, −0.008] |
| Sessions with ≥ 1 myceliums call | 0 % | 81 % | 0 % | 85 % | 0 % | 85 % |
| Input tokens (C ÷ none) | 33,443 | 1.51× [1.32×, 1.70×] | 83,926 | 1.09× [0.88×, 1.29×] | 100,535 | 1.31× [1.13×, 1.52×] |
| Output tokens (C ÷ none) | 1,354 | 1.00× [0.74×, 1.31×] | 3,745 | 0.64× [0.50×, 0.80×] | 4,147 | 0.87× [0.71×, 1.02×] |
| Cost, USD, CLI estimate (C ÷ none) | 0.036 | 1.27× [0.99×, 1.59×] | 0.091 | 0.78× [0.63×, 0.93×] | 0.102 | 0.98× [0.83×, 1.13×] |
| Wall time, s (C ÷ none) | 13.4 | 0.96× [0.78×, 1.18×] | 30.3 | 0.69× [0.55×, 0.83×] | 35.4 | 0.84× [0.71×, 0.97×] |

Over all budgets (each task averaged over its three budgets; an exact sign test
on those per-task differences, not McNemar): success 68 % → 69 % (+1 pp [−7,
+10], 6 : 4, p = 0.75), F1 +0.058 [−0.012, +0.126], cost 0.95× [0.82×, 1.07×],
wall time 0.80× [0.70×, 0.90×].

All 30 tasks, the three Django `tests` tasks included: success 23 % → 40 %
(+17 pp [−3, +37], 7 : 2, p = 0.18) at 4 turns, 87 % → 83 % (−3 pp [−17, +7])
at 10, 97 % → 87 % (−10 pp [−23, +0], 0 : 3, p = 0.25) at 40; mean F1 +0.268
[+0.093, +0.445], −0.044 [−0.128, +0.013] and −0.049 [−0.096, −0.013].

Exploratory, primary analysis (few tasks per group; uncorrected):

| | 4 turns | 10 turns | 40 turns | All budgets: Δ success [95 % CI] |
|---|---:|---:|---:|---:|
| Django (12 tasks) | 25 % → 58 % | 92 % → 83 % | 100 % → 92 % | +6 pp [−14, +22] |
| Vue (15 tasks) | 20 % → 20 % | 80 % → 80 % | 93 % → 87 % | −2 pp [−9, +4] |
| ambiguous (6) | 0 % → 50 % | 83 % → 100 % | 100 % → 83 % | +17 pp [+6, +28] |
| transitive callers (6) | 0 % → 33 % | 100 % → 100 % | 100 % → 100 % | +11 pp [+0, +22] |
| blast radius (4) | 0 % → 25 % | 100 % → 100 % | 100 % → 100 % | +8 pp [+0, +25] |
| tests (3, Vue) | 0 % → 0 % | 33 % → 0 % | 67 % → 67 % | −11 pp [−33, +0] |
| flow (4) | 50 % → 25 % | 75 % → 50 % | 100 % → 75 % | −25 pp [−50, +0] |
| config (4) | 100 % → 75 % | 100 % → 100 % | 100 % → 100 % | −8 pp [−25, +0] |

* **With a shell and 10 or more turns, no success gain.** (Re-run after #169
  in [#171](#re-run-of-c-after-169-171): C at 89 % at both budgets, +4 pp at
  10 turns and −7 pp at 40 against none, neither significant.) The agent without
  myceliums solved 85 % of the tasks with 10 turns and 96 % with 40; C solved
  81 % and 89 %, the 40-turn F1 difference −0.042 [−0.088, −0.008]
  (uncorrected). At 4 turns C's success gain (+15 pp) is not significant
  (McNemar p = 0.29); mean F1 rose by 0.263 [+0.078, +0.452]. #153's success
  gain under 3 rounds of tool calls without a shell does not carry over to a
  budget of 10 turns with a shell.
* **Where C lost, it twice trusted the graph.** In the primary analysis C lost
  four task-budget pairs at 10 or 40 turns that none solved. On the two Django
  flow tasks it answered from one `trace_flow` result and wrote that it had not
  checked the call sites; `trace_flow` lists chains up to a length limit, and
  the longer chains were missing. Once it checked with grep and still added 18
  names; once it ran out of turns. A fifth loss, `django-tests-number-format`
  at 40 turns (5 test files missing), is one of the three tasks outside the
  primary analysis: `find_callers` could only answer for `django/`, because the
  index has no `tests/`. On Vue, myc links 3,682 of 16,397
  call sites: imports between packages (`@vue/shared`, `@vue/reactivity`, ...)
  count as library calls, so callers in other packages are missing
  (`myc callers toRawType` reports none). C's Vue success equalled none's at
  every budget but 40 turns (one task fewer).
* **C was faster from 10 turns on, cheaper only at 10.** Wall time 0.69× and
  0.84×; output tokens 0.64× at 10 turns. Input tokens rose (1.51× at 4 turns,
  1.31× at 40); C's first request alone is 12,584 tokens against 7,165. Cost:
  1.27× at 4 turns, 0.78× at 10, 0.98× at 40.
* **MCP, not the CLI.** In C the agent called an MCP tool in 68 of 81 sessions
  of the primary analysis (77 of all 90); over all 90
  it called `find_callers` 143 times, `trace_flow` 14, `find_callees` 8,
  `impact_of_change` once (14 calls returned an error, 13 of them because the
  name was ambiguous), and it never ran `myc` from Bash, although the grep hook
  put a `myc callers …` hint into 205 of the 341 greps it saw. The SessionStart context
  line reached the model in 90 of 90 C sessions, naming the index current.
* **Questions that are not about calls** stayed at or near the ceiling in both
  conditions (one C session ran out of turns at 4).

Session accounting: 180 main sessions, each graded on its first attempt (no
retries), and 15 calibration sessions: 195 of the 195 approved. Five probe
sessions against the mock API used no model.

```bash
export AEVAL_CACHE=~/.cache/myceliums-agent-eval-163 AEVAL_WORK_ROOT=/private/tmp/aeval153 TS_PATH=<typescript 6.0>
python3 evals/agent/agent_eval.py fetch --repo django --repo vue
python3 evals/agent/agent_eval.py index --repo vue        # ~20 min on CPU (the Django index of #153 is reused)
python3 -I evals/agent/groundtruth/scale_tasks.py edges $AEVAL_CACHE/repos $AEVAL_CACHE/edges
python3 -I evals/agent/groundtruth/scale_tasks.py build $AEVAL_CACHE/edges $AEVAL_CACHE/repos > evals/agent/tasks/tasks-scale.json
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json check-tasks
for b in 4 10; do
  python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json run --label my-scale-t$b \
      --conditions none,C --reps 1 --max-turns $b --prompt-budget --bash --max-retries 0 --max-sessions 60
done
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json run --label my-scale-t40 \
    --conditions none,C --reps 1 --max-turns 40 --timeout 480 --bash --max-retries 0 --max-sessions 60
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json export-scale --label my-scale \
    --runs my-scale-t4,my-scale-t10,my-scale-t40
python3 evals/agent/agent_eval.py report my-scale
```

## Re-run of C after #169 (#171)

#163 measured C slightly below none at 10 and 40 turns (81 % and 89 % against
85 % and 96 %, primary analysis, not significant), and two of C's losses were
answers taken from a partial `trace_flow` result. #169 (PR #170) made every
graph walk that stops early say so: `trace_flow`, `find_callers` /
`find_callees` with a depth, `impact_of_change` and `myc callers/callees/impact`
end a cut result in an `Incomplete (N branch(es) not followed)` block whose
`Next:` line names the step that continues it (a deeper or wider call, "read
the source at the lines listed above", "search the source for `X(`"); Vue's
workspace packages and tsconfig paths now resolve, so calls between its
packages get edges; the routing block (v3), the server instructions and the
subagent say graph answers are heuristic. #171 asks whether the #163 shortfall
disappears.

**Design.** Condition C only, at 10 and 40 turns, 30 tasks × 2 budgets × 1
repetition = 60 sessions (the approved budget; up to 5 retries for technical
failures, none needed). Everything else as in #163: `tasks/tasks-scale.json`
(sha256 `5c5164668ae6…`, the file the #163 results are graded against; it
differs from the freeze `fd0f92ee8148…` only by the optional item of the #163
review), the #153 prompt with the budget stated at 10 turns and an unstated
cap at 40, timeouts 360 and 480 s, `claude-sonnet-5-5` at `--effort medium`,
Claude Code 2.1.330, `Read`/`Grep`/`Glob`/`Bash`, the isolation, and
`myc setup-claude --scope project --no-agent`, at the same working paths. Per
session the prompt was compared byte for byte with the #163 one (identical in
60 of 60), and the command line differed only in temporary paths. The `none`
and old `C` rows are the committed #163 sessions, paired by task; no session of
#163 was re-run. `report` reads them from `results/2026-11-22-scale/`.

**Binary and indexes.** `myc` built in release mode from `d1edc390e0c4`
(sha256 `4f59a4b2ada3…`). Both indexes were built fresh by it
(`myc analyze --strict-embeddings`): Vue with the #163 configuration (15,750
symbols, all embedded; 20,566 relationships against 19,544), Django with the
#153/#163 one, without `tests/` (12,443 symbols, all embedded). The plan was a
Django index with `tests/`, in myceliums' default configuration
(`index --default-config django`): it parsed 41,842 symbols, embedded all of
them in 8,386 s, and then community detection had not finished after more than
1.5 hours; the build was stopped and the problem filed as #172. So the confound
of #163 remains: in C myceliums had no data for the three Django `tests`
tasks, and the primary analysis is the same 27 tasks; the 30-task numbers are
given alongside. Index fingerprints (sha256 over the sorted sha256 of every
index file) are in the report's notes.

**Hints.** `aeval/hints.py` reads each C transcript: the myceliums results (MCP
calls and `myc` runs in Bash) that carry an `Incomplete` block or a structured
`incomplete`, and whether a tool call made after the result arrived takes one
of the `Next` steps, judged by the kind of call (the same myceliums tool; a
`Read`, `Grep` or non-`myc` Bash call for "read the source"; a `Grep` or Bash
call containing `X` for "search the source for `X(`"), not by its exact
arguments. Run over the 90 C transcripts of #163 it finds no block, as
expected for the old binary.

**Results, 22 November 2026** ([report](results/2026-11-22-rerun-c/REPORT.md)).
Primary analysis, 27 tasks; brackets: 95 % paired bootstrap intervals over
tasks; McNemar's exact test per budget; **nothing corrected for multiple
comparisons** (two budgets, two baselines, two task views, several metrics):

| Per session | 10 turns: none (#163) | C (#163) | C (#171) | 40 turns: none (#163) | C (#163) | C (#171) |
|---|---:|---:|---:|---:|---:|---:|
| Success (F1 ≥ 0.8) | 85 % | 81 % | 89 % | 96 % | 89 % | 89 % |
| … C (#171) − none, McNemar | | | +4 pp [−7, +15], 2 : 1, p = 1 | | | −7 pp [−19, +0], 0 : 2, p = 0.5 |
| … C (#171) − C (#163), McNemar | | | +7 pp [−7, +22], 3 : 1, p = 0.63 | | | +0 pp, no discordant task |
| Mean F1 | 0.880 | 0.832 | 0.927 | 0.971 | 0.930 | 0.942 |
| … C (#171) − none | | | +0.047 [−0.026, +0.144] | | | −0.030 [−0.069, +0.003] |
| Sessions with ≥ 1 myceliums call | 0 % | 85 % | 85 % | 0 % | 85 % | 81 % |
| Input tokens (C (#171) ÷ none) | 83,926 | 91,192 | 1.04× [0.84×, 1.27×] | 100,535 | 132,074 | 1.39× [1.05×, 1.79×] |
| Output tokens (C (#171) ÷ none) | 3,745 | 2,412 | 0.73× [0.59×, 0.87×] | 4,147 | 3,619 | 0.86× [0.74×, 0.98×] |
| Cost, USD, CLI estimate (C (#171) ÷ none) | 0.091 | 0.071 | 0.88× [0.72×, 1.05×] | 0.102 | 0.100 | 1.04× [0.88×, 1.21×] |
| Wall time, s (C (#171) ÷ none) | 30.3 | 20.9 | 0.74× [0.62×, 0.86×] | 35.4 | 29.6 | 1.01× [0.77×, 1.30×] |

All 30 tasks: success 87 % / 83 % / 90 % at 10 turns (C (#171) − none +3 pp
[−7, +13]) and 97 % / 87 % / 90 % at 40 (−7 pp [−17, +0]).

* **At 10 turns the shortfall did not recur.** C solved 89 % against none's
  85 % and the old C's 81 %; neither difference is significant (2 : 1 and
  3 : 1 discordant tasks). C was faster than none (0.74× wall time).
* **At 40 turns C still solved two tasks fewer than none**, the same two as in
  #163: `django-flow-runserver-import-string` (F1 0.64; it read the source
  after the hint and still missed 8 functions of the exception-handling
  branch) and `vue-amb-reactivity-warn` (F1 0.68, 22 names too many). Mean F1
  −0.030 [−0.069, +0.003] against #163's −0.042 [−0.088, −0.008]. At 40 turns
  C was neither faster (1.01×) nor cheaper (1.04×) than none, unlike #163
  (0.84× wall time).
* **The hints reached the agent, and it often took them.** 45 of 60 sessions
  received at least one result with an `Incomplete` block (63 results); for 39
  of them a later call took a named `Next` step, and 39 of the 45 sessions read
  the source after a hint. In the 10-turn loss on
  `django-flow-runserver-import-string` it did not: it answered from two
  `trace_flow` calls although the block said three calls had no edge and
  named "read the source at the lines listed above", and gave the #163 answer
  (F1 0.48). The `myc` CLI was again never run from Bash.
* **Exploratory, per type** (primary analysis, uncorrected): the 3 Vue `tests`
  tasks went from 0 % (old C) to 67 % at 10 turns; `flow` at 10 turns from
  50 % to 75 % (none 75 %); one `transitive_callers` task fell below 0.8 at 10
  turns (`vue-tc-code-frame`, F1 0.76).

Caveats: the arms without myceliums and the old C ran on the same day about
nine hours earlier (10:34-11:12 UTC against 20:06-20:24 UTC), not interleaved,
so any drift of the model behind the alias or of the service counts into the
differences. One repetition; the gold labels of `tasks-scale` were derived by
an agent and are not reviewed (#164 left this set out). The 4-turn budget was
not re-run.

Session accounting: 60 sessions, every one graded on its first attempt (no
retries, no timeouts, no turn-limit hits); two probe sessions against the
mock API used no model.

```bash
export AEVAL_CACHE=~/.cache/myceliums-agent-eval-171 AEVAL_WORK_ROOT=/private/tmp/aeval153
python3 evals/agent/agent_eval.py index --repo vue
python3 evals/agent/agent_eval.py index --repo django   # --default-config django: with tests/, did not finish (#172)
for b in 10 40; do
  python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json run --label my-rerun-t$b \
      --conditions C --reps 1 --max-turns $b --timeout $([ $b = 10 ] && echo 360 || echo 480) \
      $([ $b = 10 ] && echo --prompt-budget) --bash --max-retries 0 --max-sessions 30
done
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-scale.json export-scale --label my-rerun \
    --runs my-rerun-t10,my-rerun-t40 --rerun-of 2026-11-22-scale --primary-exclude-reason "..."
python3 evals/agent/agent_eval.py report my-rerun
```

## Java tasks on Spring Framework (#182)

#163/#171 measured mid-size Python/TypeScript repositories with symbol-named
questions, where grep is strongest, and found parity. #182 asks the same on a
large Java code base, with task categories where a code graph might help, to
different degrees: does myceliums change success, F1, tokens and cost per
solved task, and in which category? This section
is the prepared measurement: task set, gold labels, review, index and run
configuration, and the budget, written before any session; what ran since is under
[Calibration and pre-registration](#calibration-and-pre-registration-of-the-main-run-182),
the outcome under [Result](#result-of-the-main-run-182).

**Repository.** Spring Framework v7.1.0 (`spring-framework` in
[`repos.json`](repos.json), commit `f2e4863de60e`, the #177 Java sets'
checkout): 11,464 files, 9,295 Java files in 23 Gradle modules, 470 AsciiDoc
pages. Indexed in myceliums' default configuration (code, tests, Kotlin,
`framework-docs/`, the config files of #179): 10,158 files, 145,670 symbols,
492,203 relationships with the binary of `276d9169f443` (main with #179,
#180, #181 and #190).

**Tasks** ([`tasks/tasks-java.json`](tasks/tasks-java.json); 30, six per
category; `labelled_by: agent`):

| Category (`type`) | Tasks | Answer | Gold size | Example |
|---|---:|---|---|---|
| concept (`concept`) | 6 | methods | 1-2 (+ 35 optional neighbours over the six) | where a transaction's rollback rule for a thrown exception is chosen; where Spring AOP chooses a JDK proxy or a subclass |
| deep transitive (`transitive_callers`, `blast_radius`) | 4 + 2 | methods / files | 10-21 | every caller up to three levels up of a private or static spring-core/spring-aop method, across 3-5 modules |
| interface dispatch (`dispatch`) | 6 | classes / methods | 6-16 | which `HandlerMethodReturnValueHandler.handleReturnValue` bodies can run; which `doBegin` overrides of `AbstractPlatformTransactionManager` |
| by-name references (`by_name`) | 6 | classes / files | 11-22 | the registrars in `META-INF/spring/aot.factories`, the default `TestExecutionListener`s of `spring.factories`, the XML namespace handlers, spring-aspects classes named in string literals |
| docs to update (`docs`) | 6 | files | 10-16 | the `framework-docs/` pages and documentation samples to update if `WebMvcConfigurer.configureViewResolvers` or `CaffeineCacheManager` were renamed |

**The categories are not equally graph-friendly, and they are reported
separately.** About 12 of the 30 tasks, all six docs tasks and most by-name
tasks, can be answered with a single text search for a name the question
gives, then reading the hits; concept, deep transitive and dispatch tasks need
several steps. The pooled numbers are therefore no test of "tasks that favour
a graph"; the report gives recall, success and F1 per category.

Every question states its own rules (scope `*/src/main/java` unless the
category needs resources or docs; what counts as a call, an implementation or
a reference; levels; overloads, lambdas, constructors). Concept questions do
not contain the answer's symbol name. The prompt is the same in every arm and
names no tool (`tests/test_schedule.py` checks the 30 prompts for tool words).
The set is **not calibrated**: no session ran, so whether the tasks sit
between floor and ceiling is unknown until the first repetition.

**Grading notes.** Success stays F1 ≥ 0.8 as in every earlier set. Four of
the six concept tasks have a single required method (the other two have
two), so for them success means the exact answer: that method and nothing
beyond its optional neighbours (listed per task, neutral in grading); a
second wrong method already gives F1 0.67. Recall per category is reported
next to success so that this is visible. Symbol answers of this set must
name the owner (`"require_owner": true`, the prompt's `Type.method` rule): a
bare method name, accepted in the earlier sets, counts as wrong here.
Answers carry no package, so a class name that exists in two modules still
matches either one: `AbstractHandlerMethodMapping.lookupHandlerMethod` and
`RequestMappingInfo.compareTo` (gold of the Spring MVC concept task) also
name the WebFlux classes of the same names. This leniency is the same in
every arm.

**Gold.** Traced by reading the code and the docs, never from myceliums
output: `git grep` per level and reading each call site's receiver type
(deep transitive), plain-text type-hierarchy and call-site helpers
([`groundtruth/java/javahier.py`](groundtruth/java/javahier.py),
[`groundtruth/java/jcalls.py`](groundtruth/java/jcalls.py)) cross-checked with
`git grep` (dispatch), the factories/handlers/XML files and string literals
(by name), word-boundary search over `framework-docs/` and reading every hit
(docs). Each task records its method in `gold_method`; each gold item has an
evidence line that `check-tasks` re-verifies.

**Review** (owner policy `owner-2026-11-22`, as for #164). An independent
second agent re-derived every task's set from the code (its own caller levels,
its own hierarchy scanner, its own reading of the resources and docs) and
judged all 383 claims (316 required answers, 37 optional answers, 30 "is the
gold complete?"): 382 confirmed, 1 wrong
([`../review/prescreen/agent-tasks-java.jsonl`](../review/prescreen/agent-tasks-java.jsonl)).
A third agent decided the disagreement: the optional answer
`BeanDefinitionRegistryPostProcessor` was removed, because its only body is
an interface default method, which the question excludes
([`../review/record-182/`](../review/record-182/): `third-check.jsonl`,
`provenance.jsonl` with a class per item). No person has reviewed the set;
each task's `review` says so and `reviewed_by` is `null`. For Marc's spot
check, the review UI serves the dataset `agent-tasks-java` with its own
sample (58 of the 382 confirmed items, seed 164) and decisions file:

```bash
python3 evals/review/export.py
grep '"dataset": "agent-tasks-java"' evals/review/items.jsonl > /tmp/items-182.jsonl
python3 evals/review/serve.py --items /tmp/items-182.jsonl --sample evals/review/record-182/sample.json \
    --decisions evals/review/record-182/decisions.jsonl
```

**Arms.** Issue #182's arms map to the harness's conditions, all with `--bash`:
A (grep, no myceliums) = `none`; C (myceliums + grep) = `C`; D (myceliums
only) = `D` (see [Arm D](#arm-d-myceliums-only)).

**Schedule.** `--profile realistic` (100-turn cap not stated in the prompt, 3
repetitions) now also fixes `--schedule interleaved`: per repetition the tasks
run in a shuffled order, each task's three arms back to back, and the arms'
order inside the blocks is counterbalanced (rotations of one shuffled order,
assigned to the tasks at random), so each arm runs first, second and third
in exactly 10 of the 30 blocks of every repetition. All arms therefore share
the same stretch of time, unlike #171, where the arms ran nine hours apart.
An interleaved run refuses `--condition` (one arm alone). Its export records
`comparisons: pairwise`, and the report then has no baseline: each pair (A vs
C, A vs D, C vs D) gets McNemar's exact test on task success (a task counts
as solved in an arm when at least half of its repetitions are, Holm-adjusted
over the three pairs), paired bootstrap intervals over tasks for the
difference in success and F1, and tokens (input + output) and cost per solved
task with their ratio; a multiplicity statement (3 pairs × 4 measures, only
the success tests corrected) and a per-category table of recall, success and
F1 follow. `plan` prints the
order and the balance without running anything; the run directory keeps
`plan.txt`. The default schedule of earlier runs (`paired`, arms adjacent in a
random order) is unchanged.

```bash
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-java.json plan --conditions none,C,D --profile realistic
# 270 sessions: 30 tasks x 3 conditions x 3 reps; schedule interleaved
#   rep 1: position counts none [10, 10, 10]; C [10, 10, 10]; D [10, 10, 10]   (reps 2 and 3 the same)
```

**Index** (embeddings included). `index` runs `myc analyze --strict-embeddings`
with jina-embeddings-v2-base-code (the default; long symbols in several
chunks). Without embeddings the analysis takes 34 s; embedding is the cost.
Measured on this machine (shared, 4 CPU-bound build jobs of other sessions
running): spring-jdbc alone, 4,896 symbols, 426 s, 11.5 symbols/s. For
145,670 symbols that is **about 3.5 hours** (8 hours at #171's Django rate of
5 symbols/s on a loaded machine). Peak memory for the analysis without
embeddings was 3.1 GB in #177.

```bash
export AEVAL_CACHE=~/.cache/myceliums-agent-eval-182 AEVAL_WORK_ROOT=/private/tmp/aeval182 \
       FASTEMBED_CACHE_DIR=~/.cache/myceliums-eval-models
cargo build --release -p myc && mkdir -p $AEVAL_CACHE/bin && cp target/release/myc $AEVAL_CACHE/bin/myc
python3 evals/agent/agent_eval.py fetch --repo spring-framework
python3 evals/agent/agent_eval.py index --repo spring-framework          # ~3.5 h on CPU
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-java.json check-tasks
```

`index --skip-embeddings` builds the graph only (34 s) for `--mock-api`
checks; `meta.json` records `"embeddings": "skipped"` and a real run refuses
that index.

**Run** (after approval):

```bash
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-java.json run --label 182-spring \
    --conditions none,C,D --bash --profile realistic --timeout 1200 --max-retries 2
python3 evals/agent/agent_eval.py --tasks evals/agent/tasks/tasks-java.json export --label 182-spring
python3 evals/agent/agent_eval.py report 182-spring
python3 evals/agent/agent_eval.py tokens 182-spring
```

Sessions of one repository share its working path, so the 270 sessions run
one after another whatever `--concurrency` says. `--max-utilization` (0.97)
stops scheduling before the account's limit; a stopped run resumes with the
same command, and the repetition-major order keeps completed repetitions
complete.

**Budget estimate** (CLI cost estimate at the published Sonnet 5.5 prices,
`aeval/tokens.py`). Base: #163's 40-turn sessions (unstated cap, as here),
per session: none 8.1 turns, 100k input tokens, 4.1k output, $0.101, 36 s;
C 8.1 turns, 128k, 3.5k, $0.098, 29 s; #171's C re-run 132k and $0.100.
Each call re-reads the context, so input tokens ≈ turns × first-call size +
growth per turn × turns² / 2; fitted on #163 the first call is 7.2k (none)
and 12.6k (C) tokens and the growth 1.3k and 0.8k tokens per turn. Assumed for
Spring: 20 turns per session (2.5×: answers of 10-22 items over three
levels or 10-16 doc files in a 9,300-file repository), 1.5× the growth per
turn (larger grep output), D between A and C (10.5k first call: the tool
schemas without the routing block; 1.0k × 1.5 growth). Cost: new context is
a 1-hour cache write ($4/M), the re-read part a cache read ($0.10/M), output
$10/M, scaled by 0.85, the factor by which this model over-predicts #163's
none arm.

| Arm | Sessions | Input tokens / session | Output / session | USD / session | Input tokens, arm | USD, arm |
|---|---:|---:|---:|---:|---:|---:|
| A (`none`) | 90 | ~520k | ~10k | ~0.28 | ~47M | ~25 |
| C | 90 | ~490k | ~9k | ~0.24 | ~44M | ~22 |
| D | 90 | ~510k | ~9k | ~0.25 | ~46M | ~23 |
| **Total** | **270** | | | | **~137M** (about 90 % cache reads), ~2.5M output | **~70** |

Range: at 12 turns per session about $35, at 30 turns about $140 (input
tokens grow with the square of the turns). Wall time: #163 spent about 4 s per
turn; the fixed cost per session (CLI start, working copy and index copy)
was 9.4 s in the mock-API run below; at 20 turns that is ~90 s per session, **~7 hours** for 270 sessions in sequence (4-11
hours over the range), plus the index build (~3.5 hours, once). A
calibration of one repetition of arm A on the 30 tasks (30 sessions, about
$8) before the main run would show whether the set sits between floor and
ceiling; it is not part of the 270.

**Calibration rule (fixed before any session).** A calibration, or anything
seen in the main run, may only fix wrong gold (an item the question's own
rules contradict, shown from the code) or unclear wording (a rule two careful
readers apply differently). It never drops, replaces or changes a task
because of how any arm performed on it: not because a task was too easy, too
hard or favoured one arm. Every fix is recorded with its reason, and the
stored answers of all arms are re-graded alike (`regrade`).

**Pipeline check against the mock API** (no model call, no usage; myc at
`276d9169f443`, Claude Code 2.1.330, the index without embeddings):

* `probe --conditions none,C,D --bash` on `spring-tc-auto-proxy-creator`: no
  problem in any arm. `none`: `Bash, Glob, Grep, Read`, no myceliums tool, the
  shell's `myc` not found. C: the 9 core tool schemas up front, the routing
  block, the SessionStart line (`index: f2e4863 = HEAD`), the grep hint, and
  `myc callers registerOrEscalateApcAsRequired` from Bash answering from the
  session's index (3 direct callers). D: `Bash, Read` and the 9 tools; the
  scripted `grep -rl` refused by the allowlist hook, `myc callers` ran.
* `run --mock-api --profile realistic --rep 1 --conditions none,C,D --bash`
  (each session's final answer is the task's gold, `--mock-answers gold`),
  then `export`, `report` and `tokens`: 90 sessions (30 tasks × 3 arms, the
  interleaved order) in 14 minutes, every one `ok`, no isolation problem, every
  answer extracted and graded F1 1.00, so all 30 gold sets survive the
  answer normalisation. `REPORT.md` has the three arms; the scripted
  `myc callers` from Bash counts as a myceliums call in C and D (100 % of
  their sessions) and not in `none`, where no `myc` exists. `TOKENS.md` has a
  section for C and one for D against `none` (before #182 it showed only the
  first arm); its search audit counts 30 blocked and 0 leaked searches in D.
  The mock reports one token per call, so the token tables only show that
  the pipeline runs; the attribution itself is tested on real transcripts
  (#178). The mock run's results were not committed.

New in the harness for this: `--schedule`/the profile's interleaved order and
`plan`, `run --mock-api`, `index --skip-embeddings`, myceliums calls from Bash
in the variant report, and a token report per arm
([`tests/test_schedule.py`](tests/test_schedule.py)).

### Calibration and pre-registration of the main run (#182)

Written on 2026-11-23 **after** the calibration and **before any session of
arm C or D**, and before the first session of the main run.

**Calibration** ([report](results/182-spring-calibration/REPORT.md),
[tokens](results/182-spring-calibration/TOKENS.md),
[supplement](results/182-spring-calibration/SUPPLEMENT.md)): arm A, 30 tasks,
one repetition, `--bash --max-turns 100 --timeout 1200`. 29 of 30 solved, mean
F1 0.989, mean 4.0 turns (median 3, at most 12), 45k input and 1.6k output
tokens per session, $1.47 in total ($0.049 per session). The harness's grades
were re-derived independently from the raw transcripts (30 of 30 equal). No
session used a path outside its working copy; the checkout is clean at the
pin. The one miss, `spring-blast-interface-method`, lists ten files one caller
level beyond the question's rule; its ten gold files were checked again with
`git grep`. **No task, gold label or wording was changed.**

1. **Success is at the ceiling.** Arm A can be improved on by at most about
   one task, so this run cannot show a success gain for myceliums, whatever
   the truth is. The task set was meant to be hard for text search and is not
   (the rule above forbids changing it for that).
2. **Primary outcome: tokens and cost per solved task**, for A vs C, A vs D
   and C vs D, with input split into uncached, cache write and cache read,
   plus output and turns (`supplement.py`, `report`, `tokens`). **Success and
   F1 are a harm check.** This emphasis was chosen after seeing arm A's
   calibration and before any session of C or D. What the harm check can
   detect with 30 tasks × 3 repetitions (a task is solved when at least 2 of
   3 sessions are): with no task going the other way, an arm must lose 6 of
   30 tasks for McNemar's exact p < 0.05 uncorrected (p = 0.031), 7 for the
   smallest Holm-adjusted p of the three pairs (7: p = 0.016, × 3 = 0.047).
   80 % power needs a true loss of about 25 (uncorrected) or 29 (Holm)
   percentage points. Smaller harm goes undetected: no significant difference
   is **not** evidence of equal success. The intervals on spend are 95 %
   paired bootstraps over tasks, not corrected for the number of comparisons.
3. **No pooling.** The calibration sessions are not part of the main run's
   results.
4. **Validity threat: memorisation.** Spring Framework is very likely in the
   model's training data, which may be part of the ceiling. `supplement.py`
   counts, per arm and category, the answer items that never appeared in the
   session's tool output (calibration: 3 of 326, all correct). That is a lower
   bound: a name the agent remembered and then confirmed with one search
   counts as seen. Results may not transfer to a private code base.
5. **Estimate correction.** The budget above (~$70 for 270 sessions) assumed
   20 turns per session; the calibration measured 4.0. The new estimate is
   about $12 at the planned cost ratios of C and D to A, about $31 if C and D
   each cost three times A.

**Wall time** is not an outcome: the calibration overlapped the index build
and other jobs (load 60-200 on 10 cores). The main run starts after the index
is built and the #183 Gradle prechecks have ended; a per-minute log of load
and competing jobs is kept for the run.

**Harness change before the main run:** a session cut off by the account's
usage limit could be recorded as a final `timeout` or `error` (F1 0); it is
now always `usage_limit`, not final, and re-run; a session killed from outside
(SIGKILL, SIGTERM) is a `crash`, also not final
(`tests/test_schedule.py`, `UsageLimit`). No stored session was affected.

### Result of the main run (#182)

270 sessions on 2026-11-23, 20:27-22:01 UTC ([report](results/182-spring/REPORT.md),
[tokens](results/182-spring/TOKENS.md), [supplement](results/182-spring/SUPPLEMENT.md),
[audit](results/182-spring/outside_audit.json)): all `ok`, no retry, no usage
limit, no isolation problem, $14.84 (A 3.51, C 3.16, D 8.17; the ~$70 estimate
assumed 20 turns, A took 4.1). The index was built with the binary of
`388c6e1` (sha256 `a42bffce8d3a`): 145,670 of 145,670 symbols embedded, 17,699 s
on a loaded machine, of which 17,626 s embedding; 1.2 GB; fingerprint in the
report's notes.

| | A (grep) | C (myc + grep) | D (myc only) |
|---|---:|---:|---:|
| Solved sessions (of 90) | 89 | 90 | 89 |
| Mean F1 | 0.996 | 0.997 | 0.985 |
| Tokens per solved task (input + output) | 46.8k | 47.8k | 125.5k |
| — uncached / cache write / cache read / output | 0.0 / 5.0 / 40.3 / 1.6k | 0.0 / 4.2 / 42.2 / 1.4k | 0.0 / 14.1 / 108.9 / 2.4k |
| Cost per solved task (USD) | 0.0395 | 0.0351 | 0.0918 |
| Turns per session | 4.1 | 3.7 | 10.6 |
| Sessions that used myceliums | 0 | 13 | 78 |

| Pair | Tasks solved in one arm only (first : second), McNemar p (Holm) | ΔF1 | Tokens per solved, ratio | Cost per solved, ratio |
|---|---|---:|---:|---:|
| A vs C | 0 : 0, p = 1 (1) | −0.001 [−0.010, +0.005] | 0.98× [0.82×, 1.22×] | 1.13× [0.97×, 1.37×] |
| A vs D | 0 : 0, p = 1 (1) | +0.011 [−0.003, +0.026] | 0.37× [0.24×, 0.59×] | 0.43× [0.29×, 0.65×] |
| C vs D | 0 : 0, p = 1 (1) | +0.012 [+0.000, +0.027] | 0.38× [0.25×, 0.58×] | 0.38× [0.26×, 0.57×] |

(Ratios first ÷ second: below 1 the first arm spends less.) 12 comparisons, only
the McNemar tests corrected. The two rows with C are confounded (#200, below).

**Reading.**

* **Offering myceliums next to grep (C): no difference detected** in tokens or
  cost per solved task; both intervals include 1 (C ÷ A: tokens 1.02× [0.83×,
  1.23×], cost 0.89× [0.74×, 1.03×]). **This comparison is confounded (#200):** A
  and D had Spring's own `AGENTS.md` (4,262 characters: build, code style, Javadoc,
  tests, commits, and one line on where reference docs and their snippets live, which
  the docs tasks ask about) in their context, C did not: the `CLAUDE.md` that
  `myc setup-claude` then wrote took its place. Seen in the probe's requests: the
  text is in all of A's and D's and in none of C's. Its size, from first-call
  usage at equal prompt length against #163's `none` sessions (same CLI, no
  instruction file): 1,804 tokens. With it added to each of C's model calls and
  behaviour held fixed, C ÷ A is **1.16×** in tokens instead of 1.02×, and 0.90× to
  1.08× in cost instead of 0.89×, depending on caching (point estimates; whether C
  would have behaved differently with the file is unknown). The 2,162 tokens between
  C's and A's first calls are the same on both CLI versions. C's first prompt, 11.0k
  tokens against A's 8.8k, is thus about 4.0k above A's without that file. #163 and
  #171 are not affected: Django and Vue (and the pilot's fastapi, hono and fzf) have
  no `AGENTS.md`, `CLAUDE.md` or `.claude/` at their pins.
* **C did not use what it had** (post hoc): a myceliums call in 13 of 18
  deep-transitive sessions (14 calls) and in **0 of 72** others, with the MCP
  server connected, the routing block, the SessionStart line and 142 grep hints in
  place. This is only partly a preference for grep: the routing block itself says
  that "grep/rg stay right for string literals, config values, log messages and
  non-code files", so on the docs and by-name tasks C partly followed the
  guidance; on concept and dispatch questions the block points to the graph.
* **C ÷ A by category** (post hoc, 6 tasks each), measured and with the missing
  `AGENTS.md` added as above (point estimates, behaviour held fixed; cost from all
  cache reads to written on every first call;
  [`POSTHOC.md`](results/182-spring/POSTHOC.md)):

  | Category | Tokens, measured | Tokens, adjusted | Cost, measured | Cost, adjusted |
  |---|---|---:|---|---:|
  | docs | 1.26× [1.13×, 1.38×] | 1.43× | 0.90× [0.78×, 0.98×] | 0.92×-1.21× |
  | dispatch | 1.22× [1.03×, 1.44×] | 1.39× | 0.89× [0.71×, 1.10×] | 0.91×-1.09× |
  | by-name | 1.12× [1.01×, 1.24×] | 1.27× | 1.01× [0.92×, 1.08×] | 1.03×-1.31× |
  | concept | 1.05× [0.81×, 1.27×] | 1.20× | 0.99× [0.70×, 1.42×] | 1.01×-1.35× |
  | deep transitive | 0.83× [0.46×, 1.26×] | 0.92× | 0.80× [0.50×, 1.13×] | 0.81×-0.89× |

  C used more tokens than A on docs, dispatch and by-name; whether it also cost
  more depends on the caching assumed for the missing file.
* **Replacing grep with myceliums (D) cost 2.3× [1.6×, 3.4×] as much and 2.7×
  [1.7×, 4.2×] the tokens per solved task** as A. Of the 77.8k extra
  tokens per session, 37.3k are myceliums results, 33.8k the base context re-read
  in more model calls (A 3.8, C 3.5, D 6.3 per session; `num_turns`, shown as turns
  above, is tool calls + 1), 7.2k file reads, 4.8k replies; search output is 6.1k less. Post
  hoc: the hook refused 107 commands, 93 of them searches, in 88 of the 90 sessions
  (no search got through, no session flagged), in 105 model calls, of which 88 were
  the session's first call and 17 came later. The 101 calls made only for refused commands are
  11.5 % of D's input by themselves (D ÷ A without them: 2.38× in tokens), 13.4 %
  with the re-reads of the refusals (2.33× without both). The rest is `search_code` (217 calls, 5.4k characters each, with file
  paths cut to `.../File.java`), 30 ambiguous-symbol errors and the extra calls. **D
  is not a fair proxy for myceliums on docs and by-name tasks**: what they ask for
  (text in pages, XML and factories files) is searched for with text search.
* **Harm check: nothing detected, and little could be.** No task differs at
  the majority level; a true loss below about 25-29 points would likely be
  missed, and no significant difference is not evidence of equal success. D's
  recall is lower on docs (0.919 against 1.000; 13 of the 16 missed files are XML
  samples of `framework-docs/`) and by-name (0.983).
* **By category** (6 tasks each; the intervals are post hoc, exploratory):
  tokens per solved task, D ÷ A: concept 1.4×, dispatch 2.4×, by-name 4.1×, docs
  12×, and deep transitive 0.73× [0.42×, 1.17×] (cost 0.65× [0.39×, 1.01×]), the
  only category whose point estimate is below 1; C ÷ A there 0.83× [0.46×,
  1.26×], 0.92× with the confound adjusted. D ÷ C over all tasks is confounded too:
  2.32× in tokens adjusted, 2.63× measured. The deep-transitive intervals include 1: a hypothesis for a new
  set, not a finding.
* **What this does not answer:** whether myceliums raises success. Arm A was
  at the ceiling (99 %), so the hypothesis of #182 could not be tested; that
  needs a new, harder, pre-registered set, on code the model has not seen.

**Verdict for the Clausohm decision.** On this evidence there is no reason to
expect better answers or lower cost from adding myceliums to a capable agent
with grep for tasks of this kind; a small loss or gain cannot be ruled out, and
C was measured without the project's own instructions (#200; about 1.16× A's
tokens with them, by a point estimate).
Replacing search by myceliums was clearly dearer here. The one lead worth testing is
deep multi-hop caller questions. The evidence is from an open-source code base
the model knows; it says little about a private one.

**Limits.**

* *Gold*: agent-labelled and agent-reviewed; no person has checked it.
* *Memorisation*: Spring is very likely in the training data. Answer items never
  seen in tool output (lenient): A 8 of 956, C 16 of 951, D 8 of 922; in
  the concept category A 2, C 7, D 5 of 24. A lower bound. Five sessions answered `spring-concept-mvc-best-match` exactly without any tool
  call (C 3 of 3, D 2 of 3, A 0 of 3), although the prompt says to investigate.
  Another 10 of D's 90 sessions made no myceliums call and read a file whose path
  was neither in the question nor in any tool output. Results may not
  transfer to a private code base. (Strict matching shows 84 for D; for the other
  76 file name and parent directory appear, but not the full path.)
* *Two CLI versions*: the CLI was updated during the run. 46 sessions from
  21:46 UTC (A 15, C 15, D 16, all repetition 3) ran Claude Code 2.1.331, the other
  224 ran 2.1.330 ([post hoc](results/182-spring/POSTHOC.md)). Without repetition 3, C ÷ A is 0.97× [0.74×,
  1.20×] in tokens and 0.86× [0.67×, 1.03×] in cost (all: 1.02×, 0.89×), D ÷ A 2.56×
  [1.58×, 4.10×] and 2.16× [1.45×, 3.15×] (all: 2.68×, 2.33×).
* *Instruction files differ between the arms* (#200), see above.
* *Time drift and caching*: the arms are interleaved, so they share the same
  hours, but the cost per session fell over the run (A 0.049, 0.036, 0.032 USD in
  repetitions 1-3) with the cache-write tokens (A 7.6k, 3.8k, 3.4k; C 6.4k, 3.1k,
  3.2k; D 16.0k, 12.4k, 13.4k), which fits 1-hour prompt caches shared between
  sessions. 31 sessions
  (A 12, C 16, D 3) have no cache write at all. Cost ratios need care for that
  reason: a real deployment may not have those caches.
* *Wall time* (A 14.1, C 13.4, D 28.2 s) is comparable between arms only because
  of the interleaving: load was 12-91 (median 31) on 10 cores from other jobs (a
  Gradle process in 3 of 95 minutes);
  nothing is claimed about absolute times.
* *Emphasis chosen after the calibration*: tokens and cost became primary once A
  was at the ceiling (see above).
* *myceliums' known gaps on Spring*: same-arity overloads (#186), generic type arguments (#187), method
  references (#188), dispatch to implementations outside the receiver's static
  type (#194). Their share in these results was not measured. The most frequent
  myceliums error was "Symbol ... is ambiguous" for overloaded methods.
* *No sandbox*: sessions run as the harness's user and could read other files
  (gold sets, caches) or, in A and C, use the network. `outside_audit.py` (added
  during the run, after a review of #183, without changing the running setup)
  checked all 270 transcripts. Paths outside the working copy: 24 hits in A,
  27 in C, 15 in D, of which one is a real access (an A session wrote and ran its own
  awk helper in `/tmp`); the others are regex text, a refused redirect, two reads of
  a path that does not exist and a `cd` back to the root. Network: 2 hits, both
  the file name `websocket.adoc`. D's refusal message shows the path of the hook
  script, which lies in the eval checkout, in 88 of 90 D sessions; no session
  followed it (later runs execute the hook from a neutral copy). **No session read a gold file or a later Spring version**, so the
  results with and without such sessions are the same. The audit parses commands
  statically and misses a path built at run time.

## Token attribution and cost (#178)

Where do a session's tokens go: into grep output, myceliums output, file
reads, or the model's own text? Every session row (since #178) and every
`TOKENS.md` answers it per category, prices the cache classes separately and
divides by the solved tasks. No model call is needed: it is read from the
stream-json transcripts ([`aeval/tokens.py`](aeval/tokens.py)).

**Input tokens per category.** Each model call re-sends the whole
conversation, so a session's input tokens are the sum over its calls of each
call's context (`input_tokens + cache_creation_input_tokens +
cache_read_input_tokens`, from the `usage` on the call's assistant events;
events of one call share a message id and count once). The first call's
context is the **base** (system prompt, tool definitions, task prompt,
SessionStart hook text). The growth from one call to the next is what arrived
in between: the model's previous reply and the tool results (and hook text)
it led to. That growth, measured, is split over those parts in proportion to
their size estimated as characters / 4 (for the reply: text and tool-call
inputs, plus the CLI's `thinking_tokens` estimate, since thinking is replayed
and its text is not in the transcript). Each part then stays in the context
for every later call, so a result's billed input is its size times the calls
after it. A call's usage is laid over the context in order: the first
`cache_read` tokens are cache reads, the next `cache_creation` tokens cache
writes (1-hour entries before 5-minute ones, as a prompt orders them), the
rest uncached. A growth with nothing to explain it goes to *other*; a context
that shrinks scales the earlier parts down. Without per-call usage the
context is estimated from characters / 4 alone, scaled to the session total
(`attribution_method: chars/4`). The chars/4 estimate of each category is
kept in the rows (`estimated_tokens_by_category`).

**Categories.** `base`; `myceliums` (MCP results and `myc` CLI runs in Bash);
`hook` (text the hooks add after the first call: in these runs the
`setup-claude` hooks); `search` (`Grep`, `Glob`, and Bash commands running
`grep`, `rg`, `ag`, `ack`, `ugrep`, `find`, `fd`, `locate`, `tree`, `awk`,
`git grep`, `git ls-files`, `ls -R`, `perl -n`, `sed -n '/re/p'` or
`git log -S/-G/--grep`); `read` (`Read`; Bash `cat`, `sed -n 1,9p`, `head`,
`tail`, ...); `edit` (`Edit`, `Write`, `sed -i`); `other`; `assistant` (the
model's earlier replies, re-read); `unattributed` (the residual, below);
`output` (generated tokens, thinking included, from the result event: the
per-event `output_tokens` are streaming snapshots and undercount). Bash
commands are parsed with quotes respected, through `bash -c`, `eval`,
wrappers (`timeout`, `nice`, `xargs`, `stdbuf`, ...), loops, `$(...)` and
git's global options; a command that runs several kinds of program gets the
highest of myceliums > search > edit > read
([`aeval/bashcmd.py`](aeval/bashcmd.py)). Code run through an interpreter
(`python3 - <<EOF`, `node -e`) is `other`: its intent is not visible.

**Reconciliation, and what it does not show.** The categories add up, by
construction, to the summed per-call usage. The residual is the session total
in the result event's `modelUsage` minus that sum, per model and field. It is
reported per session (`attribution_residual`) and kept as `unattributed`.
Over the 240 sessions of #163 and #171 it is 0 in every field and session.
That proves only that the transcript shows every model call and that the
per-call usage adds up to the CLI's totals; it says nothing about how a call's
growth is divided between categories. The cost priced from the table below
equals the CLI's `total_cost_usd` to within 5 × 10⁻⁷ USD per session.

**Cross-check of the split.** The split is fair between two kinds of content
(myceliums results and grep output, say) only if chars/4 misjudges both by the
same factor. `tokens` tests that independently of the reconciliation: it
fits, by least squares over every main-agent call of the run, the measured
growth on the chars/4 estimate of each kind of part plus a constant per call
(`fit_rates`; written to `tokens_crosscheck.json` and shown in `TOKENS.md`),
then re-splits every growth with the fitted rates (the *fitted split*
columns). On the 850 calls of #163 (R² = 0.98, 43 tokens of fixed overhead
per call): 0.446 tokens per character of myceliums results, 0.428 of grep
output, 0.434 of file reads, 0.417 of hook text, 0.481 of reply text, and
1.07 tokens per estimated thinking token. Myceliums results and grep output
differ by 4 %, so the plain split treats them alike; chars/4 undercounts all
text by about 1.7× but the CLI's thinking estimate hardly at all, so the plain
split gives the model's re-read replies 10-15 % too much. With the fitted
rates, C's myceliums tokens rise from 7.5k to 7.8k per session, its search
tokens from 8.9k to 9.2k, none's search tokens from 17.9k to 18.5k; no
conclusion below changes. The fit on #171's 288 calls agrees (myceliums
0.453, search 0.422 tokens per character).

**Cost.** Uncached input, cache writes (5 minutes: 1.25× input; 1 hour: 2×),
cache reads and output are priced with the published per-token prices of the
model each call reports (`PRICES` in [`aeval/tokens.py`](aeval/tokens.py),
from <https://platform.claude.com/docs/en/about-claude/pricing> as of
2026-11-03; `claude-sonnet-5-5`: input $2, 1-hour write $4, cache read $0.10,
output $10 per million tokens). Rows and `TOKENS.md` record the model ids
(`models`). `tokens --prices FILE` merges a JSON table over the built-in one.
A category's cost is its share of each class's cost. Claude Code writes the
cache with the 1-hour TTL, so in these runs almost all input is a 1-hour
cache write or a cache read (no 5-minute writes; at most 46 uncached input
tokens in a session).

**Per solved task.** Tokens and cost summed over a condition's sessions,
divided by its solved sessions (a ratio of sums, so unsolved tasks still add
their spend), with a 95 % paired bootstrap over tasks for each condition and
for the ratio ([`aeval/stats.py`](aeval/stats.py) `per_success_bootstrap`).

**#163 and #171, attributed afterwards** ([scale](results/2026-11-22-scale/TOKENS.md),
[re-run](results/2026-11-22-rerun-c/TOKENS.md); 27 tasks of the primary
analysis; thousands of billed tokens per session, all budgets together):

| Run | Arm | Base | myceliums | Hook | Search | Reads | Replies | Output | Total | USD/session | USD per solved, C ÷ none |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| #163 | none | 44.5 | 0 | 0 | 17.9 | 2.6 | 7.6 | 3.1 | 75.7 | 0.076 | |
| #163 | C | 66.4 | 7.5 | 1.3 | 8.9 | 1.5 | 5.8 | 2.5 | 93.7 | 0.072 | 0.93× [0.78×, 1.10×] |
| #171 (10 + 40 turns) | none | 52.7 | 0 | 0 | 24.9 | 3.8 | 10.7 | 3.9 | 96.2 | 0.096 | |
| #171 (10 + 40 turns) | C (re-run) | 75.1 | 13.0 | 2.0 | 12.5 | 1.8 | 8.8 | 3.2 | 116.5 | 0.093 | 0.99× [0.83×, 1.16×] |

With myceliums the agent read about half as many search tokens (−9.0k per
session in #163), and the myceliums results plus hook text took about as many
(+8.8k): the swap was roughly even. What made C's sessions larger is the base:
C's first call carries ~5.4k more tokens (the nine tool schemas, the CLAUDE.md
routing block, the SessionStart line), and every call re-reads them (+21.9k
per session). Because that part is a cheap cache read, C was not more
expensive per session or per solved task (#163: 0.93× [0.78×, 1.10×] cost per
solved task, while tokens per solved task were 1.22× [1.01×, 1.45×]). At 40
turns C used 1.40× [1.17×, 1.73×] the tokens per solved task of none (#171
re-run: 1.48× [1.08×, 1.96×]), at a cost ratio of 1.07× and 1.13× (both
intervals include 1).

**Search audit.** Every session row (and `TOKENS.md`) also carries a
post-hoc audit of the transcript ([`aeval/searchaudit.py`](aeval/searchaudit.py)):
Grep/Glob calls, Bash search commands (blocked by a hook / ran), interpreter
calls that ran, Read calls, and *blind reads* (a `Read` of a file whose name
appeared nowhere before: not in the prompt, an earlier tool input or result,
or a reply). A session is flagged by any Grep/Glob call, search that ran,
interpreter that ran, or 5 or more blind reads. In arms with search it
measures how much the agent searched (#163: 354 Bash search commands and 71
Grep/Glob calls in the 81 primary none sessions, 223 and 53 in C); in arm D
every flag is a hole in the isolation.

```bash
# a committed run whose transcripts are elsewhere; writes tokens.jsonl, tokens_crosscheck.json,
# TOKENS.md, tokens_summary.json
python3 evals/agent/agent_eval.py tokens 2026-11-22-scale --runs-root ../myceliums-eval-163-outcome-scale/evals/agent/runs
python3 evals/agent/agent_eval.py tokens 2026-11-22-scale --check   # CI: TOKENS.md matches the committed data
```

### Arm D: myceliums only

`--conditions D`: the myceliums MCP server (as in A, no `setup-claude` files)
and `Read`, with search taken away. `Grep` and `Glob` are not offered and are
in `--disallowedTools`. With `--bash`, Bash is reduced to an **allowlist**: a
PreToolUse hook (`python3 -I aeval/bashcmd.py hook`, passed with
`--settings`) lets a command run only if every program in it is one of:

* `myc`; `cd`, `pwd`, `true`; `mkdir`, `touch`, `mv`, `cp`, `rm`, `chmod`;
* `git status`, `git diff`, `git add`, `git commit`, `git log`,
  `git restore`, `git show`, without a search option (`-S`, `-G`, `--grep`,
  ...) and without a global option that loads configuration, code or another
  repository (`-c`, `--config-env`, `--exec-path`, `--git-dir`,
  `--work-tree`, ...): `git -c core.fsmonitor='grep ...' status` would run
  any command;
* the task's own build and test commands, matched exactly: each simple
  command of an `allowed_commands` string (default: the edit task's test
  command) with the same words, or `{"prefix": ..., "args": REGEX}`, the
  prefix followed by arguments that match the pattern in full.

Everything else is refused with a message that lists what is allowed:
`cat`, `head`, `sed`, `echo`, interpreters (`python`, `node`), shells
(`bash -c`), `eval`, any command substitution, any environment assignment or
wrapper in front of a command (`GIT_EXTERNAL_DIFF=... git diff`,
`env GIT_DIR=...`, `nice ...`), any argument naming a path inside `.git`
(`mv dotgit .git`), `git --output`, and any command that does not parse
(unbalanced quotes, or a heredoc never closed; heredoc terminators follow
bash, including `<<-` with tab-indented terminators). File operands of `cd`,
`mkdir`, `touch`, `mv`, `cp`, `rm`, `chmod` and every redirection target
(`git show :x > .git/config`, also after a task command) must not contain a
glob or expansion character (`* ? [ { ~ $`: `cp x .gi?/config`), must not
name `.git` or a git config, and must stay inside the working copy (the
hook's `cwd`, followed through `cd`). The same hook refuses `Edit`/`Write`
outside the working copy, into `.git`, and to `~/.gitconfig` or
`~/.config/git/`. The harness sets git's configuration through the session's
environment (`GIT_CONFIG_COUNT`), which the agent cannot change:
`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, `core.hooksPath` an
empty directory (so `git commit` runs no hook), `core.fsmonitor=false`,
`core.pager` and `pager.diff/log/show/status` `cat`, `core.editor` and
`sequence.editor` `true`, `safe.bareRepository=explicit`. `diff.external` is
not pinned: an empty value makes `git diff` fail ("cannot run"); the only git
config still read is the working copy's own, which D cannot write. Permission rules (`Bash(grep:*)`, ...) deny the
common search programs as a second layer. Reading a file goes through `Read`.
The edit tools are offered on edit tasks, as in every arm. The isolation
check of every D transcript requires the built-in tools to be exactly `Read`
(+ `Bash`, + edit tools) with an optional `ToolSearch`, and the search audit
above counts anything that got through. The regression tests
(`tests/test_tokens.py`, `DBypasses`) hold every bypass found in review. Over
the 875 Bash commands of the #163/#171 transcripts the classifier calls 816
search (`grep` pipelines, `awk`/`sed -n` filters, `$(grep ...)` in loops,
...), 43 reads, 1 edit and 15 other (interpreter scripts, `ls`); none of the
875 passes the allowlist. Checked against the mock API
(`probe --conditions D,A --bash`, myc at `951d041ff9e0`): D was offered
`Bash` and `Read` only, the scripted `grep -rl parseAccept .` was refused by
the hook, the scripted `myc callers parseAccept` ran, and the audit counted
one blocked search and no flag; A was offered `Grep` and `Glob` as before.

**Threat model.** Arm D isolates a *cooperative* agent that has been told,
by its tool set, not to search: it takes away the search tools and the shell
commands an agent reaches for, and the transcript audit backs that up. It is
not a sandbox against an adversarial agent: a determined one could still find
a way through an allowed program, the reviews of #185 closed the holes found
so far, not every possible one. Sessions run in a throwaway copy of the
pinned checkout, and any search-like behaviour that gets through (a search
command that ran, an interpreter, Grep/Glob, many blind reads) is caught by
the search audit and reported per session, so a D result is read together
with its audit counts.

### Edit tasks

A task with `"type": "edit"`, `"answer_kind": "diff"` asks for a change; the
agent gets `Edit` and `Write` and a prompt without an answer format
([`aeval/editgrade.py`](aeval/editgrade.py)). The gold is the files a
reference commit changed (`gold_files`, each `M`, `A` or `D`; for the commit
replays of #183, the commit on its parent, which is the repository pin).
`primary_files` names the file(s) of the core change; every other gold file
is a dependent. Before any agent runs, `agent_eval.py precheck-edit` runs
each task's `test_command` on a fresh copy of the pinned checkout and records
the result in the task file (`precheck`); `check-tasks` (and so `run`)
refuses an edit task whose test command was not prechecked on the current
pin or does not fail there, because passing it afterwards would prove
nothing. After the session, before the working copy is replaced, the runner
records `git status` and the diff and grades: (a) the test command must exit
0; (b) file-level recall and precision against the gold files, overall and
separately for code, tests and docs; (c) forgotten dependents, split into
docs, tests, *remote* code (another directory than every primary file: another
module) and *local* code (next to a primary file). Without `primary_files`
every gold file would be a dependent and (c) would equal 1 − recall, so it is
not reported. A session succeeds when the test command passes, or without one
when the file-level F1 over code files reaches the threshold.

File kinds, first rule that matches: a Java/Gradle source set decides
(`src/main/**` is code, also in a module named `spring-test/`; `src/test/**`,
`src/testFixtures/**` and `src/integrationTest/**` are tests, fixtures
included); docs are `*.md`, `*.adoc`, `*.rst` and anything under `docs/` or
`framework-docs/` (so Django's `docs/*.txt`, but not `CMakeLists.txt` or
`requirements.txt`); tests are anything under `tests/` or `__tests__/` and
test file names (`test_*.py`, `*_test.go`, `*.test.ts`, `*Test.java`, ...); a
library package named `test` (`django/test/`) is code; everything else is
code.

### Realistic budget profile

`run --profile realistic` sets `--max-turns 100` as a safety cap only (never
stated in the prompt; `--prompt-budget` is refused with it), 3
repetitions and (since #182) the interleaved schedule, see
[#182](#java-tasks-on-spring-framework-182). The profile
name is stored in the run's config, so a resumed run cannot change it.

## Commit replay on Java (#183)

Do agents change everything a real change needs, in other modules, tests and
docs, and does myceliums save tokens per solved task? 20 edit tasks replay
recent spring-framework commits. **Prepared, not run**: the sessions need
Marc's budget approval, and the prechecks need Gradle (below).

**Mining** ([`groundtruth/commit_replay.py`](groundtruth/commit_replay.py)
`mine`, over a blobless clone with history): non-merge commits on `main` since
2025-10-01 with at most 25 files (at most two renamed), no dependency bump,
polish, copyright, revert or release subject, changes to main code and to
tests, at least two build modules touched (`framework-docs` counts), and a
parent that builds with the Java 25 toolchain: **151 candidates**. 34 were
picked for a public API or behaviour change a description can state
(no pure refactoring or deprecation), and stratified by files, code modules
and docs; 20 are the task set, 13 are reserves
(`tasks/tasks-commit-replay-reserve.json`) that replace tasks whose precheck
fails.

| Task | Commit | Files (code/tests/docs) | Code modules | Docs | Primary files |
|---|---|---:|---:|---|---:|
| `spel-simple-context-restricted-conversion` | d41495720c | 5 (2/1/2) | 2 | yes | 1 |
| `observation-record-handled-exception` | 297c74d29c | 20 (10/9/1) | 4 | yes | 2 |
| `beans-max-nested-path-depth` | 8ab525f6f0 | 12 (6/5/1) | 2 | yes | 3 |
| `spel-big-power-result-limit` | 8a92c19e4d | 8 (4/2/2) | 2 | yes | 2 |
| `jsonl-media-type-support` | 22e4d84993 | 16 (11/2/3) | 2 | yes | 1 |
| `retry-max-attempts-to-max-retries` | 24590092ef | 13 (6/6/1) | 2 | yes | 2 |
| `fn-request-bind-pathvars-headers` | a9a404266c | 7 (3/2/2) | 2 | yes | 1 |
| `jdbcclient-batch-entry-dsl` | 833ab4d48f | 6 (2/3/1) | 1 | yes | 1 |
| `type-mismatch-required-type-arg` | 4847ee80b0 | 3 (1/1/1) | 1 | yes | 1 |
| `servlet-response-ignore-body-flush` | e0b54e244e | 5 (3/1/1) | 2 | yes | 1 |
| `content-negotiation-no-fallback-mapping` | 7fbdbd6eb7 | 4 (2/2/0) | 2 | no | 1 |
| `client-builders-deep-copy-default-headers-cookies` | 4cf6335372 | 6 (3/3/0) | 3 | no | 1 |
| `http-range-honor-if-range` | 8ab5332639 | 11 (5/6/0) | 2 | no | 1 |
| `error-response-interceptor-with-request` | d70220eb5d | 11 (7/4/0) | 3 | no | 1 |
| `mime-type-parameter-names-case-insensitive` | 2028eb3694 | 4 (2/2/0) | 2 | no | 1 |
| `deferred-bean-registrar-contains-bean` | 7502b92392 | 14 (7/7/0) | 2 | no | 2 |
| `cache-manager-reset-caches` | 96aadc2b12 | 6 (5/1/0) | 2 | no | 1 |
| `api-version-deprecation-handler-handler-arg` | b128f59714 | 12 (10/2/0) | 3 | no | 2 |
| `bean-provider-parameterized-type-reference` | 2e4ca41382 | 7 (6/1/0) | 3 | no | 1 |
| `message-source-default-charset` | 02af3e7a17 | 5 (4/1/0) | 2 | no | 1 |
Spread: 6 small (≤ 5 files), 10 medium (6-12), 4 large (13-20); code in one
module 2, two modules 13, three or more 5; 10 with docs.

**Descriptions** were drafted by agents from the commit message, the linked
issue and the diff ([`groundtruth/commit_replay_spec.json`](groundtruth/commit_replay_spec.json),
rules: an issue-style change request that names every new public name the
commit's tests need, may name the type of the core change, and never names
files, test classes, dependent classes, modules, docs or tests). A second
agent reviewed all 34 for leakage and solvability against the test diff (it
split them over four sub-agents and itself re-checked nine): 27 ok, 7 to fix.
Six were replaced by the reviewer's rewrite (two of them among the 20); the
rewrites were not reviewed again. The seventh, a reserve, had to name a
dependent (a Kotlin extension its module's tests compile against) and was
dropped, which leaves 13 reserves. No human has reviewed the descriptions.
Scope words an issue would use ("for both Spring MVC and WebFlux") are allowed
and noted in the spec.

**What the build derives** (`commit_replay.py build`): one pin per task in the
task file's own `repos` (the parent; `gold_commit` the replayed commit),
`gold_files` (the commit's name-status, renames as delete + add),
`test_overlay` (the commit's test files, fixtures included), the default test
command (`./gradlew --no-daemon --console=plain :module:test --tests ...` over
the changed test classes; three tasks narrow it, see the spec), arm D's
`allowed_commands` (`./gradlew` with task paths, `--tests` filters and a few
flags; `-I`, `-D`, `-P`, `-p`, `--init-script` are refused) and the strata.

**Harness additions.**

* `load_repos` merges the pins a task file declares (`repos`); `fetch` and
  `index` default to them. `fetch` shallow-fetches each parent and keeps the
  gold commit's files in `$AEVAL_CACHE/gold/<repo>/`, outside every working
  copy, so the agent cannot find the commit in the history (`check-tasks`
  verifies `gold_files` against the commit).
* Grading writes the commit's own tests (`test_overlay`) over the agent's
  working copy after its changed files are recorded, then runs the test
  command. An agent's edit of one of those test files is therefore replaced
  (its file still counts for recall); new public names the tests use are
  stated in the description.
* `precheck-edit` applies the overlay to the parent (the test must fail),
  then the whole commit (it must pass), and records both exit codes and run
  times; `check-tasks` refuses a task without both.
* The repository's `session_env` (`GRADLE_OPTS`: no Gradle daemon, Kotlin
  compiler in process, so no daemon outlives a session) and the caller's
  `JAVA_HOME` (`env_from_caller`; `$JAVA_HOME/bin` first on `PATH`) reach
  the session, the test command and the precheck.
* Files the harness wrote before the session (setup-claude's `CLAUDE.md` and
  `.mcp.json` in C) no longer count as the agent's changes (a snapshot before
  the session); before, every C session got them as extra files.
* Forgotten dependents also report `other_module`: code dependents in another
  Gradle module than every primary file (`remote` counts another directory,
  which in Java is any other package).
* `probe --edit-file/--edit-old/--edit-new/--edit-command`: the mock session
  reads and edits a file and runs the test command in Bash, so the probe
  shows the grading and D's allowlist on an edit task.

**Run configuration.**

```bash
export JAVA_HOME=$HOME/.cache/myc-java/jdk/jdk-25.0.5.1+2/Contents/Home   # Spring's build needs a JDK 25 toolchain
T=evals/agent/tasks/tasks-commit-replay.json
python3 evals/agent/agent_eval.py --tasks $T fetch           # 20 parents (~100 MB each) + gold files
python3 evals/agent/agent_eval.py --tasks $T precheck-edit   # fails on parent + overlay, passes on the commit
python3 evals/agent/agent_eval.py --tasks $T check-tasks
python3 evals/agent/agent_eval.py --tasks $T index --myc $MYC --models-dir ~/.cache/myceliums-eval-models --base $BASE_HOME
python3 evals/agent/agent_eval.py --tasks $T run --label 2026-11-xx-commit-replay --conditions A,C,D \
    --profile realistic --bash --timeout 2400 --concurrency 1 --myc $MYC --models-dir ~/.cache/myceliums-eval-models
```

Arms A (MCP server), C (what `myc setup-claude --no-agent` installs) and D
(myceliums only, Bash allowlisted to file operations, git and `./gradlew`
test runs); every arm has `Bash`, `Edit` and `Write`. Realistic profile:
100-turn cap not stated in the prompt, 3 repetitions and the interleaved schedule of #182
(repetition-major, tasks shuffled, the three arms adjacent per task with
their order counterbalanced). Each session gets a fresh copy of its task's
parent at one shared path (`work_name`: `spring-framework` under `AEVAL_WORK_ROOT`) and a
fresh copy of that parent's index. The path is shared because of the index
(below); its price is that **sessions run one at a time**.

**Index plan: derive, do not rebuild.** `myc` has no command that updates an
index from a git diff: file-level re-indexing (`analyze_files`) is reachable
only through `analyze --watch` (and email sync), and the post-commit hook
runs `analyze --skip-embeddings`. What it does have is a full graph rebuild
that takes over the vector of every symbol whose embedding text is unchanged
(`analyze --force`, #151), so only changed symbols are embedded. That reads the
previous index by repository id, which hashes the path: at another path
nothing would be reused, silently (read in the code, not run). Hence one shared work path, and `index --base HOME`:

* it refuses a base of another path, and an index that reused no vector;
* newest parent first, each index is derived from its neighbour's, so each
  step embeds one interval of history only;
* each is compared with a fresh graph-only build of the same working copy
  (symbols, files, relationships, mentions; `meta.json` `derived`).

The base is the #182 run's embedded index of v7.1.0 (same models; its path is
`/private/tmp/aeval182/spring-framework`, so `AEVAL_WORK_ROOT=/private/tmp/aeval182`),
used only after that run has finished with the path. v7.1.0 is the better
base than the oldest parent: 8 of the 20 parents differ from it by at most
606 files (7 to 606), against 3,039 to 3,296 from the oldest.
**Not measured yet** (embedding waits for the #182 build): vectors reused,
time and disk per task, graph equality. This was checked against a stand-in
`myc` in the unit tests only.

Measured on one parent (`mime-type-parameter-names-case-insensitive`, M1 Max,
10 cores, machine under other load): the graph alone took 73 s for 10,109
files, 143,541 symbols, 485,247 relationships and 39,899 doc mentions, 668 MB.
A full embedded build was stopped by a 2-hour tool limit while still
embedding; about 3 hours at Vue's #163 rate is an extrapolation across
different code and load, so no more than an order of magnitude. If derived indexes
fail, the fallback is graph-only with semantic search off in every arm, which needs
the #182 guard on `--skip-embeddings` indexes relaxed.

**Budget estimate** (not measured; from #163/#171 scaled to edit tasks).
#163/#171 question sessions with Bash at 40 turns: 8 turns, 100-136k input
tokens, 3.5-4k output, $0.10, 29-36 s per session (`claude-sonnet-5-5`,
medium effort: input $2, 1-hour cache write $4, cache read $0.10, output $10
per million). An edit session on Spring reads more and larger files, edits,
and runs Gradle: assumed 20-80 calls (central 40) with the context growing
from ~25k to 60-150k (central 100k) tokens. Input ≈ calls × mean context:
0.8M-7M (central 2.5M) tokens, almost all cache reads; plus one cache write
of the final context and 8-30k output.

| | Low | Central | High |
|---|---:|---:|---:|
| Input tokens per session | 0.8M | 2.5M | 7M |
| Cost per session | $0.40 | $0.80 | $1.60 |
| Sessions (20 tasks × 3 arms × 3 reps) | 180 | 180 | 180 |
| Tokens, whole run | 145M | 450M | 1.26B |
| Cost, whole run | $72 | $144 | $288 |
| Wall time per session (model + agent's Gradle runs + grading) | 6 min | 12 min | 25 min |
| Wall time, one session at a time (shared path) | 18 h | 36 h | 75 h |

Plus, once: the derived indexes (not measured) and the prechecks (40 Gradle
runs, not measured). The call counts and context sizes above are assumptions;
a pilot of one repetition (60 sessions) would replace them. Disk: 20 parents
about 2 GB, graph-only indexes 0.67 GB each, embedded ones larger (not
measured; about 1.1 GB if vectors add 0.4 GB, so 22 GB for 20, close to all the
free space: build, run and delete task by task), `~/.gradle` not measured
(nothing downloaded yet; the parents use 11 Gradle versions).

**Blocked: Java cannot connect on the benchmark machine.** Connections from
`java` (Homebrew JDK 23 and Temurin JDK 25) to the internet time out while
`curl`, `git` and Python connect and loopback works. Little Snitch runs on the
machine and is the likely cause, but its log showed nothing about `java` and
its rules were not readable, so this is not confirmed. Until `java` can connect,
Gradle cannot download its distribution, no precheck has run, and the test
commands themselves are unproven (none has been seen to pass). `check-tasks` then
reports every task as not prechecked; CI covers the task file with unit tests
only (`tests/test_commit_replay.py`).

**Mock validation** (no model call; run on the per-task paths, before the shared path). It shows the pipeline, not a passing grade: the scripted edit is a comment, so the tests would fail even with a working Gradle. `probe --profile realistic --bash` on
`mime-type-parameter-names-case-insensitive` against the graph-only index, with
the scripted session searching, calling `myc callers parseMimeType`, reading
and editing `MimeType.java` and running the task's test command:

| Arm | Isolation | Tools offered | Search | Graded |
|---|---|---|---|---|
| A | ok | Bash, Edit, Glob, Grep, Read, Write + 9 myceliums tools | Grep ran | 1 file changed (code recall 1/2, `MediaType.java` forgotten as an other-module dependent, both tests forgotten), overlay of 2 test files applied, test command ran (exit 1: no network for Gradle) |
| C | ok, routing block loaded | as A | Grep ran | as A; setup-claude's `CLAUDE.md`/`.mcp.json` not counted |
| D | ok | Bash, Edit, Read, Write + 9 myceliums tools | `grep` refused by the hook | as A; the hook let `./gradlew ... --tests ...` run, `myc callers` answered from the index |

```bash
export JAVA_HOME=...; T=evals/agent/tasks/tasks-commit-replay.json
python3 evals/agent/agent_eval.py --tasks $T index --repo spring-mime-type-parameter-names-case-insensitive --skip-embeddings --myc $MYC
python3 evals/agent/agent_eval.py --tasks $T probe --label v183 --task mime-type-parameter-names-case-insensitive \
    --conditions A,C,D --bash --profile realistic --myc $MYC --grep parseMimeType --callers parseMimeType \
    --edit-file spring-core/src/main/java/org/springframework/util/MimeType.java \
    --edit-old "public class MimeType implements Comparable<MimeType>, Serializable {" \
    --edit-new "public class MimeType implements Comparable<MimeType>, Serializable { // probe" \
    --edit-command "./gradlew --no-daemon --console=plain :spring-core:test --tests org.springframework.util.MimeTypeTests :spring-web:test --tests org.springframework.http.MediaTypeTests"
```

## Replaying myceliums calls (#181)

`agent_eval.py replay` measures a change to myceliums' answer format or tool
definitions without a model: it takes every `mcp__myceliums__*` call from a
run's transcripts, sends it again to two `myc` builds over stdio (both on the
same index, `repo_id` dropped so the server resolves the repository from the
working copy), and writes `results/<label>/REPLAY.md` and
`replay_summary.json`:

* sizes: the tool definitions as Claude Code sends them to the model (name
  with the `mcp__myceliums__` prefix, description, input schema), the server
  instructions, the `tools/list` JSON, and the answers per tool, in
  characters and characters / 4, plus `o200k_base` tokens when `tiktoken` is
  importable. `o200k_base` is an OpenAI tokenizer, not Claude's: it shows the
  ratio, not Claude's token count (#178 measures Claude tokens from usage).
* equivalence, per call: the same hits (file, qualified name, start line,
  depth or chain position, `via`, `[imports]` and `resolved` marks), totals,
  `Incomplete` reasons and `Next` step, read from the JSON of
  `find_callers`/`find_callees`, the older bullet lists or the compact lists
  grouped by file; error results must be the same text. The command exits 1
  when a call differs.

The working copies must be the repositories at the run's commits, with the
run's index exclusions in `.myceliums.toml` (`index_exclude_extra` in
`results/<label>/meta.json`); `--index` indexes them first
(`myc analyze --skip-embeddings`, enough for the graph tools) into `--data`,
which also serves as `HOME`.

```bash
# #181: the #163/#171 calls against origin/main and the branch (REPLAY.md in results/2026-11-23-compact-output)
uv run --no-project --with tiktoken python evals/agent/agent_eval.py replay 2026-11-23-compact-output \
  --runs-root ../myceliums-eval-163-outcome-scale/evals/agent/runs \
  --runs-root ../myceliums-eval-171-rerun-c/evals/agent/runs --run-glob '2026-11-22-*-t*' \
  --before /tmp/myc-before --after /tmp/myc-after --before-name "origin/main fbbf24f" --after-name feat-181-compact-output \
  --work django=/tmp/r181/work/django --work vue=/tmp/r181/work/vue --data /tmp/r181/data --index
python3 evals/agent/agent_eval.py replay 2026-11-23-compact-output --check   # CI: REPLAY.md matches its data
```

## Tool-selection eval

[`toolsel/intents.json`](toolsel/intents.json) holds 56 natural-language
requests, each with the myceliums tools a maintainer would accept as the first
call (primary first). Since #97 the expectations use the task-named core tools
(`search_code`, `find_callers`, `impact_of_change`, ...); a deprecated alias
is not a correct pick. `toolsel-run --toolsets <profile>` (default `all`;
`core` is what `myc mcp` serves by default) starts `myc mcp` with that profile
(empty data directory), reads the live `tools/list`, fails if any expected
tool is missing from `all`, scores a smaller profile on the intents it can
answer (19 of 56 for `core`), then
asks the model once per intent and pass, in a single turn, with only the
myceliums tools attached as native tool definitions (no built-in tools, tool
search off). The first tool call is the pick; it is never executed: the
session runs in `dontAsk` permission mode without any allow rule, so the call
is denied, and `--max-turns 1` ends it. Accuracy per pass, agreement between
passes and the most frequent confusions go to `TOOLSEL.md`.

## Limits

* Outcome eval at scale (#163): one repetition (each task, condition and
  budget is one sample), 30 tasks, two repositories, one model and effort
  level; per type at most six tasks, so the type rows are exploratory. No
  interval or p-value is corrected for multiple comparisons (three budgets,
  the pooled comparison, several metrics and subgroups). The reused Django
  index has no `tests/`, so the three Django `tests` tasks do not compare the
  conditions fairly; the primary analysis leaves them out (27 tasks).
  The labels were derived from the AST and the type checker by an agent and
  are not yet reviewed by a person (#164). The calibration covered 15 of the
  35 candidates: 20 tasks of the set, the four `config` tasks among them, are
  not calibrated, and with a shell the baseline at 10 turns (87 %) ended at
  the top of the 30-80 % target. The call-graph questions spell out static
  call rules a graph computes by construction; myc's own Vue graph misses the
  calls between packages, so the Vue results measure myc as it is, not a
  complete graph. The 40-turn arm is a cap the prompt does not state, not an
  unlimited session. Two sessions (one per repository) ran at a time; their
  scratch scripts shared `/tmp`. Bash reaches the whole machine: one none
  session found a TypeScript package and node outside its PATH and built its
  own call graph; no session read the eval's task files, gold sets or index
  cache (every transcript was searched for those paths).
* Re-run of C (#171): one condition, two budgets, one repetition; the
  baselines are the #163 sessions from about nine hours earlier, so time drift
  is not controlled. The Django index still has no `tests/` (#172), so the
  three Django `tests` tasks stay out of the primary analysis. Following a
  `Next` step is judged by the kind of the later call, not by its arguments
  or by whether it changed the answer. The labels are not reviewed.
* Outcome eval (#153): a pilot. #163 answers its open question with a shell
  and more turns: with 10 or 40 turns myceliums did not raise success, and C
  was faster (0.69× and 0.84× wall time, primary analysis) and at 10 turns
  cheaper (0.78×). One repetition, 11 paired tasks, one
  repository, one language, one model: the intervals are wide and say
  direction and rough size. The effect is measured under a tight budget
  (three rounds of tool calls), chosen in calibration because the agent
  without myceliums failed there. With up to seven rounds (and an earlier,
  partly wrong budget text) that agent was at the ceiling, so only the
  success gain is shown to be a budget effect; time and cost without a
  budget were not measured (C never ran without one), apart from the
  descriptive four blast-radius tasks both conditions solved. The gold sets
  follow static call rules that a call graph computes by construction,
  while an agent with grep has to emulate them; the tasks are structural
  questions of the kind myceliums is built for, not a sample of developer
  questions. Neither condition had a shell. The calibration (10 sessions)
  covered 9 of the 12 tasks, and one task was not run.
* Variant matrix (#125): both hard sets turned out close to the ceiling for
  this model, so the matrix measures tool use, tokens and time well and
  accuracy hardly at all. Its task set was not calibrated (the allowance was
  spent on the first set), B and D were not run, and 4 of 120 planned sessions
  are missing. The gold sets follow static call rules the questions spell out
  (interface and function-value calls do not count); an agent that reasons
  about runtime behaviour instead loses precision.

* Three repositories, 30 tasks, one model and one effort level. Small n per
  task type: read the per-type intervals as direction, not size.
* Tasks are answerable with grep and reading; they were not chosen to favour
  either condition, but they are not a sample of real developer questions.
* Gold sets were produced and checked by an agent with compiler tooling, then
  checked in #164 mostly by agents under Marc's owner policy, not item by item
  by a human. One label was amended after the pilot started (an
  optional item for `hono-dead-utils`, see the report's notes); grades are
  always recomputed from the stored answers, so every session uses the same
  labels.
* The tool-selection intents were written by the same agent that had read
  the tool descriptions. They avoid the descriptions' wording where a
  developer would not use it, but they are easier than requests in the wild,
  and the eval offers only myceliums tools, so it says nothing about whether
  an agent prefers them over its own.
* Token and cost figures depend on prompt caching across sessions and on the
  CLI's own accounting; `cost_usd` is the CLI's estimate at list prices, not
  what a subscription is billed.
* Wall time includes local CPU contention: the pilot ran on a busy machine.

<!-- label-review:start -->
## Review of the labels (#164)

Applied on 2026-11-22 with `evals/review` under policy `owner-2026-11-22`. An independent AI pre-screen marked every item confirmed, doubtful or wrong. Marc reviewed items one by one in the review page: the queue held every doubtful and wrong item and a seeded random sample of the confirmed ones (seed 164, 15 % per dataset; `evals/review/sample.json`). Over all datasets he decided 229 sampled confirmed items with 0 errors (`evals/review/decisions.jsonl`); the table gives each dataset's own human sample, which can be 0, and the prescreen provenance string names it. Marc's policy of 22 November 2026: pre-screen-confirmed items outside the sample are accepted as correct on the strength of the sample he checked; every other item he did not decide, and every item he marked unsure, is decided by an independent second-check agent (`evals/review/agent-check/decisions-*.jsonl`). An item whose final decision is unsure is not marked reviewed.

Provenance, per item in `evals/review/provenance.jsonl`:

- **human** — `human:marc`: Marc decided the item himself;
- **prescreen** — `agent:prescreen, accepted by marc's policy (owner-2026-11-22); human sample in this dataset: N checked, E errors`: confirmed by the pre-screen, outside the sample, accepted by the policy — no person looked at this item;
- **second-check** — `agent:second-check, per marc's policy (owner-2026-11-22)`: decided by the second-check agent (`evals/review/agent-check/`) — no person looked at this item;
- **rule** — `policy owner-2026-11-22 consistency rule`: decided by the policy's consistency rule (`evals/review/policy/owner-2026-11-22.json`), whatever any single check said;
- **restated** — `<class>, wording restated by policy owner-2026-11-22`: decided as its class says; the policy then restated its wording (a graph-gold trap now names the one target it must not have).

Policy addendum: Consistency rule: an optional answer that the task wording explicitly excludes (a Starlette or library call, a class instantiation where the task excludes instantiations, a deprecated decoy documented as not used internally, a function passed as a value where the task counts that as a reference) is removed from the optional list everywhere in the task set, not only where a second check flagged it; genuine near-misses (recursion or self-calls, thin wrappers, callers on the real path) stay optional. A graph-gold trap whose call site has a real CALLS edge is restated as 'must not target <X>' instead of being deleted.

`reviewed_by` of a unit (query, language file, held-out set, task) names the classes of its items: `human:marc` only when Marc decided every item, otherwise the class, or `mixed, policy owner-2026-11-22: …` with a count per class. It stays `null` where an item is unsure, open or needs a fix by hand. `real-repo-eval` reports retrieval queries as human-reviewed, agent-verified under owner policy, or unreviewed; only `human:` provenance counts as human.

| dataset | items | human | prescreen | second-check | rule | restated | wrong: fixed / by hand | unsure | open | units marked | human sample in this dataset: checked / errors |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| agent-tasks | 190 | 16 | 152 | 20 | 2 | 0 | 10 / 0 | 0 | 0 | 30 of 30 | 16 / 0 |
| agent-tasks-hard | 470 | 0 | 398 | 72 | 0 | 0 | 1 / 0 | 0 | 0 | 20 of 20 | 0 / 0 |
| agent-tasks-hard-v1 | 250 | 0 | 212 | 38 | 0 | 0 | 0 / 0 | 0 | 0 | 20 of 20 | 0 / 0 |
| agent-tasks-outcome | 299 | 0 | 254 | 45 | 0 | 0 | 0 / 0 | 0 | 0 | 12 of 12 | 0 / 0 |

Checked against the reviewed labels and current:

- evals/agent/results/2026-11-20-calibration — grades checked against the reviewed tasks/tasks-hard-v1.json: unchanged (its meta records the sha256 of the file before review metadata was added)
- evals/agent/results/2026-11-20-variants — graded against the reviewed tasks/tasks-hard.json
- evals/agent/results/2026-11-21-outcome — grades checked against the reviewed tasks/tasks-outcome.json: unchanged (its meta records the sha256 of the file before review metadata was added)
- evals/agent/results/2026-11-21-outcome-calibration — grades checked against the reviewed tasks/tasks-outcome.json: unchanged (its meta records the sha256 of the file before review metadata was added)
- evals/agent/results/2026-11-21-outcome-calibration-t4 — grades checked against the reviewed tasks/tasks-outcome.json: unchanged (its meta records the sha256 of the file before review metadata was added)
- evals/agent/results/2026-11-12-pilot — graded against the reviewed tasks/tasks.json
<!-- label-review:end -->
