# Claude Code Integration

Claude Code gets the deepest integration with myceliums: besides the MCP
server, `myc setup-claude` installs hooks, a CLAUDE.md routing block and a
`code-explorer` subagent. Why each piece exists, and what is and is not
measured about it, is in the
[agent integration guide](../guides/agent-integration.md).

## Setup

```bash
cd your-project
myc analyze .
myc setup-claude                  # project scope (default): files to commit
myc setup-claude --scope user     # user scope: every project, only for you
```

`myc setup` (and the setup wizard) configures Claude Code at user scope.

## What gets configured

| Piece | Project scope | User scope | Opt out |
|---|---|---|---|
| MCP server `myceliums`, core tools loaded up front | `.mcp.json` (no `alwaysLoad`; the tools' `_meta`) | `claude mcp add-json -s user` (fallback: `~/.claude.json`), with `alwaysLoad` | `--no-always-load` (user scope) keeps the tools deferred |
| SessionStart hook | `.claude/settings.json` | `~/.claude/settings.json` | – |
| PostToolUse hook | `.claude/settings.json` | `~/.claude/settings.json` | – |
| PreToolUse grep-redirect hook | `.claude/settings.json` | `~/.claude/settings.json` | `--no-redirect-hook` |
| Routing block | `CLAUDE.md` | `~/.claude/CLAUDE.md` | `--no-routing` |
| `code-explorer` subagent | `.claude/agents/code-explorer.md` | `~/.claude/agents/code-explorer.md` | `--no-agent` |

Re-running `myc setup-claude` is idempotent, and an opt-out flag removes a
layer that an earlier run installed. Your own entries in these files are kept.
A settings or MCP file that is not valid JSON is reported and left untouched.

### MCP server

Registers myceliums as an MCP server so Claude Code can call tools like `search_code`, `find_callers`, and `impact_of_change`. By default `myc mcp` serves the nine core tools, and each carries `"_meta": {"anthropic/alwaysLoad": true}` in `tools/list`: Claude Code loads them into context at session start instead of deferring them behind tool search, while opt-in groups stay deferred; see [Toolsets](../reference/mcp-tools.md#toolsets).

**Project scope** (`.mcp.json`, shared with the team) writes no `alwaysLoad`:

```json
{
  "mcpServers": {
    "myceliums": { "type": "stdio", "command": "myc", "args": ["mcp"] }
  }
}
```

A server-level `"alwaysLoad": true` loads every tool the server lists, and
what a teammate's `myc mcp` lists depends on their `mcp.toolsets` (or on an
older `myc` with 55 tools), so the shared file leaves loading to the tools'
own `_meta`. The difference: the server-level key also makes Claude Code wait
for the server (at most 5 s) before the first prompt. To get that for
yourself only, add a local-scope entry in the project directory:

```bash
claude mcp add-json -s local myceliums '{"type":"stdio","command":"myc","args":["mcp"],"alwaysLoad":true}'
```

Claude Code uses the whole entry of the highest-precedence scope — local,
then project, then user — and does not merge fields across scopes, so the
local entry replaces the project one for you (keep its `command` and `args`
in step with `.mcp.json`). `"alwaysLoad": false` in a local entry keeps every
myceliums tool deferred (Claude Code 2.1.287 or later).

**User scope** (`--scope user`, the wizard) is personal, so `setup-claude`
writes the key there: `"alwaysLoad": true` when `mcp.toolsets` is the default
(core only), no key when you have added groups (they stay deferred; the core
tools still load through their `_meta`), and `false` with `--no-always-load`.
Re-run it after changing `mcp.toolsets`. The entry is registered with
`claude mcp add-json`, which, unlike `claude mcp add`, carries `alwaysLoad`.

At project scope the command is `myc` when `myc` is on `PATH` (so the file
works on every machine), otherwise the absolute path of the running binary.
Claude Code asks each user once to approve a project's MCP server.

### Hooks

```json
{
  "hooks": {
    "SessionStart": [
      { "matcher": "", "hooks": [{ "type": "command", "timeout": 15,
        "command": "myc session . --yes --timeout 300 2>/dev/null; exit 0" }] }
    ],
    "PostToolUse": [
      { "matcher": "mcp__myceliums__.*", "hooks": [{ "type": "command",
        "command": "myc format-hook 2>/dev/null" }] }
    ],
    "PreToolUse": [
      { "matcher": "Grep", "hooks": [{ "type": "command", "command": "myc hook-grep 2>/dev/null || true", "timeout": 5 }] },
      { "matcher": "Bash", "hooks": [
        { "type": "command", "if": "Bash(grep *)", "command": "myc hook-grep 2>/dev/null || true", "timeout": 5 },
        { "type": "command", "if": "Bash(rg *)", "command": "myc hook-grep 2>/dev/null || true", "timeout": 5 },
        { "type": "command", "if": "Bash(git grep *)", "command": "myc hook-grep 2>/dev/null || true", "timeout": 5 }
      ] }
    ]
  }
}
```

- **SessionStart** checks whether the project has a fresh index; if not, it
  starts a re-index in the background and returns at once.
  It adds one line to the model's context (`additionalContext`: repository
  id, index freshness, the core tools) and shows you a status line such as
  `[myceliums] my-project ready | 312 files · 1,247 symbols`. Details in the
  [guide](../guides/agent-integration.md#sessionstart-hook).
- **PostToolUse** formats myceliums tool results. Its matcher is the regex
  `mcp__myceliums__.*`: Claude Code compares a matcher made only of letters,
  digits and `_` as an exact tool name, so the bare `mcp__myceliums__` that
  earlier releases wrote never matched.
- **PreToolUse** runs `myc hook-grep` before a Grep call or a Bash call that
  runs grep, rg or git grep. If the pattern names a function, method or type
  the index knows, it adds the matching `myc callers` / `myc impact` call to
  the agent's context. It never blocks or approves the call and is silent
  otherwise. See the [guide](../guides/agent-integration.md#3-grep-redirect-hook).

### Routing block and subagent

The routing block (under 15 lines, between `<!-- BEGIN myceliums -->` and
`<!-- END myceliums -->`) tells the agent to take structural questions to
myceliums and keep grep for literals, config and logs. The subagent file
defines `code-explorer` with the myceliums MCP tools, Read and ToolSearch.
Both are described in the [guide](../guides/agent-integration.md).

### Tools in the session context

The SessionStart context line names `find_callers`, `impact_of_change` and
`search_code` for structural questions, unless you declined AI instructions
in the setup wizard (`setup.instructions_enabled = false`).

## Uninstall

```bash
myc setup-claude --uninstall                # project scope
myc setup-claude --scope user --uninstall   # user scope
```

This removes the `myceliums` MCP entry, the hook handlers that run
`myc session`, `myc format-hook` or `myc hook-grep` (including ones written by
earlier releases), the routing block, and the subagent file if it still has
its myceliums marker line. Other servers, hooks and text are kept. At project
scope, files that held nothing else are deleted.

## Token savings

Myceliums reduces token consumption by replacing multi-step file-reading workflows with single structured queries. For details on how this works, see the [Token Savings Guide](../guides/token-savings.md).

## Troubleshooting

### "Hook causes high CPU on session start"

The SessionStart hook returns at once and re-indexes in the background (`myc analyze`, log in `~/.myceliums/logs/session-<repo id>.log`), one run at a time per repository. For very large codebases, the initial analysis can take significant time and resources. Run `myc status` to check the size of the indexed project. If analysis consistently takes too long, consider using `--skip-embeddings` in your project's `.myceliums.toml` to speed things up.

### "I want only the MCP server"

Run `myc setup-claude --no-routing --no-redirect-hook --no-agent`, then remove
the SessionStart and PostToolUse entries from `.claude/settings.json` if you do
not want them either.

### "Hooks run twice"

Earlier releases installed at user scope. If both a user and a project install
exist, both sets of hooks run. Remove one: `myc setup-claude --scope user --uninstall`.

### "MCP tools not showing up in Claude Code"

1. Verify that `myc mcp` runs without errors from your terminal.
2. Check that `.mcp.json` (project) or `~/.claude.json` (user) contains the `myceliums` entry under `mcpServers`, and that you approved the project server (`/mcp` in Claude Code).
3. Restart Claude Code. MCP servers are loaded on session start.
4. Run `myc doctor` to check for installation issues.

### "SessionStart hook fails silently"

The hook command ends with `; exit 0` so failures do not block your session. To debug, run the hook command manually:

```bash
myc session . --yes --timeout 300
```

This will show any errors directly in your terminal.

### "The grep hook never says anything"

Run it by hand with a pattern the index should know:

```bash
echo '{"hook_event_name":"PreToolUse","cwd":"'"$PWD"'","tool_name":"Grep","tool_input":{"pattern":"your_function"}}' \
  | MYC_HOOK_DEBUG=1 myc hook-grep
```

With `MYC_HOOK_DEBUG=1` the hook prints why it stayed silent on stderr: the
pattern is not identifier-shaped, the index does not define it as a function,
method or type, no index covers `cwd` (`myc list`), or the lookup took longer
than its deadline (400 ms; `MYC_HOOK_DEADLINE_MS` changes it).
