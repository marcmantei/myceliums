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
schema check, and a check that every committed `REPORT.md` / `TOOLSEL.md` is
exactly what the generator produces from the committed results.

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
| `C` | A + `myc setup-claude --scope project --no-agent` run in the working copy: routing block in `CLAUDE.md`; SessionStart, PostToolUse and PreToolUse grep-redirect hooks in `.claude/settings.json` |
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
the project `CLAUDE.md` (routing block) reaches the context in C and E and
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
