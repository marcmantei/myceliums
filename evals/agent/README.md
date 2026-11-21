# Agent benchmark

Does myceliums help a coding agent? This directory measures it directly: a
headless Claude Code agent answers code-understanding questions about pinned
open-source repositories, once with only its built-in read and search tools
and once with the myceliums MCP server added. Answers are graded
automatically against gold sets derived with grep, reading and compiler
tooling and verified by an agent (human review still pending). A second, smaller eval checks
whether a model picks the right myceliums tool for a request.

Results: [`results/`](results/). Each run directory holds the compact per-session
results (`results.jsonl`), the run metadata (`meta.json`) and the generated
report (`REPORT.md`); raw transcripts stay in the git-ignored `runs/`.

**Pilot, 12 November 2026** (myc 0.3.2, Claude Code 2.1.322 with
`claude-sonnet-5-5` at medium effort, 180 graded sessions):
[report](results/2026-11-12-pilot/REPORT.md). Both conditions were at the
ceiling (success 100 % without, 99 % with myceliums; mean F1 0.997 vs 0.991,
difference −0.006 [−0.018, +0.003]). The agent called a myceliums tool in 0
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

**Outcome eval, 21 November 2026** (#153; myc at `951d041ff9e0`, Claude
Code 2.1.330, 22 graded sessions on 11 multi-hop Django tasks, one repetition):
[report](results/2026-11-21-outcome/REPORT.md), [method and
summary](#outcome-eval-153). With a budget of three rounds of tool calls,
the agent without myceliums succeeded on 4 of 11 tasks (36 %), the agent
with what `myc setup-claude --no-agent` installs (C) on 11 of 11; success
+64 pp [+36, +91], mean F1 +0.63 [+0.35, +0.90]. Every failure without
myceliums was a session that ran out of turns before answering; with seven
rounds the agent without myceliums had solved 5 of 6 of these tasks. C also
cost 35 % less [24 %, 46 %] and took half the wall time. A pilot: n = 11
tasks, wide intervals, one repository and model.

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

The labels were written and verified by an AI agent (Claude), not reviewed by
a human. Spot-check them before relying on a single task's result.

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
| Mean F1 (difference to A) | 0.984 | 0.983 (−0.001 [−0.027, +0.022]) | 0.989 (+0.005 [−0.009, +0.021]) |
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
hooks), the built-in `Read`, `Grep` and `Glob`. `C`: the MCP server with its
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

**Calibration** (10 sessions, `none`): at `--max-turns 8`, 5 of 6 tasks
succeeded ([report](results/2026-11-21-outcome-calibration/REPORT.md); the
agent used two to five rounds of tool calls), which is the ceiling; at
`--max-turns 4`, 1 of 4 ([report](results/2026-11-21-outcome-calibration-t4/REPORT.md)).
The main run uses 4.

**Results, 21 November 2026** (myc 0.3.2 at `951d041ff9e0`, Claude Code
2.1.330; 22 graded sessions, 11 tasks × 2 conditions × 1 repetition;
[report](results/2026-11-21-outcome/REPORT.md)). Means over tasks, brackets
95 % paired bootstrap intervals over the 11 tasks:

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
* **It is an efficiency effect.** With seven rounds (calibration) the agent
  without myceliums solved 5 of 6 of these tasks. myceliums gets
  the same answer in one call instead of four or five rounds; a budget turns
  that into success or failure. On the four blast-radius tasks both
  conditions solved, C used 2.25 tool calls instead of 4.25, 12.5 s instead
  of 21.8 s and 0.047 USD instead of 0.069 (n = 4, descriptive).
* **Cost fell, input tokens did not change measurably.** C's first request is
  5,418 tokens larger (the tool schemas, the routing block, the context
  line), which the shorter sessions offset: input tokens 0.93× [0.74×,
  1.17×], output tokens and cost clearly lower.

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

* Outcome eval (#153): a pilot. One repetition, 11 paired tasks, one
  repository, one language, one model: the intervals are wide and say
  direction and rough size. The effect is measured under a tight budget
  (three rounds of tool calls); with seven rounds the agent without
  myceliums was at the ceiling, so outside a budget the gain is speed and
  cost, not accuracy. The gold sets follow static call rules that a call
  graph computes by construction, while an agent with grep has to emulate
  them; the tasks are structural questions of the kind myceliums is built
  for, not a sample of developer questions. The task set was calibrated on
  10 of its 12 tasks, and one task was not run.
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
* Gold sets were produced and checked by an agent with compiler tooling, not
  by a human reviewer. One label was amended after the pilot started (an
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
