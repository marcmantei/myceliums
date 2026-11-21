# Integrating myceliums with coding agents

A code knowledge graph only helps an agent that uses it. The agent pilot
([#99](../../evals/agent/README.md), 12 November 2026) measured the weakest
integration, the MCP server attached and nothing else: Claude Code called a
myceliums tool in **0 of 90** sessions. Claude Code deferred the server's 54
tool schemas behind tool search, so the agent saw tool names and never loaded
one, and nothing in its context sent it from grep to the graph.

The variant matrix ([#125](../../evals/agent/README.md#variant-matrix-125),
20 November 2026, [report](../../evals/agent/results/2026-11-20-variants/REPORT.md))
measured three integrations on 20 harder multi-hop tasks (transitive callers,
blast radius, same-name symbols, tests that reach a function, call chains)
with `claude-sonnet-5-5` in Claude Code 2.1.329, 116 graded sessions:

| Variant | What the session has | Sessions with ≥ 1 myceliums call | First tool (most common) | Success / mean F1 | Input tokens, cost vs A |
|---|---|---:|---|---:|---:|
| A | MCP server only, 9 core tools loaded up front (layer 2) | 80 % (32 of 40) | `Grep` | 100 % / 0.984 | – |
| C | A + what `myc setup-claude --no-agent` installs: routing block, grep hook, SessionStart and result-format hooks (layers 1-3) | 100 % (38 of 38) | `find_callers` | 98 % / 0.983 | 0.96×, 0.98× |
| E | C + the `code-explorer` subagent (layer 4) | 97 % (37 of 38) | `find_callers` | 100 % / 0.989 | 0.96×, 0.93× |

> **What the numbers say, and what they do not.** With the core tools loaded
> up front the agent uses myceliums; the routing block and hooks push the rate
> from 80 % to 100 % (+20 pp [+5, +38], paired over tasks) and make a graph
> query the first step. On these tasks that did **not** change accuracy, tokens
> or cost measurably: all variants were at or near the ceiling (F1 differences
> within ±0.03), and the token and cost intervals span roughly ±25 %. The
> subagent was never called. The matrix had no deferred-tools control, so it
> does not show how much of the jump from 0 % to 80 % is layer 2's doing (the
> task set and Claude Code version also changed since the pilot), and it did
> not test routing without hooks (B) or hooks without up-front tools (D).

The outcome eval ([#153](../../evals/agent/README.md#outcome-eval-153),
21 November 2026, [report](../../evals/agent/results/2026-11-21-outcome/REPORT.md))
compared variant C with no myceliums at all on 11 multi-hop Django tasks
under a budget of three rounds of tool calls (`--max-turns 4`, stated in the
prompt), Claude Code 2.1.330, one repetition:

| Condition | Sessions with ≥ 1 myceliums call | Success / mean F1 | Cost per session | Wall time |
|---|---:|---:|---:|---:|
| none: built-in read and search tools only | 0 % | 36 % / 0.362 | 0.076 USD | 24.0 s |
| C (layers 1-3) | 100 % | 100 % / 0.988 | 0.049 USD (0.65× [0.54×, 0.76×]) | 12.6 s (0.52×) |

> **What the numbers say, and what they do not.** Success rose by 64 pp
> [+36, +91] (paired over 11 tasks): with C the agent answered each question
> with one `find_callers` call at `depth` 2 or 3, while without myceliums it
> ran past the budget, without an answer, on every transitive-caller and
> same-name task. With seven rounds of tool calls the agent without myceliums
> had solved 5 of 6 of these tasks, so the gain is speed, which a budget turns
> into success; input tokens did not change measurably (0.93× [0.74×, 1.17×]).
> A pilot: n = 11, one repository, one model, and questions whose static call
> rules a call graph computes by construction.

This guide describes five ways to route an agent to myceliums, from soft to
hard, and what `myc setup-claude` installs for each.

## The five layers

| Layer | What | Measured (#125, Claude Code 2.1.329) | Cost | Status |
|---|---|---|---|---|
| 1. Instructions | Routing rules in CLAUDE.md / AGENTS.md: structural questions (callers, impact, flows) go to myceliums, grep stays for literals, config and logs | together with layer 3 (variant C): myceliums used in 100 % of sessions instead of 80 %, first call a graph query; not measured alone | +348 tokens per request | `myc setup-claude`, `myc setup-codex` |
| 2. Tool surface | 9 task-named core tools, loaded up front instead of deferred; the other 51 opt-in by group | variant A: myceliums used in 80 % of sessions (pilot with 54 deferred tools: 0 %; not a controlled comparison) | the schemas are in every request (A's first prompt: 10,162 tokens) | `myc mcp` default; `myc setup-claude` (`alwaysLoad`) |
| 3. Hooks | A PreToolUse hook spots a grep for a known symbol and adds the matching `myc` call to the agent's context | ran on 175 Grep calls in C, hint added to 120 (69 %); the SessionStart status line was then a `systemMessage` the model did not see. Since #152 it adds a context line: in #153 it reached the model in 11 of 11 C sessions, and the grep hook added a hint to 14 of 17 Grep calls | a few ms per Grep (400 ms cap) | `myc setup-claude` |
| 4. Subagent | A `code-explorer` agent with only the myceliums tools, Read and ToolSearch; the main agent delegates structural questions | never called (0 of 38 sessions): with the tools in the main context the agent queries them itself | about 1,860 tokens per request more than C (Agent tool and its agent list) | `myc setup-claude` |
| 5. Custom harness | Your own agent loop prefetches a context pack (`myc context --json`) and injects it before the model call | not measured | you maintain it | [`examples/agent-sdk-prefetch/`](../../examples/agent-sdk-prefetch/) |

Layers 1, 3 and 4 work through the same one-shot CLI commands, which any
agent with a shell can call:

| Question | Command |
|---|---|
| Who calls X? | `myc callers X` (`--depth 3` for transitive callers) |
| What does X call? | `myc callees X` |
| What breaks if X or a file changes? | `myc impact X`, `myc impact src/file.rs` |
| Where/how is something implemented? | `myc context how are sessions refreshed` |

They print compact text (or `--json`), keep the text within `--budget`
estimated tokens (default 2000; one token is estimated as four characters;
`--json` returns the same items, which are larger because of the field
names), and end
with the index freshness (`index: abc1234 = HEAD`, or a warning when HEAD has
moved since `myc analyze`). An ambiguous name lists its candidate definitions
with file:line and exits with code 2; an unknown name suggests similar ones
and exits with code 1. They read the index that covers the working directory
and never fall back to another repository.

## `myc setup-claude`

```bash
cd your-project
myc analyze .
myc setup-claude                  # project scope: files to commit for the team
myc setup-claude --scope user     # or: every project, for you only
```

The files `setup-claude` writes after the analysis (`.mcp.json`, `.claude/`,
the routing block in `CLAUDE.md`) do not make the index stale: freshness
counts only files the analysis indexes, and the routing block is left out of
the index. An edit to `CLAUDE.md` outside the block is a change, as for any
indexed Markdown file.
Before #151 the MCP `analyze` tool counted them as edits and re-indexed (in
the #125 matrix once into the 300 s tool timeout, and once, called with
`skip_embeddings`, into an index without embeddings); a re-index now also
keeps the vectors of unchanged symbols.

What each layer writes:

| Layer | Project scope (default) | User scope | Opt out |
|---|---|---|---|
| MCP server (2. core tools loaded up front) | `.mcp.json` (core tools through their `_meta`) | `claude mcp add-json -s user` (fallback: `~/.claude.json`), with `alwaysLoad` | `--no-always-load` (user scope) |
| SessionStart hook: refresh the index, one context line for the model, a status line for you | `.claude/settings.json` | `~/.claude/settings.json` | – |
| PostToolUse hook: format myceliums tool results | `.claude/settings.json` | `~/.claude/settings.json` | – |
| 1. Routing block | `CLAUDE.md` | `~/.claude/CLAUDE.md` | `--no-routing` |
| 3. Grep redirect hook (PreToolUse) | `.claude/settings.json` | `~/.claude/settings.json` | `--no-redirect-hook` |
| 4. `code-explorer` subagent | `.claude/agents/code-explorer.md` | `~/.claude/agents/code-explorer.md` | `--no-agent` |

- **Re-running** is safe: every file ends up as if installed once, and an
  opt-out flag removes a layer that an earlier run installed.
- **Uninstall** with `myc setup-claude --uninstall` (add `--scope user` for
  the user install). It removes the `myceliums` MCP entry, the hook handlers
  that run `myc session`, `myc format-hook` or `myc hook-grep`, the routing
  block and the subagent file, and nothing else. At project scope, files that
  only held myceliums content are deleted.
- **Project scope** names `myc` by its name when `myc` is on `PATH`, so the
  files work on every machine that has it installed. Claude Code asks each
  user once to approve a project's MCP server; to pre-approve it for yourself,
  add `"enabledMcpjsonServers": ["myceliums"]` to `.claude/settings.local.json`.
- Earlier releases' `myc setup-claude` installed at user scope. If both are
  installed, the hooks run twice; remove one with
  `myc setup-claude --scope user --uninstall`.
- `myc setup` (the wizard) installs at user scope.

### SessionStart hook

`myc session . --yes --timeout 300 2>/dev/null; exit 0` runs when a session
starts, resumes, or restarts after `/clear` or compaction. It prints one JSON
object on stdout in the shape the
[hooks reference](https://code.claude.com/docs/en/hooks#sessionstart)
documents:

```json
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"myceliums code graph of repo flask-3735c01f (index: 22d9247 = HEAD). MCP tools find_callers, impact_of_change and search_code answer structural questions."},"systemMessage":"[myceliums] flask ready | 93 files · 1592 symbols"}
```

- `additionalContext` goes into the model's context before the first prompt:
  the repository id, the index state and the core tools. It is one line of at
  most 280 characters (about 60 tokens), because every session pays for it.
  The index state is one of:
  - the freshness (`index: <commit> = HEAD`, or a stale warning when HEAD
    has moved);
  - `stale, re-indexing in the background; results may lag until done, and
    new symbols lack embeddings until a full myc analyze`;
  - `no index of repo … yet, building in the background; … without
    embeddings until a full myc analyze` (the embeddings note is left out
    with `--strict-embeddings`);
  - `… already running`, when another process holds the analysis lock.

  The tools sentence is left out when you declined AI instructions in the
  `myc setup` wizard.
- `systemMessage` is the status line Claude Code shows you, not the model.
  The same line goes to stderr for a run by hand.
- On a skip (no git repository, home directory, no supported files) or an
  error, the hook adds no context, only the status line. It always exits 0;
  `; exit 0` covers a crash or an older `myc`.

**Runtime.** Claude Code waits for SessionStart hooks before the first model
response, so the hook never analyzes in the foreground. It reads the
registry, runs the cache check and `git rev-parse HEAD` (bounded to 1.5 s),
then returns: 0.05 to 0.11 s on flask with a debug build, whatever the index
state. A missing or stale index is rebuilt by a detached `myc analyze .
--force --skip-embeddings` (`--strict-embeddings` when the hook got it). It
runs in its own session (`setsid`), so it is not tied to the hook's process
group or terminal, and appends its output to
`<data dir>/logs/session-<repo id>.log` (cut at 1 MB). It takes the analysis
lock that MCP `analyze` and `myc analyze` share. If the lock is already held,
the hook starts nothing and reports the run in progress. A second session
racing the first loses at the lock and exits. The re-index has no time limit:
on flask it took 26 s with a debug build. Until it finishes, the graph tools
answer from the previous index, or fail if there is none. `--timeout` is
ignored since #152; installed hooks still pass it, which bounds an older
`myc`. The handler carries `"timeout": 15`, so Claude Code stops a hook that
hangs anyway.

### 1. Routing block

A block of under 15 lines between `<!-- BEGIN myceliums -->` and
`<!-- END myceliums -->`. It is loaded on every turn, so it is short: it
names the commands above, the MCP tools and the subagent as the place for
structural questions, and grep/rg as the place for string literals, config
values, log messages and non-code files. The block is versioned (`v2` in its
first comment line); everything between the markers is replaced on re-run,
and text outside them is never touched. Put your own rules outside the
markers.

`myc setup-codex` writes the same block, without the subagent line, into the
project's `AGENTS.md` (when run inside a git repository; `--no-routing` skips
it).

### 3. Grep redirect hook

`myc hook-grep` runs before a `Grep` tool call and before a `Bash` call that
runs `grep`, `rg` or `git grep` (the hook's `if` filter keeps it off other
Bash calls). When the search pattern looks like an identifier
(`parse_query`, `parseQuery`, `fn parse_query`, `\.AsString\(`,
`Item.AsString`) and the project's index defines it as a function, method or
type, the hook adds one or two sentences to the agent's context, for example:

```text
`parseAccept` is a Function defined at src/utils/accept.ts:211 in the myceliums index.
`myc callers parseAccept` lists its 4 direct callers with file:line, `myc callees parseAccept`
what it calls, `myc impact parseAccept` the files that depend on it.
```

It is silent for everything else: plain words (`error`), phrases, regexes
with character classes or quantifiers, searches restricted to config or prose
files (`*.json`, `*.yaml`, `*.md`, ...), names the index does not define, and
directories without an index. It never blocks or approves the tool call (it
sets no permission decision), exits 0 on any failure, and gives up silently
if the index lookup takes longer than 400 ms (`MYC_HOOK_DEADLINE_MS` changes
the limit; `MYC_HOOK_DEBUG=1` prints why the hook stayed silent on stderr).
The installed command is `myc hook-grep 2>/dev/null || true`: a PreToolUse
hook that exits with code 2 blocks the tool call, and an older `myc` without
`hook-grep` (say, on a teammate's machine) would exit 2 on the unknown
subcommand. The hook's input and output
follow the [Claude Code hooks reference](https://code.claude.com/docs/en/hooks).

### 4. `code-explorer` subagent

`.claude/agents/code-explorer.md` defines a subagent whose description asks
the main agent to delegate structural questions ("who calls", "what breaks",
"where is X implemented") to it. Its tools are the whole myceliums MCP server
(`mcp__myceliums__*`), `Read` to confirm results, and `ToolSearch` in case the
session defers MCP tool schemas (the non-core groups). Its instructions name
the core tools; they default to the repository of the working directory, and
`list_repos` lists the others. It has no Bash: the subagent
format can give or withhold a tool, but cannot limit Bash to `myc` commands.

The file carries a marker comment in its frontmatter. Delete that line to make
the file yours: `myc setup-claude` then neither overwrites nor removes it.

## 2. Core toolset

`myc mcp` exposes nine tools by default, each named after a question agents
ask: `search_code` (where is X implemented), `explain_symbol` (X with its
source, callers and callees), `find_callers`, `find_callees`,
`impact_of_change` (what breaks if this symbol, file or diff changes),
`trace_flow` (how control gets from A to B), `find_dead_code`, `list_repos`
and `analyze`. They answer through the same `myceliums_core::navigation`
functions as `myc callers`, `myc callees` and `myc impact`, so the CLI, the
grep hook's hints and the MCP tools agree. The other 51 tools are opt-in by
group (`myc mcp --toolsets core,architecture`, or the `mcp.toolsets` config
key; `all` for everything); see [Toolsets](../reference/mcp-tools.md#toolsets).

Two settings put the core tools in front of the model instead of behind tool
search ([Claude Code docs](https://code.claude.com/docs/en/mcp)):

- Each core tool carries `"_meta": {"anthropic/alwaysLoad": true}` in
  `tools/list`, so it loads up front whatever the client configuration says,
  unless the server entry sets `"alwaysLoad": false`.
- At user scope, `myc setup-claude` sets `"alwaysLoad": true` on the
  `myceliums` server entry when the server serves the core toolset only (the
  default). Then Claude Code also waits for the server (at most 5 s) before
  it builds the first prompt, so the tools are there from the first turn.
  With extra groups configured it leaves the key out, so they do not all load
  into every request; `--no-always-load` writes `false` and keeps every
  myceliums tool deferred. A project's `.mcp.json` never gets the key (see
  Team mode).

In the #125 matrix all nine were sent with their full schemas in every
session (none deferred), and the agent called a myceliums tool in 80 % of the
sessions that had only the server; `find_callers`, `find_callees` and
`trace_flow` took most of the calls.

## 5. Custom harness: prefetch a context pack

If you run your own agent loop (for example with the Claude Agent SDK), you
can skip tool discovery: build a context pack before the model call and put it
in the prompt.

```bash
myc context --json --budget 1500 "Why does the login redirect loop?"
```

The JSON holds `query`, `budget_tokens`, `estimated_tokens`, `truncated`,
`symbols` (each with `name`, `qualified_name`, `kind`, `file`, `line`,
`end_line`, `signature`, `reason`, up to five `callers` and `callees` with
their totals), `files`, `repo` and `freshness`. Symbols the text names
(`parse_query`, `Item.AsString`) come first, then lexical search hits.
`--hybrid` adds vector search when the index has embeddings; it loads the
embedding model, which takes seconds.

[`examples/agent-sdk-prefetch/`](../../examples/agent-sdk-prefetch/) shows the
whole loop in a short Python script.

## Team mode

Commit the project-scope files from `myc setup-claude` so every clone gets
the same MCP entry, routing block, hooks and subagent; each developer needs
`myc` on `PATH`. The CLI commands and the grep hook read a local index: the
SessionStart hook builds and refreshes it, or point `MYCELIUMS_DATA_DIR` at an
index built elsewhere (the CLI, the hooks and `myc mcp` all honour it). A
central index built in CI can also be served with
`myc mcp --http <addr> --allow-remote`, which requires a bearer token
(`MYC_MCP_TOKEN`); that serves the MCP tools only, not the CLI commands.

The committed `.mcp.json` carries no `"alwaysLoad"`: a server-level
`true` loads every tool the server lists, and each teammate's `myc mcp` lists
what their own `mcp.toolsets` says (or 55 tools on an older `myc`). The core
tools load up front anyway, through their own `_meta`. Whoever also wants
Claude Code to wait for the server before the first prompt adds a personal
local-scope entry, which replaces the project entry for them (Claude Code
uses one scope's whole entry, local before project before user, and does not
merge fields):

```bash
claude mcp add-json -s local myceliums '{"type":"stdio","command":"myc","args":["mcp"],"alwaysLoad":true}'
```

A tool call without `repo_id` uses the index that covers the server's working
directory. Outside every indexed directory it fails with a hint to run
`analyze` there or to pass a `repo_id`; it never answers from another
repository's index, and every result's `Index:` line names the repository it
read.
