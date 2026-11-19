# Integrating myceliums with coding agents

A code knowledge graph only helps an agent that uses it. The agent pilot
([#99](../../evals/agent/README.md), 12 November 2026) measured the weakest
integration, the MCP server attached and nothing else: Claude Code called a
myceliums tool in **0 of 90** sessions. Claude Code defers MCP tool schemas
behind tool search, so the agent saw 54 tool names and never loaded one, and
nothing in its context sent it from grep to the graph. (The pilot ran myc
0.3.2; since #97 the server's default is a 9-tool core toolset loaded up
front, layer 2 below.)

This guide describes five ways to route an agent to myceliums, from soft to
hard, and what `myc setup-claude` installs for each.

> **What is measured and what is not.** The only measured result so far is the
> pilot above (MCP server only: 0 % tool-call rate, +25 % input tokens, no
> accuracy change on an easy task set). How much each layer below changes
> tool use, accuracy and cost is being measured in
> [#125](https://github.com/marcmantei/myceliums/issues/125) (variants A–E on a
> harder task set). Until those numbers are in, treat the "effect" column as
> the design intent, not a result. The recommended default will follow the
> measurements.

## The five layers

| Layer | What | Intended effect | Risk | Status |
|---|---|---|---|---|
| 1. Instructions | Routing rules in CLAUDE.md / AGENTS.md: structural questions (callers, impact, flows) go to myceliums, grep stays for literals, config and logs | cheap, only a hint | ignored under pressure | `myc setup-claude`, `myc setup-codex` |
| 2. Tool surface | 9 task-named core tools, loaded up front instead of deferred; the other 50 opt-in by group | the model sees and understands them | each up-front schema costs context in every request | `myc mcp` default; `myc setup-claude` (`alwaysLoad`) |
| 3. Hooks | A PreToolUse hook spots a grep for a known symbol and adds the matching `myc` call to the agent's context | fires at the moment the agent reaches for grep | blocking grep would be wrong, so it never blocks | `myc setup-claude` |
| 4. Subagent | A `code-explorer` agent with only the myceliums tools, Read and ToolSearch; the main agent delegates structural questions | clean separation, good for teams | one extra hop | `myc setup-claude` |
| 5. Custom harness | Your own agent loop prefetches a context pack (`myc context --json`) and injects it before the model call | full control | you maintain it | [`examples/agent-sdk-prefetch/`](../../examples/agent-sdk-prefetch/) |

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

What each layer writes:

| Layer | Project scope (default) | User scope | Opt out |
|---|---|---|---|
| MCP server (2. core tools loaded up front) | `.mcp.json` (core tools through their `_meta`) | `claude mcp add-json -s user` (fallback: `~/.claude.json`), with `alwaysLoad` | `--no-always-load` (user scope) |
| SessionStart hook: refresh the index, print a status line | `.claude/settings.json` | `~/.claude/settings.json` | – |
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
grep hook's hints and the MCP tools agree. The other 50 tools are opt-in by
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

How much this changes tool use is measured in
[#125](https://github.com/marcmantei/myceliums/issues/125) (variant C).

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
