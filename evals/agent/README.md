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

python3 evals/agent/agent_eval.py toolsel-run --label my-toolsel   # 54 intents x 2 passes, haiku
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

## Tool-selection eval

[`toolsel/intents.json`](toolsel/intents.json) holds 54 natural-language
requests, each with the myceliums tools a maintainer would accept as the first
call (primary first). `toolsel-run` starts `myc mcp` (empty data directory),
reads the live `tools/list` and fails if any expected tool is missing, then
asks the model once per intent and pass, in a single turn, with only the
myceliums tools attached as native tool definitions (no built-in tools, tool
search off). The first tool call is the pick; it is never executed: the
session runs in `dontAsk` permission mode without any allow rule, so the call
is denied, and `--max-turns 1` ends it. Accuracy per pass, agreement between
passes and the most frequent confusions go to `TOOLSEL.md`.

## Limits

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
