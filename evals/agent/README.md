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
recent spring-framework commits. **Pilot run, decision recorded, main run not**: the pilot and the
first pre-registration were fixed before the first session; the recall was redefined after the pilot in arm `none`, without using any outcome of C or D (*Pre-registration*). The warm-up has run and all 20 prechecks are valid.

**Mining** ([`groundtruth/commit_replay.py`](groundtruth/commit_replay.py)
`mine`, over a blobless clone with history): non-merge commits on `main` since
2025-10-01 with at most 25 files (at most two renamed), no dependency bump,
polish, copyright, revert or release subject, changes to main code and to
tests, at least two build modules touched (`framework-docs` counts), and a
parent that builds with the Java 25 toolchain: **151 candidates**. 34 were
picked for a public API or behaviour change a description can state
(no pure refactoring or deprecation), and stratified by files, code modules
and docs; 20 are the task set, 9 are reserves
(`tasks/tasks-commit-replay-reserve.json`) that replace tasks whose precheck
fails. **Tests that listen on all interfaces:** a static closure over the test sources that each task's `--tests` classes reach (imports,
same package) finds server sockets in two, both on `0.0.0.0` (`AbstractHttpServer`): `fn-request-bind-pathvars-headers`, replaced by
`form-converter-map-payloads` (same size, modules and docs strata; chosen before its precheck ran), and the reserve `problemdetail-no-instance-from-path`, dropped.
It does not see sockets opened by main code or libraries alone, so it is no proof that the 20 open none, and it was not confirmed: the socket samples of the prechecks saw loopback TCP listeners only, but during the grading of `client-builders-...` on 24 November one listener on all interfaces appeared (below; process and port were not logged, [`groundtruth/watch_run.py`](groundtruth/watch_run.py) logs them from now on). The sandboxes do not stop inbound connections.

| Task | Commit | Files (code/tests/docs) | Modules whose code changes | Comment-only files | Docs | Weakly tested | Primary files |
|---|---|---:|---:|---:|---|---|---:|
| `spel-simple-context-restricted-conversion` | d41495720c | 5 (2/1/2) | 1 | 1 | yes |  | 1 |
| `observation-record-handled-exception` | 297c74d29c | 20 (8/9/3) | 3 |  | yes | yes | 2 |
| `beans-max-nested-path-depth` | 8ab525f6f0 | 12 (6/5/1) | 2 |  | yes |  | 3 |
| `form-converter-map-payloads` | ab8de8ec4b | 6 (4/1/1) | 2 |  | yes |  | 1 |
| `jsonl-media-type-support` | 22e4d84993 | 16 (11/2/3) | 2 | 1 | yes | yes | 1 |
| `test-context-cache-pause-mode` | 9711db787e | 9 (4/3/2) | 1 | 1 | yes |  | 3 |
| `retry-max-attempts-to-max-retries` | 24590092ef | 13 (6/6/1) | 2 | 1 | yes | yes | 2 |
| `jdbcclient-batch-entry-dsl` | 833ab4d48f | 6 (2/3/1) | 1 |  | yes |  | 1 |
| `type-mismatch-required-type-arg` | 4847ee80b0 | 3 (1/1/1) | 1 |  | yes |  | 1 |
| `servlet-response-ignore-body-flush` | e0b54e244e | 5 (3/1/1) | 2 |  | yes | yes | 1 |
| `content-negotiation-no-fallback-mapping` | 7fbdbd6eb7 | 4 (2/2/0) | 2 |  | no |  | 1 |
| `client-builders-deep-copy-default-headers-cookies` | 4cf6335372 | 6 (3/3/0) | 3 |  | no |  | 1 |
| `http-range-honor-if-range` | 8ab5332639 | 11 (5/6/0) | 2 |  | no |  | 1 |
| `error-response-interceptor-with-request` | d70220eb5d | 11 (7/4/0) | 3 |  | no |  | 1 |
| `mime-type-parameter-names-case-insensitive` | 2028eb3694 | 4 (2/2/0) | 2 |  | no |  | 1 |
| `deferred-bean-registrar-contains-bean` | 7502b92392 | 14 (7/7/0) | 2 | 1 | no | yes | 2 |
| `api-version-deprecation-handler-handler-arg` | b128f59714 | 12 (10/2/0) | 3 |  | no | yes | 2 |
| `bean-provider-parameterized-type-reference` | 2e4ca41382 | 7 (6/1/0) | 3 |  | no | yes | 1 |
| `message-source-default-charset` | 02af3e7a17 | 5 (4/1/0) | 1 | 1 | no | yes | 1 |
| `retryable-method-retry-event` | 92a43c007a | 10 (6/4/0) | 2 | 2 | no |  | 3 |
Spread: 6 small (≤ 5 files), 10 medium (6-12), 4 large (13-20); main code changes
in one module 5, two 10, three or more 5; 10 with docs. The split is computed from
the diffs: a file in which only comments, Javadoc or imports change does not count for a
module (seven tasks have such gold files; the mining filter and the first versions of this
table counted them). Grading reports these files and Gradle scripts as their own
kinds of dependent (`comments`, `build`), in no other kind and not among the forgotten, and the code recall also without them (`code_changed`).

**Classifications corrected by rule** (24 November, after the pilot; the rules are code in `commit_replay.py` and `editgrade.py` and were run over all 20 tasks and the reserves; they change three tasks, no reserve):

| Rule | Task and file | Before | Now | Why |
|---|---|---|---|---|
| a file git calls renamed (50 % similar) is one dependent (`renames`) | `retry-max-attempts-...`: `MaxAttemptsRetryPolicyTests` → `MaxRetriesRetryPolicyTests` | two dependents, deleted + added | one (`equivalents`) | editing the old file in place and renaming it are the same change; the only rename in the 29 |
| an XML schema in which only documentation elements change is comment-only (`schema_text`) | `jsonl-...`: `spring-mvc.xsd` | remote code | `comments` | the diff is one sentence of `xsd:documentation`; the only schema in the 29 |
| the documentation module is docs, its sample code too (`file_kind`) | `observation-...`: two `framework-docs/src/main/java/.../UserController.java` | remote code | docs | the pages include the samples; counted either way, so (a) does not change, the split does (docs 3, code modules 3). Mining ran with the old rule |

Looked for and not found: generated files (by path and by marker), files that change in white space, licence header or imports only beyond those already marked (Javadoc imports in `deferred-...`), other renames. The Gradle script of `observation-...` was already apart (`build`).
Not touched, because no rule tells them apart: annotations and casts that a signature change needs (`form-converter-...`) are code.

**What the test commands enforce** (an agent's reading of the diffs and tests, nothing was run): in 8 tasks at least one gold dependent
can stay as at the parent and the command still passes (`weak_test` in the task file says which; `bean-provider-...` is one of them because only the
demanded abstract method enforces its five, and `api-version-...` because no test checks which handler is passed); `untested` notes other unchecked behaviour. The report gives success and file recall with
and without those 8. `spel-simple-...`, `test-context-...` and `type-mismatch-...` have no code dependent to find at all, so they measure docs and
tests only. Where a part of the change has several valid places (`equivalents`: the reader or the post-processor in `deferred-bean-registrar-...`), they count as one, everywhere: found when either is edited, one in the denominator. In `beans-max-nested-path-depth`
three dependents can go into a primary file, which every solution edits: the file list cannot tell, so they are reported apart (`either`) and neither counted nor forgotten.

**Descriptions** were drafted by agents from the commit message, the linked
issue and the diff ([`groundtruth/commit_replay_spec.json`](groundtruth/commit_replay_spec.json),
rules: an issue-style change request that names every new public name the
commit's tests need, may name the type of the core change, and must not name
files, test classes, dependent classes, modules, docs or tests; a unit test checks the
20 for gold class names other than the primary ones, with `@Retryable`, the feature's own
name, as the one exception). A second
agent reviewed all 34 for leakage and solvability against the test diff (it
split them over four sub-agents and itself re-checked nine): 27 ok, 7 to fix.
Six were replaced by the reviewer's rewrite (two of them among the 20); the
rewrites went to a third check. A third agent re-reviewed those six: two ok, four
changed again (a class name that also names its counterpart, a wrong fact about the server
conventions, two wrong facts about the overflow, a call-site scope that was
not stated, and for `bean-provider-parameterized-type-reference`, one of the 20,
that the method is abstract as in the commit; without it a one-line default
method would have passed). With it the implementations are compile errors, so this task says little
about forgotten dependents, and only because the test command compiles every changed module
(before the PR review it did not). The PR review checked these four texts. It also found two that named a gold class
(`DefaultConversionService`, `@Import`; removed, see the spec) and a weak test:
`cache-manager-reset-caches` passed with an interface default method alone while its description made the
gold overrides optional; it was replaced by `retryable-method-retry-event` (same strata). A fourth agent checked the two reworded texts and the new one: no leak, nothing missing for the tests,
but wrong facts in all three (for `deferred-bean-registrar-contains-bean` one that would have led to a file the commit does not
change); all three now have its text, which the second PR review checked, and `weak_test` in the task file lists the gold
dependents their test commands do not enforce. The seventh, a reserve, had to name a
dependent (a Kotlin extension its module's tests compile against) and was
dropped. The PR reviews checked the texts of the 20 that were in the set then; the final review checked `form-converter-...`, which came in last. The reserve
`resource-loader-...` was rewritten after it (its description had demanded nothing of three gold files) and is not re-reviewed. No human has reviewed the descriptions.
Scope words an issue would use ("for both Spring MVC and WebFlux") are allowed
and noted in the spec.

**What the build derives** (`commit_replay.py build`): one pin per task in the
task file's own `repos` (the parent; `gold_commit` the replayed commit),
`gold_files` (the commit's name-status, renames as delete + add and as one `equivalents` group),
`forced_tests` (below), `test_overlay` (the commit's test files, fixtures included; for
`observation-record-handled-exception` also the Gradle script that gives its overlaid tests
their test dependency), the test command (`./gradlew --offline --no-scan --no-build-cache`, main and
test compilation of **every module the commit changes sources in**, then `:module:test --tests ...`
over the changed test classes; three tasks narrow the tests, see the spec), the Gradle rule of the hook
(`allowed_commands`: the three flags, options and tasks by exact name, because Gradle expands abbreviations
such as `bSPP`, and `--tests` with a class or method pattern) and the strata.

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
* `forced-tests` ([`aeval/forced.py`](aeval/forced.py)) measures which of the commit's test changes the change itself forces (below).
* The repository's `session_env` (`GRADLE_OPTS`: no Gradle daemon, Kotlin
  compiler in process, so no daemon outlives a session) and the caller's
  `JAVA_HOME` (`env_from_caller`; `$JAVA_HOME/bin` first on `PATH`) reach
  the session, the test command and the precheck.
* Files the harness wrote before the session (setup-claude's `.claude/` files and
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
export AEVAL_WORK_ROOT=/private/tmp/aeval182 # the path the derived indexes were built for (index plan below); another one matches no index
export AEVAL_PROTECT=/private/tmp/aeval183   # scratch of the preparation (contexts, logs)
export JAVA_HOME=$HOME/.cache/myc-java/jdk-ipv4/Contents/Home   # JDK 25, the copy of make_jdk_ipv4.py: offline only; the warm-up uses the original
T=evals/agent/tasks/tasks-commit-replay.json
python3 evals/agent/agent_eval.py --tasks $T fetch           # 20 parents (~100 MB each) + gold files
python3 evals/agent/agent_eval.py gradle-cache               # after the warm-up: freeze ~/.gradle's dependencies
python3 evals/agent/agent_eval.py --tasks $T precheck-edit --gold-runs 2 --sandbox   # needs a warm Gradle cache: the commands are offline
python3 evals/agent/agent_eval.py --tasks $T check-tasks
python3 evals/agent/agent_eval.py --tasks $T forced-tests --logs DIR   # once, about an hour; the record is committed. Then `commit_replay.py build` and `dependents`.
MYC=$HOME/.cache/myceliums-agent-eval/bin/myc-787920c
# Once. The 20 derived indexes exist; `index` refuses to delete a derived index, `--replace` rebuilds them (4,942 s for the 20 last time).
# python3 evals/agent/agent_eval.py --tasks $T index --myc $MYC --models-dir ~/.cache/myceliums-eval-models --base $BASE_HOME
# Before the main run: one session of arm none per task, then the registered decision (recorded once).
python3 evals/agent/agent_eval.py --tasks $T run --label 2026-11-xx-none20 --conditions none --rep 1 \
    --profile realistic --bash --timeout 2400 --concurrency 1 --myc $MYC --models-dir ~/.cache/myceliums-eval-models
python3 evals/agent/agent_eval.py --tasks $T ceiling 2026-11-xx-none20 --write
python3 evals/agent/agent_eval.py --tasks $T run --label 2026-11-xx-commit-replay --conditions none,C,D \
    --profile realistic --bash --timeout 2400 --concurrency 1 --myc $MYC --models-dir ~/.cache/myceliums-eval-models
```

Arms: `none` (no myceliums: the issue's arm A, the baseline), C (what `myc setup-claude --no-agent` installs) and D
(myceliums only; search taken away). The first versions of this plan said `A,C,D`: in the harness `A` is the MCP server alone, so that
run would have had no baseline. Every arm's Bash is the hook's allowlist (below; file operations, git, `./gradlew`
test runs); every arm has `Bash`, `Edit` and `Write`. Realistic profile:
100-turn cap not stated in the prompt, 3 repetitions and the interleaved schedule of #182
(repetition-major, tasks shuffled, the three arms adjacent per task with
their order counterbalanced). Each session gets a fresh copy of its task's
parent at one shared path (`work_name`: `spring-framework` under `AEVAL_WORK_ROOT`) and a
fresh copy of that parent's index. The path is shared because of the index
(below); its price is that **sessions run one at a time**.

#### Pre-registration (24 November 2026; first before the pilot, outcome (a) redefined the same day after it)

Arms `none`, C, D; 20 tasks × 3 repetitions × 3 arms = 180 sessions; the task is the unit of analysis (its repetitions averaged first). Every
final session counts, also one that ran into the time or turn limit; only infrastructure failures (API errors, a killed CLI, a usage limit) are run again.

**What changed, when and why.** Until the pilot, (a) was the recall over *all* dependents: docs, remote code and every test file of the commit. The pilot's five sessions of arm `none` (24 November, 08:25-08:52) and the independent
review of its pull request showed that this mostly measures new tests the commit's author added and nobody asked for: a perfect engineer who adds none would average 0.64 over the 20 tasks (the review's number; computed here: 0.642, with the corrections above and all measured forced tests counted).
On 24 November 2026, after those sessions and **without using any outcome of C or D, and before any C or D session that counts**, (a) was redefined to what the change needs; the old (a) stays as a named secondary outcome. Corrections, forced tests and rules were made by an agent that has seen no C or D transcript, result or run directory
(the agent that ran the pilot's two pipeline checks in C and D had; what the reviewer and the coordinator saw of them is not known to it). **The bias this brings:** (a) now discards an arm's effect on writing tests nobody asked for, in either direction; only the secondary outcomes show it.
It was chosen knowing that arm `none` missed most of those tests, and it moves `none` towards the ceiling (in the five pilot sessions, with the corrected gold, from 0.57 to 0.80), so it makes "tokens alone" (decision rule) more likely. Ten tasks have at most two counted dependents, five of them one, so per task (a) is coarse and those five carry 25 % of the mean (*Sensitivity*: units pooled).

* **Outcome (a): recall over remote code + docs + forced tests**: the share of the replayed commit's code files outside the directories of the primary files, docs files and forced test files that the session changed. **Forced** has a measured and a judged part: that a test file does not compile or fails is **measured** (*Forced tests*, below); whether a failing one counts, because the task text states what it contradicts, and whether a code or docs file is affected at all are **judged** (*Is the file affected*, below). One that does not compile counts without a judgement.
  **Not counted:** every other test file of the commit (`new_tests`), also one that fails without the text stating why; code and docs the stated change does not affect (`optional`, *Is the file affected*); files in which only comments, imports or documentation text change (`comments`; docs files never are), Gradle scripts, code beside a primary file, and files whose part can go into a primary file (`either`). Equivalent places are one dependent, found when any of them is edited.
  Grading and report use one function (`editgrade.dependents`). The numbers per task are fixed: the table below and [`groundtruth/dependents_183.json`](groundtruth/dependents_183.json) (a unit test compares them with the task file).
* **Outcome (b): tokens per session**, unsolved sessions included: input with cache plus output; the CLI's cost estimate is shown beside it and not tested.
  A session without a result event (timeout) counts with the calls its transcript shows, its output as a lower bound; the report says how many.
* **Decision rule, before the main run:** one session of arm `none` per task (20 sessions, a label of their own, not pooled with the pilot or the main run; all 20 final sessions count, timeouts and turn limits included). The **unrounded** mean over the 20 tasks of their (a) decides, with the threshold 0.90 − 1e-9; it is shown with three decimals:
  **at or above it, tokens become the sole primary outcome** (Holm over the three pairs) and (a) is a **harm check**, reported for each pair as "harm detected" (the 95 % interval of the difference lies entirely below zero) or "harm not detected", always with the number of tasks worse/better and **two bounds on the loss not excluded**: the interval's lower bound, and the exact one-sided 95 % bound (Clopper-Pearson) on the share of worse tasks, each of which loses at most everything (none of 20 worse: 13.9 %; one: 21.6 %).
  The interval alone cannot contain a loss where no task is worse, and choosing between the two by the data would let the bound shrink when a task gets worse; both together never do (a test). "Not detected" is never evidence of none.
  **Below it, (a) and (b) stay co-primary** (Holm over six tests: three pairs × two outcomes). `agent_eval.py ceiling LABEL --write` computes it and records it once ([`groundtruth/decision_183.json`](groundtruth/decision_183.json): the mean, each task's recall, the sessions' statuses, the task file's sha256, the Claude Code version and the model, of which there must be one each). **`report` refuses without that record**, or when the task file is another one, and says when the run's CLI or model differ from it. These 20 sessions change nothing else. **Whether a main run happens at all is Marc's decision.**
* **Secondary outcomes**, not tested: the **recall with new tests (the outcome registered first)**: docs + remote code (the optional ones included) + all test files; the share of the new tests alone; docs, remote and forced-test recall alone; tests pass, which is also success (an edit session succeeds when the task's test command passes, nothing else); tokens and cost per solved task; precision (share of the changed files that the commit changed); API calls; wall time, as measured and without API stalls (*Refused reads and API stalls*).
* **Comparisons:** C vs `none`, D vs `none`, C vs D. Every difference is **first minus second**: a positive recall difference favours the first arm, a negative token difference favours it.
  Paired percentile bootstrap over the tasks (10,000 resamples, seed 0), two-sided p by inverting it; Holm as above at 5 %. With 20 tasks the percentile bootstrap is slightly liberal. Intervals are 95 % and not corrected.
* **Detectable difference** (two-sided 5 %, power 80 %, 20 tasks, normal approximation): at an SD of 0.3 of the tasks' paired recall differences 0.19 for one test, 0.22 with Bonferroni over three pairs, 0.23 for the first Holm step over six tests. The SD is a guess; the pilot's (0.45, below) is one of five tasks and of single sessions, not of differences. A difference that is not significant is not evidence of none.
* **Sensitivity**, not tested: without the 8 weakly tested tasks; **with** the comment-only files that lie outside the primary files' directories; only the 6 tasks whose parent has an AGENTS.md (`strata.agents_md`); the recall over all units **pooled** (a task weighs as many as it has counted dependents).
* **Validity limits:** Spring Framework is public and its commits are likely in the model's training data, in every arm alike; agents have no scripting (Bash is an allowlist without interpreters) and work in a sandbox, so they are not
  agents as in daily use; D has its search tools blocked, so C vs D and D vs `none` measure myceliums *instead of* search, C vs `none` beside it; one project, one language, one model.
  Forced by compilation is relative to the commit's implementation too: another correct one (an alias kept) may leave such a test green, so an agent can be right and miss it. Which files a change affects is an agent's reading (*Is the file affected*).

`agent_eval.py report` computes exactly these (tests on synthetic sessions: known differences, ties, an arm that solves nothing, missing sessions, the decision on both sides of 0.90 and what it refuses, the report without a decision, the harm check with and without a worse task, units pooled, an arm that touches new tests only).

**Forced tests** ([`aeval/forced.py`](aeval/forced.py), `agent_eval.py forced-tests`; record: [`groundtruth/forced_tests.json`](groundtruth/forced_tests.json)). The commit's non-test files are written over the parent. A test file of the commit is forced when its **parent version** does not compile there or a test in it fails there; a test file that is new in the commit never is, and a renamed one counts as the deleted one.
All modules the commit changes sources in are compiled; files with errors are replaced by the commit's version and the compilation repeated (one error can hide another), then the tests of the others run in their parent version (an abstract test class through its subclasses). Failing tests ran twice and had to fail alike. Two controls per file: its tests pass on the untouched parent and, in the commit's version, on the whole commit.
Anything else would have been *unclear*, listed and not used. Run on 24 November, 09:31-10:32, offline in the grading sandbox with the JDK copy and the frozen dependencies, one task at a time (84 Gradle runs, 60 minutes, load 20-147 from other work): **no unclear file, no infrastructure failure, no control that failed, no difference between the two runs of the 4 failing test runs**; no connection but loopback in the socket samples.
Per file the record has the command, the outcome, the first errors or failing tests, the tests run in the controls and the hashes of both versions; per task the commits, every command with exit code, time and the hash of its log, and a hash over the sources it depends on (a test compares it where the checkouts exist). To run one again: the run block's `forced-tests --task ID --out NEW.json`.

Test files by outcome (compile: does not compile; fails; passes: not forced; new), then **the registered dependents by type** (units: equivalents count once, so the rename in `retry-...` is one; (a) = remote + docs + forced; the types after it are not counted; *fails* that the text does not state count as new tests, see below):

| Task | Compile | Fails | Passes | New | Remote code | Docs | Forced tests | **(a)** | New tests | Optional | Local | Comments | Build | Either |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `spel-simple-context-restricted-conversion` | 0 | 0 | 1 | 0 | 0 | 2 | 0 | **2** | 1 | 0 | 0 | 1 | 0 | 0 |
| `observation-record-handled-exception` | 0 | 6 | 2 | 1 | 5 | 3 | 0 | **8** | 9 | 0 | 0 | 0 | 1 | 0 |
| `beans-max-nested-path-depth` | 0 | 0 | 5 | 0 | 0 | 1 | 0 | **1** | 5 | 0 | 0 | 0 | 0 | 3 |
| `form-converter-map-payloads` | 1 | 0 | 0 | 0 | 3 | 1 | 1 | **5** | 0 | 0 | 0 | 0 | 0 | 0 |
| `jsonl-media-type-support` | 0 | 1 | 1 | 0 | 9 | 0 | 1 | **10** | 1 | 3 | 0 | 1 | 0 | 0 |
| `test-context-cache-pause-mode` | 0 | 0 | 2 | 1 | 0 | 2 | 0 | **2** | 3 | 0 | 0 | 1 | 0 | 0 |
| `retry-max-attempts-to-max-retries` | 5 | 0 | 0 | 1 | 2 | 1 | 5 | **8** | 0 | 0 | 1 | 1 | 0 | 0 |
| `jdbcclient-batch-entry-dsl` | 3 | 0 | 0 | 0 | 0 | 1 | 3 | **4** | 0 | 0 | 1 | 0 | 0 | 0 |
| `type-mismatch-required-type-arg` | 0 | 0 | 1 | 0 | 0 | 1 | 0 | **1** | 1 | 0 | 0 | 0 | 0 | 0 |
| `servlet-response-ignore-body-flush` | 0 | 0 | 1 | 0 | 1 | 1 | 0 | **2** | 1 | 1 | 0 | 0 | 0 | 0 |
| `content-negotiation-no-fallback-mapping` | 0 | 0 | 2 | 0 | 1 | 0 | 0 | **1** | 2 | 0 | 0 | 0 | 0 | 0 |
| `client-builders-deep-copy-default-headers-cookies` | 0 | 0 | 3 | 0 | 2 | 0 | 0 | **2** | 3 | 0 | 0 | 0 | 0 | 0 |
| `http-range-honor-if-range` | 0 | 0 | 6 | 0 | 4 | 0 | 0 | **4** | 6 | 0 | 0 | 0 | 0 | 0 |
| `error-response-interceptor-with-request` | 0 | 0 | 4 | 0 | 6 | 0 | 0 | **6** | 4 | 0 | 0 | 0 | 0 | 0 |
| `mime-type-parameter-names-case-insensitive` | 0 | 0 | 2 | 0 | 1 | 0 | 0 | **1** | 2 | 0 | 0 | 0 | 0 | 0 |
| `deferred-bean-registrar-contains-bean` | 0 | 0 | 2 | 5 | 3 | 0 | 0 | **3** | 7 | 0 | 0 | 1 | 0 | 0 |
| `api-version-deprecation-handler-handler-arg` | 2 | 0 | 0 | 0 | 2 | 0 | 2 | **4** | 0 | 0 | 6 | 0 | 0 | 0 |
| `bean-provider-parameterized-type-reference` | 0 | 0 | 1 | 0 | 5 | 0 | 0 | **5** | 1 | 0 | 0 | 0 | 0 | 0 |
| `message-source-default-charset` | 0 | 1 | 0 | 0 | 0 | 0 | 1 | **1** | 0 | 0 | 2 | 1 | 0 | 0 |
| `retryable-method-retry-event` | 0 | 1 | 2 | 1 | 1 | 0 | 1 | **2** | 3 | 0 | 0 | 2 | 0 | 0 |
| **all** | 11 | 9 | 35 | 9 | 45 | 13 | 14 | **72** | 49 | 4 | 10 | 8 | 1 | 3 |

**Is the file affected by the stated change at all?** (24 November, after the review of the pull request that redefined (a); [`groundtruth/affected_183.json`](groundtruth/affected_183.json), one verdict per unit, an agent's with the reviewer's on three of its points; no session's result was used.) Forced is measured against the commit's implementation, and a commit may touch files its change does not need. Two rules:

* A test forced by a **failing assertion** counts only where the task text states the behaviour it contradicts (the verdict quotes it, a test looks for the quote in the text); one forced by **compilation** stays. Of nine failing files three stay (`jsonl`, `message-source`, `retryable`); the six of `observation-...` go to the new tests.
* A code or docs file that the stated change does not affect is `optional`: apart, in no recall but the outcome registered first. Four of the 62: `HttpHeadersReturnValueHandler` (named in `weak_test` before, corrected) and the three `jsonl` pages, which name a media type only as an example and stay true.

82 units asked, **72 count** (45 remote, 13 docs, 14 forced tests; the reviewer's 75 before the three pages). Three verdicts are borderline and say so: the two `UserController` samples and `MethodRetrySpec`. By the reviewer's estimate a perfect engineer who adds no new tests reached about 0.95 of the (a) before these two rules and about 0.99 after.

| Task | Files | Kind (measured) | Counts | Why |
|---|---|---|---|---|
| `spel-simple-context-restricted-conversion` | `appendix.adoc` | docs | yes | lists the Spring properties; the text adds one |
| `spel-simple-context-restricted-conversion` | `evaluation.adoc` | docs | yes | describes what SimpleEvaluationContext supports and its conversions, which change by default |
| `observation-record-handled-exception` | `observability.adoc` | docs | yes | tells users to set the error on the context and what is recorded, the practice the text calls insufficient |
| `observation-record-handled-exception` | `reactive/UserController.java` | docs | yes | sample of exactly the idiom the text calls insufficient (findCurrent(..).setError); borderline, it still compiles |
| `observation-record-handled-exception` | `servlet/UserController.java` | docs | yes | sample of exactly the idiom the text calls insufficient (findObservationContext(..).setError); borderline, it still compiles |
| `observation-record-handled-exception` | `ResponseEntityExceptionHandler.java` | remote | yes | records server errors itself; the text: WebFlux's own exception handling should use the new methods |
| `observation-record-handled-exception` | `ResponseEntityExceptionHandlerTests.java`, `ResponseStatusExceptionResolverTests.java`, `ExceptionHandlerExceptionResolverErrorViewTests.java`, `ExceptionHandlerExceptionResolverProblemDetailTests.java`, `ResponseEntityExceptionHandlerTests.java`, `DefaultHandlerExceptionResolverTests.java` | forced tests (fails) | **no** | The text demands that Observation.error is called once at completion; it does not say that the context stays without the error until then, which is what the parent's assertion (context.getError() right after handling) contradicts. Setting it at once and reporting at completion satisfies the text and leaves this test green. |
| `observation-record-handled-exception` | `ResponseStatusExceptionResolver.java`, `ExceptionHandlerExceptionResolver.java`, `ResponseEntityExceptionHandler.java`, `DefaultHandlerExceptionResolver.java` | remote | yes | records server errors itself; the text: Spring MVC's own exception handling should use the new methods |
| `beans-max-nested-path-depth` | `data-binding.adoc` | docs | yes | says nested properties can be set "to an unlimited depth", which becomes wrong |
| `form-converter-map-payloads` | `message-converters.adoc` | docs | yes | says form data is read from and written into a MultiValueMap, which is no longer all |
| `form-converter-map-payloads` | `ContentRequestMatchers.java`, `AbstractMockHttpServletRequestBuilder.java`, `FormContentFilter.java` | remote | yes | does not compile: the converter's read returns Object |
| `form-converter-map-payloads` | `FormHttpMessageConverterTests.java` | forced tests (compile) | yes | does not compile (measured) |
| `jsonl-media-type-support` | `webtestclient.adoc` | docs | **no** | names ndjson only as an example ("such as") of infinite streams; stays true |
| `jsonl-media-type-support` | `reactive-spring.adoc` | docs | **no** | names ndjson only as an example ("such as", "for example") of a streaming media type, in both places; both stay true |
| `jsonl-media-type-support` | `mvc-ann-async.adoc` | docs | **no** | names ndjson only as an example ("such as") of a streaming media type; stays true |
| `jsonl-media-type-support` | `GsonEncoder.java`, `Jackson2CodecSupport.java`, `Jackson2JsonEncoder.java`, `JacksonJsonDecoder.java`, `JacksonJsonEncoder.java`, `KotlinSerializationJsonDecoder.java`, `KotlinSerializationJsonEncoder.java`, `ProtobufJsonEncoder.java` | remote | yes | names application/x-ndjson among its defaults or streaming types; the text: jsonl everywhere it is supported, listed directly after it |
| `jsonl-media-type-support` | `HttpMessageWriterViewTests.java` | forced tests (fails) | yes | asserts the exact list of supported media types; the text puts jsonl into it (“The JSON codecs list it among their default supported MIME types, directly after application/x-ndjson”) |
| `jsonl-media-type-support` | `ReactiveTypeHandler.java` | remote | yes | the text: Spring MVC streams application/jsonl just as application/x-ndjson |
| `test-context-cache-pause-mode` | `appendix.adoc` | docs | yes | lists the Spring properties; the text adds one |
| `test-context-cache-pause-mode` | `caching.adoc` | docs | yes | describes the pausing of inactive contexts, which can now be turned off |
| `retry-max-attempts-to-max-retries` | `resilience.adoc` | docs | yes | uses the renamed names in text and samples |
| `retry-max-attempts-to-max-retries` | `AbstractRetryInterceptor.java` | remote | yes | calls the renamed builder method; does not compile |
| `retry-max-attempts-to-max-retries` | `MethodRetrySpec.java` | remote | yes | public record of the retry API with the concept in its component; borderline, the text does not list it |
| `retry-max-attempts-to-max-retries` | `ReactiveRetryInterceptorTests.java`, `RetryInterceptorTests.java`, `MaxAttemptsRetryPolicyTests.java`, `RetryPolicyTests.java`, `RetryTemplateTests.java` | forced tests (compile) | yes | does not compile (measured) |
| `jdbcclient-batch-entry-dsl` | `core.adoc` | docs | yes | describes and shows add(), which the text removes |
| `jdbcclient-batch-entry-dsl` | `JdbcClientIndexedParameterTests.java`, `JdbcClientIntegrationTests.java`, `JdbcClientNamedParameterTests.java` | forced tests (compile) | yes | does not compile (measured) |
| `type-mismatch-required-type-arg` | `mvc-ann-rest-exceptions.adoc` | docs | yes | lists the message arguments of TypeMismatchException; the text adds {2} |
| `servlet-response-ignore-body-flush` | `appendix.adoc` | docs | yes | lists the Spring properties; the text adds one |
| `servlet-response-ignore-body-flush` | `HttpHeadersReturnValueHandler.java` | remote | **no** | the parent's getBody() writes the headers and never flushes, so the change does not affect it (its flush() does not flush either as long as the body is unused) |
| `servlet-response-ignore-body-flush` | `StreamingResponseBodyReturnValueHandler.java` | remote | yes | flushes the body stream, which becomes a no-op; the text: streaming must keep working by flushing the response |
| `content-negotiation-no-fallback-mapping` | `ParameterContentTypeResolver.java` | remote | yes | the text: apply to the parameter-based strategy in WebFlux; it adds the fallback to its mappings |
| `client-builders-deep-copy-default-headers-cookies` | `DefaultWebTestClientBuilder.java`, `DefaultWebClientBuilder.java` | remote | yes | clone/mutate share the value lists; the text: the other HTTP client builders must be fixed in the same way |
| `http-range-honor-if-range` | `ResourceHttpMessageWriter.java`, `DefaultEntityResponseBuilder.java`, `AbstractMessageConverterMethodProcessor.java`, `ResourceHttpRequestHandler.java` | remote | yes | answers Range for resource responses itself; the text: consistently in MVC and WebFlux |
| `error-response-interceptor-with-request` | `AbstractMessageWriterResultHandler.java`, `ResponseBodyResultHandler.java`, `ResponseEntityResultHandler.java`, `AbstractMessageConverterMethodProcessor.java`, `HttpEntityMethodProcessor.java`, `RequestResponseBodyMethodProcessor.java` | remote | yes | calls the interceptors; the text: all framework calls use the request-aware variant and pass the current request, which only the caller has |
| `mime-type-parameter-names-case-insensitive` | `MediaType.java` | remote | yes | compares the quality factor's name; named in the text |
| `deferred-bean-registrar-contains-bean` | `BeanRegistryAdapter.java` | remote | yes | implements BeanRegistry, which gets three methods |
| `deferred-bean-registrar-contains-bean` | `ConfigurationClassBeanDefinitionReader.java` | remote | yes | one of the two places where imported registrars run; the text: deferred ones at the end |
| `deferred-bean-registrar-contains-bean` | `GenericApplicationContext.java` | remote | yes | runs programmatically registered registrars; the text: deferred ones at the end |
| `api-version-deprecation-handler-handler-arg` | `StandardApiVersionDeprecationHandlerTests.java`, `StandardApiVersionDeprecationHandlerTests.java` | forced tests (compile) | yes | does not compile (measured) |
| `api-version-deprecation-handler-handler-arg` | `AbstractHandlerMapping.java`, `AbstractHandlerMapping.java` | remote | yes | calls deprecation handling and is the only place that has the handler |
| `bean-provider-parameterized-type-reference` | `DefaultListableBeanFactory.java`, `StaticListableBeanFactory.java`, `AbstractApplicationContext.java`, `SimpleJndiBeanFactory.java`, `StubWebApplicationContext.java` | remote | yes | implements BeanFactory, and the text makes the new method abstract |
| `message-source-default-charset` | `ResourceBundleMessageSourceTests.java` | forced tests (fails) | yes | asserts the late NoSuchMessageException for an invalid charset name; the text says it goes away (“an invalid charset name now fails at configuration time with an UnsupportedCharsetException rather than surfacing later as a NoSuchMessageException”) |
| `retryable-method-retry-event` | `RetryAnnotationBeanPostProcessor.java` | remote | yes | creates the interceptor and is the bean that can be given the application's event publisher |
| `retryable-method-retry-event` | `RetryTemplateTests.java` | forced tests (fails) | yes | asserts the old interruption message; the text gives the new one (“should carry the message "Interrupted during back-off for retryable operation '<name>'; aborting execution"”) |

**Refused reads and API stalls.** In the pilot's `none` sessions Claude Code's own permission layer ("don't ask" mode), not the hook, refused two Bash calls that only read working-copy files. Both combine `cd` with relative paths, one with `sed -n` and a second `cd`, the other with a redirection to `/dev/null`; a third call with two `cd` and relative `grep`s ran, and no read without `cd` used either, so **the cause is likely but not proven**.
Chosen: one sentence more in the build note, so in the prompt of **all** arms: "In shell commands, name files by paths relative to the working copy and do not use `cd`." It asks for no docs and no tests (a test checks the prompt around the description for those words). The hook still lets `cd` through, but its refusal text no longer lists it as allowed. The settings were **not** changed: `autoAllowBashIfSandboxed` might let such calls through, but it removes the check that stands behind the hook and would need a new probe of the sandbox with the CLI.
The audit counts per session `claude_refusals` (the CLI's `permission_denied` events or the refusal's text), those with a `cd` (`claude_refusals_cd`) and the calls themselves, so that the sessions to come show whether the sentence helps and whether `cd` is the cause; and `api_errors`, of them `api_stalls` (`stream_idle_timeout`) with `api_stall_s`, the seconds since the last event before the stall. Wall times are reported with and without them (the figure is an upper bound: it includes the time a good answer would have taken to begin).
On the five `none` transcripts it finds the two refusals, the one stall (180.6 s, `beans-max-...`), and leaves every other number of the audit as it was.

#### Pilot

**Arm `none` only**, one session of each of 5 tasks with a valid precheck, one per stratum (size × docs) among the 8 valid ones, drawn with seed 183 and recorded in [`groundtruth/pilot_183.json`](groundtruth/pilot_183.json) (since the warm-up all 20 are valid; the draw stays as it was):
`retry-max-attempts-to-max-retries` (L, docs), `api-version-deprecation-handler-handler-arg` (M, no docs), `beans-max-nested-path-depth` (M, docs), `mime-type-parameter-names-case-insensitive` (S, no docs), `type-mismatch-required-type-arg` (S, docs). Plus one session in C (`api-version-deprecation-handler-handler-arg`) and one in D (`retry-max-attempts-to-max-retries`), **only as a check of the pipeline**: did the session start, reach `myc`, build and get graded. Their outcomes
(files, tests, tokens) are not reported, and `agent_eval.py pilot` refuses sessions of other arms.

* It may change only **harness errors, wrong gold, or unclear wording**, each with a written rationale and a re-check (precheck, leakage review). It **never drops or changes a task because of how an arm did**.
* Its sessions are **never pooled** with the main run, which runs all 20 tasks afresh.
* Afterwards cost and time of the main run are estimated again: the mean of the five × 180 sessions. **Stop and report** when that is above **$288 or 24 h**. (The estimate below has 36 h in its middle row, so the time limit is expected to bite unless sessions are shorter than 8 minutes.)
* **Ceiling:** a mean dependent recall of 0.90 or more in `none`. Then say so; the tasks are not changed. (The decision that follows from it is made on 20 other sessions: *Pre-registration*, decision rule.)

The commands are [`groundtruth/run_pilot_183.sh`](groundtruth/run_pilot_183.sh) (bash; the same commands that stood here, where zsh passed the unquoted options as one word and `$MYC` was not set). It sets the environment of the run block, checks the `myc` binary's hash and the CLI's version (2.1.331) before each run,
and stops at the first failure; a report above the limits stops the main run, not the two pipeline checks.

```bash
bash evals/agent/groundtruth/run_pilot_183.sh --dry-run   # prints every command, starts nothing
bash evals/agent/groundtruth/run_pilot_183.sh             # real sessions
```

##### Pilot result (24 November 2026, 08:25-08:52)

*The numbers of this section are the ones registered first (recall over all dependents); "Scored again" at its end has the redefined outcome.*

Seven sessions, each final at the first attempt, status ok (that hides one API error: in `beans-max-...` the answer stopped arriving for 181 s, `stream_idle_timeout`, and the CLI continued by itself; its 365 s include the stall, so without it the projection below would be 9.2 h instead of 11.0 h), Claude Code 2.1.331, `claude-sonnet-5-5`, normal login (`apiKeySource` `none`). Load of the machine 8-30 during the sessions (`none`: 13-26 on average), so wall times are upper estimates of a quiet machine's.
Artifacts: [`results/pilot-183/`](results/pilot-183/) (`sessions.jsonl` with the five `none` rows, `pilot.json`, `checks.json`, `meta.json`); transcripts are archived outside the repository. **Not pooled, no test.**

| Task (`none`) | Solved | Dependent recall | Tokens | Cost | Session s | Grading s | Mean load |
|---|---|---:|---:|---:|---:|---:|---:|
| `type-mismatch-required-type-arg` | yes | 0.50 | 62,785 | $0.05 | 14 | 123 | 13 |
| `mime-type-parameter-names-case-insensitive` | yes | 0.33 | 116,892 | $0.10 | 112 | 55 | 26 |
| `beans-max-nested-path-depth` | yes | 0.00 | 555,475 | $0.40 | 365 | 46 | 20 |
| `api-version-deprecation-handler-handler-arg` (weak test) | yes | 1.00 | 140,659 | $0.15 | 35 | 124 | 17 |
| `retry-max-attempts-to-max-retries` (weak test) | yes | 0.89 | 419,177 | $0.34 | 81 | 98 | 18 |

* **Main run, estimated again** (mean of the five x 180): **$38** (95 % bootstrap over the five sessions $17-59; 180 times the cheapest or dearest session $9-71) and **11.0 h** (8.0-16.2 h; 7.3-21.1 h). Below the stop limits of $288 and 24 h. It carries over what `none` measured: **C and D are not measured** and may cost more or less.
* **Ceiling:** mean dependent recall 0.54, SD between tasks 0.41, below 0.90, so none by the rule. **The mean hides the split:** remote code 5 of 5, docs 2 of 3, tests 7 of 16 dependents. Remote code has no headroom on these tasks, and the recall as defined mostly measures whether the agent writes new tests nobody asked for: an engineer who does everything else perfectly and writes no new tests would reach about 0.64 over the 20 tasks (the reviewer's reading, not computed here). **Tests pass 5 of 5**, so that secondary outcome has no headroom on these five tasks either.
* **Detectable difference** with that SD as a stand-in for the paired differences': 0.26 for one test, 0.30 with Bonferroni over three pairs, **0.32 for the first Holm step**. It is an SD of five tasks and of single sessions, not of differences; the number replaces the pre-registered 0.3-SD one (0.23) and nothing else.
* **Pipeline checks** (outcomes not reported): C and D each started, reached the myceliums server and were graded. D's agent built in the session; C's did not, so building inside C was only checked with the mock API.
* **Audit:** no disagreement, kernel refusal, protected place or flag in the seven sessions; the hook refused every call that would have left the rules before it ran (`none`: 8 calls: `..` and a path without the working copy's directory, `sed -i`, `$`, a trailing backslash). No remote Java connection in 1,404 socket samples. **Who has read what:** the agent that ran the pilot read the tool calls of all seven sessions, C and D included (arguments shortened, not the models' messages, which is why it missed the API error), and saw the log lines with C's and D's tokens and file scores. It is therefore not blind and does not change tasks, gold or wording.
* **Found, not fixed there** (the audit counts them now, see *Refused reads*): Claude Code itself ("don't ask" permission mode, not the hook) refused three Bash calls that read working-copy files (two of them in `none`), and the audit does not count them; the cause is not known. `export` writes absolute home paths into `meta.json` (shown as `~` here).
* **Nothing was changed** because of the pilot so far. The run did not re-review the gold. The independent review of the pilot's pull request found gold problems: the renamed test class of `retry-...` counts as a deleted plus an added file, so as two dependents (the agent edited the old one in place); `spring-mvc.xsd` in `jsonl-...` is documentation text counted as code; and most missed test dependents are new tests the task does not ask for. The fixes came in a separate pull request, by someone who has not seen C or D (*Classifications corrected by rule*, *Forced tests*).

**Scored again, for information** (arm `none` only, the same five sessions, corrected gold and redefined (a); [`results/pilot-183/rescored_none.json`](results/pilot-183/rescored_none.json); `sessions.jsonl` and `pilot.json` are unchanged):

| Task (`none`) | (a) | Remote | Docs | Forced tests | New tests | Recall with new tests | Wall s | Without stall | Refused by Claude Code (`cd`) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `api-version-deprecation-handler-handler-arg` | 1.00 | 2/2 | – | 2/2 | – | 1.00 | 35 | 35 | 1 (1) |
| `beans-max-nested-path-depth` | 0.00 | – | 0/1 | – | 0/5 | 0.00 | 365 | 185 | 0 (0) |
| `mime-type-parameter-names-case-insensitive` | 1.00 | 1/1 | – | – | 0/2 | 0.33 | 112 | 112 | 0 (0) |
| `retry-max-attempts-to-max-retries` | 1.00 | 2/2 | 1/1 | 5/5 | – | 1.00 | 81 | 81 | 1 (1) |
| `type-mismatch-required-type-arg` | 1.00 | – | 1/1 | – | 0/1 | 0.50 | 14 | 14 | 0 (0) |

Mean (a) 0.80, SD between the five tasks 0.45; recall with new tests 0.57 (0.54 before the corrected classifications). `beans-max-...` has one counted dependent, a docs page, and the session did not touch it. Projection of the main run without the stall 9.2 h. Five tasks say nothing about the decision rule, which needs the 20 sessions.

##### Sessions of arm `none` and the recorded decision (24 November 2026, 11:47-13:18)

20 sessions, one per task, as pre-registered: label `2026-11-24-none20`, not pooled with anything, all 20 final at the first attempt, status ok, no API error or stall, no limit. Claude Code 2.1.331, `claude-sonnet-5-5`, normal login (`apiKeySource` `none`), `myc` not used in this arm, task file sha256 `a8d649a1ef51`. Checked before: the task file (0 problems), the frozen dependencies (6,951 files, `cc17a8254506`), the JDK copy, the login names, the mock probe of `none` with the new prompt sentence. One change to the harness before the first session: a failed login (401/403, "Please run /login", ...) stops a run and is never a final session; until then it would have counted as a failed session.
Artifacts: [`results/none20-183/`](results/none20-183/) (`sessions.jsonl`, `summary.json`, `meta.json`), [`groundtruth/decision_183.json`](groundtruth/decision_183.json); transcripts are archived outside the repository.

**The decision by the registered rule: mean (a) = 0.661** (unrounded 0.66125; 0.792 over the 72 pooled units), below 0.90 − 1e-9. **(a) and (b) stay co-primary.** 10 of the 20 tasks have (a) = 1, 5 have 0; SD between tasks 0.42. Tests pass 20 of 20. Recall with new tests (the outcome registered first): 0.45.

| By type | Units | Touched | Tasks with any | Mean over those tasks |
|---|---:|---:|---:|---:|
| remote code | 45 | 44 | 14 | 0.99 |
| docs | 13 | 2 | 9 | 0.22 |
| forced tests | 14 | 11 | 7 | 0.57 |

So (a) has almost no room in remote code (44 of 45). Of the 0.339 that the mean lacks to 1, docs are 0.254 (75 %), forced tests 0.080 and remote code 0.005; nine tasks have docs, four of them nothing else. The main run's (a) would mostly measure whether an arm follows a change into `framework-docs`.
**What the docs figure says and does not.** By task 2 of the 9 with docs were touched (exact 95 % interval 3-60 %): the direction is robust, the rate is not. Of the 13 docs units nine would be wrong or clearly incomplete after the change, two incomplete and two borderline (the `UserController` samples) (the reviewer's reading). The prompt's "change every file the change needs **and nothing else**" may discourage docs; that was not tested and is the same in every arm.

| Task | (a) | Remote | Docs | Forced tests | New tests | Tokens | Cost | Session s | Grading s | Mean load | Hook refusals | Claude Code refusals |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `api-version-deprecation-handler-handler-arg` | 1.00 | 2/2 | – | 2/2 | – | 188,991 | $0.19 | 168 | 45 | 43.6 | 3 | 0 |
| `bean-provider-parameterized-type-reference` | 1.00 | 5/5 | – | – | 0/1 | 157,647 | $0.16 | 339 | 98 | 69.9 | 1 | 0 |
| `beans-max-nested-path-depth` | 0.00 | – | 0/1 | – | 0/5 | 400,085 | $0.29 | 214 | 81 | 102.3 | 0 | 1 |
| `client-builders-deep-copy-default-headers-cookies` | 1.00 | 2/2 | – | – | 0/3 | 278,983 | $0.22 | 248 | 77 | 21.0 | 2 | 0 |
| `content-negotiation-no-fallback-mapping` | 1.00 | 1/1 | – | – | 1/2 | 211,427 | $0.15 | 166 | 45 | 24.4 | 0 | 0 |
| `deferred-bean-registrar-contains-bean` | 1.00 | 3/3 | – | – | 0/7 | 335,208 | $0.33 | 220 | 88 | 48.3 | 2 | 0 |
| `error-response-interceptor-with-request` | 1.00 | 6/6 | – | – | 0/4 | 271,660 | $0.22 | 167 | 101 | 36.2 | 1 | 0 |
| `form-converter-map-payloads` | 0.80 | 3/3 | 0/1 | 1/1 | – | 397,621 | $0.27 | 389 | 75 | 103.9 | 2 | 0 |
| `http-range-honor-if-range` | 1.00 | 4/4 | – | – | 1/6 | 254,098 | $0.22 | 203 | 65 | 68.8 | 1 | 0 |
| `jdbcclient-batch-entry-dsl` | 1.00 | – | 1/1 | 3/3 | – | 1,177,556 | $0.62 | 250 | 40 | 53.8 | 5 | 1 |
| `jsonl-media-type-support` | 0.80 | 8/9 | – | 0/1 | 0/1 | 195,616 | $0.18 | 50 | 167 | 73.5 | 4 | 0 |
| `message-source-default-charset` | 0.00 | – | – | 0/1 | – | 137,074 | $0.12 | 146 | 60 | 73.0 | 1 | 0 |
| `mime-type-parameter-names-case-insensitive` | 1.00 | 1/1 | – | – | 0/2 | 170,231 | $0.10 | 29 | 129 | 32.2 | 1 | 1 |
| `observation-record-handled-exception` | 0.62 | 5/5 | 0/3 | – | 0/9 | 222,257 | $0.22 | 169 | 79 | 55.7 | 1 | 0 |
| `retry-max-attempts-to-max-retries` | 1.00 | 2/2 | 1/1 | 5/5 | – | 581,447 | $0.41 | 204 | 45 | 36.1 | 6 | 0 |
| `retryable-method-retry-event` | 0.50 | 1/1 | – | 0/1 | 0/3 | 165,054 | $0.21 | 131 | 60 | 40.8 | 1 | 0 |
| `servlet-response-ignore-body-flush` | 0.50 | 1/1 | 0/1 | – | 0/1 | 133,798 | $0.14 | 162 | 71 | 72.0 | 1 | 0 |
| `spel-simple-context-restricted-conversion` | 0.00 | – | 0/2 | – | 1/1 | 357,077 | $0.29 | 159 | 54 | 37.3 | 4 | 0 |
| `test-context-cache-pause-mode` | 0.00 | – | 0/2 | – | 0/3 | 70,401 | $0.11 | 23 | 327 | 106.9 | 0 | 1 |
| `type-mismatch-required-type-arg` | 0.00 | – | 0/1 | – | 0/1 | 37,012 | $0.04 | 8 | 140 | 25 (grading) | 0 | 0 |

Mean: 287,162 tokens (SD between tasks 245,152), $0.224, 172 s per session, 4.47 USD in all (about 4 were approved). No stall, so wall times without stalls are the same. Machine load 13-134 (median 56), so wall times are upper estimates of a quiet machine's; `type-mismatch-...` was too short for a load sample.

**Every missed unit** (15 of 72; from the sessions' tool calls, diffs and last messages):

| Task | File | Kind | Why |
|---|---|---|---|
| `beans-max-nested-path-depth` | `data-binding.adoc` | docs | searched `src/main` of two modules only; `framework-docs` in no call or result |
| `form-converter-map-payloads` | `message-converters.adoc` | docs | its searches were limited to `*.java`; no docs searched |
| `jsonl-media-type-support` | `ProtobufJsonEncoder.java` | remote | its grep listed the file; left alone on purpose ("isn't one of the JSON codecs you listed") |
| `jsonl-media-type-support` | `HttpMessageWriterViewTests.java` | forced tests | compiled and ran nothing; no test searched |
| `message-source-default-charset` | `ResourceBundleMessageSourceTests.java` | forced tests | compiled main and tests (both pass, the test fails only when run) and ran no test |
| `observation-record-handled-exception` | `observability.adoc` | docs | searched and edited main code only; no docs searched |
| `observation-record-handled-exception` | `reactive/UserController.java` | docs | its search for `setError` covered `src/main` of three modules, not `framework-docs` |
| `observation-record-handled-exception` | `servlet/UserController.java` | docs | as above |
| `retryable-method-retry-event` | `RetryTemplateTests.java` | forced tests | ran no test; its summary says that tests asserting the old message "will need updating" |
| `servlet-response-ignore-body-flush` | `appendix.adoc` | docs | added the property's constant; searched no docs (the appendix lists such properties) |
| `spel-simple-context-restricted-conversion` | `appendix.adoc` | docs | searched no docs |
| `spel-simple-context-restricted-conversion` | `evaluation.adoc` | docs | searched no docs; added tests instead |
| `test-context-cache-pause-mode` | `appendix.adoc` | docs | its summary says the docs were not updated; read three main files, searched nothing |
| `test-context-cache-pause-mode` | `caching.adoc` | docs | as above |
| `type-mismatch-required-type-arg` | `mvc-ann-rest-exceptions.adoc` | docs | three calls in all; no docs searched |

One pattern: the eleven missed docs units were not searched for: `framework-docs` occurs in no call, result or message of those seven sessions. It was not for want of a hint in two of them: the AGENTS.md of `spel-...` and `observation-...`, which reaches the agent (mock probe), says that the reference docs are in `framework-docs/modules/ROOT` and the samples in `framework-docs/src/main`. Nor did they leave documentation alone: six of the seven (all but `type-mismatch-...`) wrote or changed Javadoc.
Another session, `api-version-...` (no docs unit in the gold), searched `*.adoc`, found pages and left them on purpose ("They only mention the handler concept, not its signature"). The two found (`core.adoc` in `jdbcclient-...`, `resilience.adoc` in `retry-...`) came from a search over the whole checkout without a file-type filter. The sessions of the three missed forced tests ran no test. `observation-...` set the context error at once and reports it at completion, exactly the reading the review gave for leaving its six tests out of (a).

**Audit.** Every refused call and every flag of the 20 sessions was read, and all shell and search calls of the ten with missed units (not the models' other messages): 36 hook refusals (expansions, `sed -i`, `python3` heredocs, braces, two reads of wrong paths, two Gradle `check` tasks, one write outside the working copy and one command naming that file), no disagreement, no kernel refusal, no edit naming a protected place. Two flags, both benign: the CLI's own "output too large, saved to ..." line names this session's result file under `~/.claude/projects/` (`error-response-...`, `jdbcclient-...`); neither session opened it.
**Refused by Claude Code itself:** 4 calls in 4 sessions (`mime-...`, `beans-...`, `test-context-...`, `jdbcclient-...`), all with a relative `cd` and relative reads, the plainest a `cd DIR; grep -n ... MediaType.java`. None of the 80 Bash calls without `cd` that reached that layer was refused; of the 7 calls with `cd` 4 were refused here and 3 by the hook, so none ran. A refusal has a price: after its one refused call `test-context-...` concluded that "the Bash tool was denied in this session", used no Bash again and never built (it was solved all the same, with (a) = 0). The sentence did not stop the agents from using `cd` (7 calls in 20 sessions, 8 in the five of the pilot) but made it rarer, and `cd` is now a stronger suspect; one `cd` call ran in the pilot, so it is still not proven.
Sockets: no remote connection in 4,610 samples. One TCP listener on all interfaces (first seen at 12:02:37, in the last seconds of the grading of `client-builders-...`; the sampler logs a kind of socket once, so later ones are not excluded; its three test classes name no server, cause not found, process and port not logged). The static reading above (*Tests that listen on all interfaces*) is corrected accordingly; the grading sandbox does not stop inbound connections. The next run uses `groundtruth/watch_run.py`, which logs process, port and every new listener.

**Main run, projected from these** (mean × 180): **$40** (95 % bootstrap over the tasks $31-51), **13.7 h** (12.1-15.6 h), below the limits of $288 and 24 h; the same without stalls. It carries over what `none` measured: C and D are not measured and may cost more or less.

**What the main run could detect in (a) is not known.** There is no paired data; the between-task SD of these sessions (0.42) is **not** the SD of a paired difference, because a task's own level cancels in the pair, while an arm that helps in some tasks and not in others adds to it. Two scenarios, neither a bound nor a power statement for the real effect (two-sided, 20 tasks, normal approximation):
*Noise only*: the whole between-task SD is session noise and no task favours an arm. Then the difference of two arms' 3-repetition means has SD √(2/3) of it, 0.35, and 0.22 would be detectable with 80 % for one test, 0.27 at the first Holm step over six. With any task × arm effect the SD can be larger, and with one session per task the noise cannot be told from the task.
*Closing every docs miss*, without noise: an arm that touches all docs units missed here gains **0.254** and nothing more. Its gains differ between tasks (SD 0.41 of the 20, zero in 13), which alone leaves a power of 80 % for one test and **56 % at the first Holm step over six**; noise would lower it. So even that arm would often not be told from `none`. For scale, paired SDs of 0.1, 0.2 and 0.3 give 0.08, 0.16 and 0.23 for the first Holm step over six.
**(b):** paired differences of a finished run exist only for #182's question tasks (same repository, model and CLI generation, 3 repetitions): their SD is 0.53 of the baseline's mean for C vs `none`, 2.69 for D vs `none` and 0.98 for C vs D, so differences of 41%, 210% and 76% of the mean for the first Holm step over six; edit sessions may differ.

#### Warm-up

`groundtruth/warm_gradle.py` (run on 24 November, 06:30-06:51, for the 12 tasks that lacked artifacts: 12 of 12 builds ok by its log; `--dry-run` prints every command and host, `--go` starts). With the network it only **resolves and compiles**: an init script takes the actions of every test task away, in `buildSrc` too, so its class path is resolved and no test runs; a test report found afterwards stops it. It stops at the first error; a connection or read without an answer fails after 45 s, one attempt;
a build over 20 minutes is killed with everything it started; no `--continue`, no JDK download. **The commit's build logic (buildSrc, plugins, code generators, annotation processors) does run, unsandboxed and with the network**, as in any online build. Java connects itself, not through another program, so that the machine's firewall sees it as what it is; the list of hosts is informative and **the firewall is the enforcement**. The init script was tested offline in the grading profile on two tasks (Gradle 9.2.1 and 9.9.0; record: [`groundtruth/warm_init_check.json`](groundtruth/warm_init_check.json)): compiled, class paths resolved for the module's and buildSrc's test tasks, no test report. The timeouts were not exercised. Afterwards `gradle-cache` (it refuses Spring Framework artifacts other than
BOM metadata), then the prechecks, offline and sandboxed.

**Index plan: derive, do not rebuild.** `myc` has no command that updates an
index from a git diff: file-level re-indexing (`analyze_files`) is reachable
only through `analyze --watch` (and email sync), and the post-commit hook
runs `analyze --skip-embeddings`. What it does have is a full graph rebuild
that takes over the vector of every symbol whose embedding text is unchanged
(`analyze --force`, #151), so only changed symbols are embedded. That reads the
previous index by repository id, which hashes the path: at another path
nothing is reused, silently (measured below). Hence one shared work path, and `index --base HOME`:

* it refuses a base of another path, and an index that reused no vector;
* newest parent first, each index is derived from its neighbour's, so each
  step embeds one interval of history only;
* each must equal a fresh graph-only build of the same working copy in symbols, files, relationships and mentions
  (`meta.json` `derived`); a difference stops the run.

The base is the #182 run's embedded index of v7.1.0 (same models; its path is
`/private/tmp/aeval182/spring-framework`, so `AEVAL_WORK_ROOT=/private/tmp/aeval182`),
used only after that run has finished with the path. v7.1.0 is the better
base than the oldest parent: 8 of the 20 parents differ from it by at most
606 files (7 to 606), against 3,039 to 3,296 from the oldest.

**Measured** (24 November; [`groundtruth/index_updates.json`](groundtruth/index_updates.json); `myc` `a42bffce`, the binary that built the base; graph and
embedding code unchanged on `main` since; M1 Max, `nice 15`, beside other work: load 16-43 during the chain, 49-61 during the no-op, 77-102 during the path test, so
the path test's seconds are slow; working on a copy of the #182 index. A reviewer's read-only search of the finished indexes ran about two minutes at `nice 19`; by its time it can only have touched the `deferred-bean-registrar-...` step):

* **Base:** 17,699 s (4.9 h) for 145,670 vectors, under load. My earlier "about 3 hours" was an extrapolation and too low.
* **Update on every merge** (the real case, forward in history): the 21 consecutive first-parent commits of Spring's `main` after v7.1.0 (13-23 November; 4 are merge
  commits, 43, 42, 4 and 2 files): median 42 s, maximum 88.5 s; about 30 s of it is the full graph rebuild, the rest vectors. Re-embedded symbols:
  0 to 210 of about 145,500 (median 3); all others reused, proved by the counts in the log. The commit that replaced every file header (9,053 files)
  re-embedded 38. Peak memory 4.5-7.7 GB. Graph counts equal a fresh graph-only build in the 3 steps compared (first, last, header change).
* **No change at all** (same commit again): 36 s, 145,670 of 145,670 vectors reused.
* **Another path** (503 symbols): updating at the same path reused 486 vectors (9.7 s against 55 s); an index moved to another path reused **0**, also after its repository entry and files were put
  under the new path's id, and so did a copied data directory. So a worktree at another path cannot use a base index today (#198/#184).
* **Disk:** 0.86-0.90 GB per embedded index. `myc diff` keeps ten graph snapshots of about 190 MB beside it (3.1 GB in all after ten updates). They hold **earlier analyses' graphs**: an
  index derived newest-first would show a session symbols of later commits. The harness removes them from derived indexes and from every session's copy, and `myc diff` is off the Bash allowlist.
* The task parents lie before v7.1.0, so their derivation runs **backwards** in history; the figures above are the forward case. All 31 parents (20 kept, 9 reserves, 2 dropped later) were derived from the base: the kept 20 in 4942 s in all (median 169 s, at most 869 s, of which a fresh graph-only comparison build ~30 s), re-embedding a median 472 (at most 4773) of 139k-146k symbols. Each equals a fresh graph-only build in symbols, files, relationships and mentions, has exactly the parent's commit, one generation and, after stripping, no snapshots; 0.86-0.90 GB each without snapshots (27 GB for 31). For 8 of 9 tasks with a checkable name the parent's index has no symbol the commit introduces (the positive control finds it in the v7.1.0 index; the ninth, the deferred registrar's interface, was removed again before v7.1.0). `groundtruth/derived_indexes.json` has the numbers per parent.

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

Plus, once: the derived indexes and the prechecks. The call counts and context sizes above are assumptions; a pilot of one
repetition would replace them. Disk: 20 parents 2.0 GB, 0.86-0.90 GB per embedded index without snapshots, `~/.gradle` 9.9 GB, frozen dependencies 1.8 GB.

**Prechecks: 20 of 20 valid, in the grading sandbox** (24 November, 06:52-07:56, after the warm-up, on the new frozen dependencies; JDK copy, a Gradle home of its own per task, below; no task swapped for a reserve). `precheck-edit --gold-runs 2 --sandbox` runs the tests as a session's grading does:
on the parent with the commit's tests (must fail, and for a compile or test failure, not a resolution failure), then twice on the whole commit with Gradle's `--rerun`. The eight tasks that were valid before were run again, because the warm-up changed the frozen dependencies; the times are from a machine with load 18-66 whenever it was read.
In all 20 parent logs every compile error and every failing test class lies in the commit's own test files (checked by script); `retryable-method-retry-event` fails on the new message of `RetryException` that its description demands. Why each test fails was read from those lines, not from whole logs.
Loopback TCP and UDP sockets in 4,055 socket samples, no remote connection and no TCP listener on all interfaces. Before the warm-up 8 of 20 were valid and 12 lacked artifacts: [`groundtruth/commit_replay_pending.json`](groundtruth/commit_replay_pending.json), kept as a record.
Earlier records used other commands: the online ones are in [`groundtruth/`](groundtruth/) (`commit_replay_prechecks_online.json`), the offline ones without a sandbox only in this branch's history.

| Task | Parent + commit's tests | Commit | Time on the commit |
|---|---|---:|---:|
| `spel-simple-context-restricted-conversion` | compile, 78 s | 2 of 2 | 22, 10 s |
| `observation-record-handled-exception` | compile, 133 s | 2 of 2 | 42, 12 s |
| `beans-max-nested-path-depth` | compile, 78 s | 2 of 2 | 31, 11 s |
| `form-converter-map-payloads` | compile, 127 s | 2 of 2 | 35, 12 s |
| `jsonl-media-type-support` | compile, 119 s | 2 of 2 | 37, 13 s |
| `test-context-cache-pause-mode` | compile, 425 s | 2 of 2 | 89, 34 s |
| `retry-max-attempts-to-max-retries` | compile, 85 s | 2 of 2 | 43, 12 s |
| `jdbcclient-batch-entry-dsl` | compile, 89 s | 2 of 2 | 17, 10 s |
| `type-mismatch-required-type-arg` | tests, 132 s | 2 of 2 | 19, 11 s |
| `servlet-response-ignore-body-flush` | compile, 314 s | 2 of 2 | 43, 10 s |
| `content-negotiation-no-fallback-mapping` | tests, 123 s | 2 of 2 | 21, 13 s |
| `client-builders-deep-copy-default-headers-cookies` | tests, 122 s | 2 of 2 | 29, 15 s |
| `http-range-honor-if-range` | compile, 122 s | 2 of 2 | 36, 11 s |
| `error-response-interceptor-with-request` | compile, 113 s | 2 of 2 | 33, 12 s |
| `mime-type-parameter-names-case-insensitive` | tests, 112 s | 2 of 2 | 50, 13 s |
| `deferred-bean-registrar-contains-bean` | compile, 88 s | 2 of 2 | 34, 11 s |
| `api-version-deprecation-handler-handler-arg` | compile, 129 s | 2 of 2 | 54, 12 s |
| `bean-provider-parameterized-type-reference` | compile, 102 s | 2 of 2 | 104, 10 s |
| `message-source-default-charset` | compile, 116 s | 2 of 2 | 40, 11 s |
| `retryable-method-retry-event` | tests, 104 s | 2 of 2 | 40, 13 s |

**Connections, sampled with `lsof` every 0.7 s or so** over the 24 offline runs and the probe's three sessions (2,616 samples in all, all
processes of the JDK): no remote connection; loopback TCP, a UDP socket per Gradle process (file-lock messages), and wildcard TCP listeners
only while `fn-request-bind-pathvars-headers` ran its HTTP server tests. A sample can miss a short connection. The wrapper unpacked
9.8.2 from its zip without a connection.

* **Reproducibility risk:** six of the 20 parents depend on unpinned `-SNAPSHOT` BOMs
  (micrometer 1.18.0, reactor 2026.0.0, not counting `spel-big-power-result-limit`; `beans-max-nested-path-depth` reactor
  only). Offline, Gradle takes the cached snapshot; a lost cache may not be recoverable.
* `~/.gradle` is 9.9 GB after the warm-up, with all eleven distributions unpacked. The frozen copy from before the warm-up (4,909 files, 1.3 GB) is kept beside the new one.

**The wrapper must never connect.** `gradlew --offline` does not cover the wrapper's
own download of its distribution, and the firewall on this machine attributes
that connection to `gradle-wrapper.jar` at the path of the working copy. Each
precheck used a new path (`.../precheck-<task>/`), deleted afterwards, so a firewall
rule never matched twice and asked again every time; that explains the
timeouts of 23 November while nobody answered. Since then every precheck uses one path, the
sessions one path (`work_name`), and `wrapper_problem` refuses to start Gradle when the
distribution is not complete on disk (unpacked with its `.ok` marker, or a whole
zip): before a run's first session, before each session, before each precheck and before the
grading test (an agent may have edited the wrapper properties).

**Network-free builds.** Test commands start with `./gradlew --offline --no-scan
--no-build-cache`, and the hook (below) holds every arm to the same rule; every arm's prompt says so. No build
cache means no task output of another arm's session or of a precheck of the gold commit, and no remote cache. The
sessions' `GRADLE_OPTS` send HTTP(S) to a closed loopback port, with the Spring build's remote cache
(`DEVELOCITY_CACHE_SERVER`) there too.

**Isolation** (repositories marked `isolated`, the same in every arm; [`aeval/isolation.py`](aeval/isolation.py)). Several independent
reviews found holes in the earlier versions (gold files, history and the network reachable; then a quoted `#` that ended the parser, brace
expansion, an incomplete list of closed places, and a keychain opened to every child process). The threat model below states what remains.

* **What runs where.** `claude` itself runs as in daily use and in #182 (login, token refresh, API), not in a sandbox of ours: an earlier version wrapped it and had to open the login
  keychain to every child process. Confined is what the agent executes, through settings passed with `--settings`, which a repository's files cannot loosen:
* **Bash:** Claude Code's own sandbox (Seatbelt; `failIfUnavailable`, `allowUnsandboxedCommands: false`, no excluded commands). Under the home directory, `/private/tmp`, the temporary directories,
  `/Volumes` and `/Users/Shared` commands and their children (a test the agent wrote, run by Gradle) read only the working copy, the session's own temporary directory and Gradle home, the JDK, the installed Gradle
  distributions and the frozen dependencies, and in C and D the `myc` binary, models and this session's index copy; they write only the working copy, that directory and that Gradle home, not `.claude`, `.mcp.json`,
  the wrapper, git hooks and configuration. No network but loopback. The model is shown these lists, so they name whole regions, and session directories are neutral (`/private/tmp/ag/...`).
* **Read, Grep, Glob, Edit, Write** run inside `claude`: `blockReads/WritesOutsideWorkingDirectories`, deny rules, and the hook, on canonical paths.
* **Hook** (PreToolUse on every tool, one script): tool calls stay in the working copy; no settings, `.git` or wrapper files; Bash is an allowlist without interpreters, wrappers or network
  programs, and a command the parser cannot fully account for (any expansion, an open quote, a continuation line) is refused, as is `..`;
  WebFetch, WebSearch and unknown tools are refused; a check that fails refuses.
* **Grading:** the test command runs under our own `sandbox-exec` profile: the same files, no network but loopback, no keychain files, no security services.
* **Same conditions.** All arms share D's allowlist, the Gradle rule and `| tail`, `| head`. `none` and C have only search in addition: `Grep`, `Glob` and the
  shell's read-only search and text programs (`grep`, `rg`, `find`, `ls`, `cat`, `awk` without `system`/`getline`/pipes/redirections, `sed -n` printing, `sort`, ...).
  I allowed `awk` and `sed -n` there because the sandbox covers reads and they are how an agent slices grep output; interpreters stay out
  to keep the arms' commands comparable and the audit readable.
* **Direction of the bias.** Without scripting, bulk edits (a rename across files) cost one `Edit` per place in every arm, so absolute tokens and wall time are
  above what a user would see. It should hurt `none` most: its usual `grep | xargs sed` and `python` scripts are gone, while D never had them; a result in favour of
  myceliums is therefore partly a result against a restricted baseline, above all for `retry-max-attempts-to-max-retries`.
* **Audit** per session, with raw-text tests that share no code with the hook (program names, addresses, absolute and parent paths, Gradle flags): what was refused, what
  ran although the scan suspects it (*disagreements*, kept in `to_review` and to be read, because the scan also meets search patterns), kernel refusals, protected places in
  results or edits; also calls Claude Code itself refused and API stalls (*Refused reads and API stalls*). `TOKENS.md` counts them per arm, with the hook's refusals.
* **Threat model.** A cooperative agent told to work in its checkout, audited per session. Not covered: **macOS's security services**: inside Claude Code's sandbox, lookups of `com.apple.SecurityServer` and
  `com.apple.securityd.xpc` succeed (2.1.331; `secd`, `securityd`, `trustd.agent` and the token service do not), and its settings can only add services, not remove them. The keychain *files* are closed; whether these services would hand anything
  to a process that cannot read them was not tried. `claude`, the myceliums server and the hooks run with the user's access. A listener a test opens on all interfaces could be reached from the local network while it runs (seen once, during a grading; the agent may also run other tests). Names of files outside may stay
  visible; Spring's commits may be in the model's training data.
* Checked with the mock API, hook on and, to see the layer below it, hook off (`probe --no-hook`): 31 places by name from a Python program inside the sandbox (keychains, Claude's history, other projects and credentials file, `.ssh`, checkouts, gold, parents, the other cache, history clone,
  build cache, stored scans, `/private/tmp/claude-501`, `/Volumes`, `/Users/Shared`, the `/System/Volumes/Data` alias of the home directory, ...): all refused; loopback works; settings and wrapper files cannot be written. No connection to another machine was attempted.
  Gradle in the sandbox: see below.
* **Same instructions, one CLI.** Claude Code reads AGENTS.md only where there is no CLAUDE.md. Until #200 `setup-claude`
  wrote one, so C alone missed Spring's AGENTS.md (found in the #182 review); now its block is in `.claude/rules/myceliums.md`. The harness adds nothing and refuses a `myc` whose setup hides AGENTS.md. Six of
  the 20 parents have an AGENTS.md (`spel-simple-...`, `observation-...`, `content-negotiation-...`, `client-builders-...`,
  `http-range-...`, `error-response-...`). A run uses one Claude Code binary (the file behind the link), refuses a resume or a
  session with another version, and keeps auto-update off. Rows carry `api_calls` (model calls; the CLI's `num_turns` is tool calls + 1). The hook
  answers with JSON, so a refusal shows its reason without the hook's command line.

`io.spring.develocity.conventions` (0.0.22 and 0.0.25, read from its source; the scan and
cache conventions are identical in both, the plugin class differs, and both check `--no-scan`):

* **Build scans** are published to `https://ge.spring.io` only if authenticated
  (`publishIfAuthenticated`); with `--no-scan` the scan conventions are not applied. No access
  key is set on this machine (no `DEVELOCITY_*`/`GRADLE_ENTERPRISE_*` variable, no key file
  in `~/.gradle`), and no log mentions a published scan. `~/.gradle/build-scan-data`
  holds two scan payloads (1.6 MB), both written locally by the online warm-up; such payloads contain the user name and local
  paths. The evidence says they were not sent; it is absence of evidence of an
  upload, not a packet capture.
* **Remote build cache:** on by default whenever Gradle's build cache is
  (`org.gradle.caching=true` in Spring), read-only, at `https://ge.spring.io`
  (pushing needs a key and a CI). The first build into an empty `~/.gradle` took 13
  tasks from a cache, which points to it having been read. Gradle turns the
  remote cache off with `--offline` (its documentation; not yet observed here).

**Mock validation** (no model call, Claude Code 2.1.331, graph-only index, shared path), on `spel-simple-context-restricted-conversion`; the scripted session
searches, runs `myc callers`, reads and edits the primary file and runs one command.

| Arm | Isolation | Tools offered | Search | `myc callers` | Edit graded |
|---|---|---|---|---|---|
| none | no problem; AGENTS.md in the first request | Bash, Edit, Glob, Grep, Read, Write | Grep ran | `command not found`, as it must | yes |
| C | no problem; AGENTS.md and the routing block | the same + 9 myceliums tools | Grep ran | answered from the index | yes; setup-claude's files not counted |
| D | no problem; AGENTS.md | without Grep and Glob | refused; the refusal names the working copy, not this repository's checkout or the hook | answered from the index | yes |

The table is the probe of 24 November (arms `none`, C, D, Claude Code's own sandbox, the derived index, shared path, the `myc` below): `myc` answers from the index in C and D and is not
found in `none`, AGENTS.md reaches every arm, C also gets the routing block (`.claude/rules/myceliums.md`; setup writes no CLAUDE.md), no isolation problem. Gradle in the session and the grading: below. The probe was repeated in the three arms after the warm-up, on the new frozen dependencies (same binary, CLI and derived index): the same findings, `BUILD SUCCESSFUL` in 76-91 s in the sessions, no remote connection in 455 socket samples. The mock gives every answer one message id, so `api_calls` is 1 there. (Real sessions so far: the pilot, above.)

**Binary.** C and D use `myc` built from main at `787920c` (sha256 `bf84b0e854f6`), so arm C measures the setup as it is now (#200), without a workaround. The indexes stay valid: between `388c6e1`, which built the base index and the derived ones, and `787920c` nothing under `crates/` changed
but `crates/myceliums-mcp/src/embedding_jobs.rs` (#201: the job releases the repository's lock before it reports its end; no effect on the graph or the vectors of a complete index). Outside it: `myc/src/commands/{setup_claude,setup_editors,instructions,routing,doctor,mod}.rs`, `cli.rs`, `main.rs` and `setup/editors/claude.rs`.
`Cargo.lock` is the same.

**Gradle in the sandbox.** The sandboxes' loopback rule does not cover IPv4-mapped IPv6 addresses, which a JVM uses by default: a 10-line Java program reaches `127.0.0.1` with `-Djava.net.preferIPv4Stack=true` and is refused without it. Environment variables and system properties reach the
daemon and the test executors, but not Gradle's process-isolated workers (checkstyle, validatePlugins, `:spring-oxm:xjc`); that they start with an empty environment is inferred from this, not read in Gradle's source. So builds in prechecks, sessions and grading, in every arm, run on a **copy of the JDK** made by
[`groundtruth/make_jdk_ipv4.py`](groundtruth/make_jdk_ipv4.py): its `bin/java` is a two-line script that runs the renamed launcher with the flag. Nothing else differs; the original is untouched ([`groundtruth/jdk_ipv4.json`](groundtruth/jdk_ipv4.json): hashes of the two changed files and of the original tree). A session or a sandboxed
precheck with another JDK is refused. **The copy is for offline builds only**; the warm-up uses the original JDK (it refuses the copy) and `~/.gradle`. Checked offline by command line only: clients, daemons, workers and test executors all ran from the copy with the flag (none from the original), and `javaToolchains` lists the copy ("Current JVM")
and no other JDK. With it:

| | Task | Result |
|---|---|---|
| Grading profile | the 20 tasks above | valid; buildSrc's checkstyle and validatePlugins run, the former `-x` excludes are gone from the commands, the hook's rule and the build note |
| Claude Code's Bash sandbox (2.1.331, mock API), arms `none`, C, D | `spel-simple-...` (Gradle 9.9.0) | `BUILD SUCCESSFUL` in 67-68 s, tests run (39-45 s with a warm shared home), 76-91 s in the repeat after the warm-up; grading 28 s (36-55 s in the repeat) |
| Claude Code's Bash sandbox, arm `none` | `type-mismatch-...` (Gradle 9.2.1) | `BUILD SUCCESSFUL` in 112 s with `:spring-oxm:xjc` |

The scripted edit is a comment, so the repository's own tests pass in the session and the commit's tests fail in the grading, as they must. Harmless noise: "Could not start the FSEvents stream".

**A Gradle home per session and per grading.** Sessions and gradings each start with an empty Gradle home in the session's directory (`GRADLE_USER_HOME`): links to the installed distributions and nothing else, so no other build's logs, scans, compiled classes or caches are there, and what an agent's builds leave does not reach the grading or a
later session. Dependencies come read-only from a frozen copy of `modules-2` (`agent_eval.py gradle-cache`, `GRADLE_RO_DEP_CACHE`, default `~/.cache/gradle-ro`; 6,951 files, 1.8 GB, listing sha256 `cc17a8254506`, downloaded artifacts and metadata only); `~/.gradle` itself is closed but for the distributions. Price: Gradle's own caches (about 560 MB, deleted with the session) and their time
are made anew, in the session and in the grading, the same in every arm. An earlier version closed old logs and scans one by one (450 bytes of the sandbox's 1 MB command line each). Those of 23-24 November (340 files) were moved to the evaluation's cache, none deleted.

**Login.** The CLI itself runs unsandboxed with the caller's normal login (macOS keychain); only what the agent executes is sandboxed. The harness passes a few login variables on, so a real run over isolated repositories **refuses to start** when the shell has `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, a base URL, or a Bedrock, Vertex or Foundry variable (names only, also when empty; no value is read), and stops at the first session whose init event reports another `apiKeySource` than `none`, the value of the #182 sessions' normal login. `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` choose among normal logins: their names, when present, are recorded per start (`login.jsonl`, `meta.json`). Claude Code's sandbox cannot close the keychain *services* (above), so a dedicated login remains Marc's decision. An alternative not built: a fixed helper outside the sandbox that runs one allowed Gradle command for the agent;
it would give the agent's code (tests, build scripts) the caller's files and network, so it would need a sandbox of its own, and is not needed now.

```bash
# The probe uses the derived index as it is (a copy per session). On a machine without one, a graph-only index in a cache of its own:
# export AEVAL_CACHE=$HOME/.cache/myceliums-agent-eval-probe   # for these two and the probe; `fetch` needs the network
# python3 evals/agent/agent_eval.py --tasks $T fetch --repo spring-spel-simple-context-restricted-conversion
# python3 evals/agent/agent_eval.py --tasks $T index --repo spring-spel-simple-context-restricted-conversion --skip-embeddings --myc $MYC
# (Without AEVAL_CACHE this `index` would replace the derived index; it now refuses.)
python3 evals/agent/agent_eval.py --tasks $T probe --label v183 --task spel-simple-context-restricted-conversion \
    --conditions none,C,D --bash --profile realistic --timeout 400 --myc $MYC --grep SimpleEvaluationContext --callers withMethodResolvers \
    --edit-file spring-expression/src/main/java/org/springframework/expression/spel/support/SimpleEvaluationContext.java \
    --edit-old "public final class SimpleEvaluationContext implements EvaluationContext {" \
    --edit-new "public final class SimpleEvaluationContext implements EvaluationContext { // probe" \
    --edit-command "./gradlew --offline --no-scan --no-build-cache --no-daemon --console=plain :spring-expression:test --tests org.springframework.expression.spel.support.SimpleEvaluationContextTests 2>&1 | tail -15"
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
