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
| MCP server `myceliums` | `.mcp.json` | `claude mcp add -s user` (fallback: `~/.claude.json`) | – |
| SessionStart hook | `.claude/settings.json` | `~/.claude/settings.json` | – |
| PostToolUse hook | `.claude/settings.json` | `~/.claude/settings.json` | – |
| PreToolUse grep-redirect hook | `.claude/settings.json` | `~/.claude/settings.json` | `--no-redirect-hook` |
| Routing block | `CLAUDE.md` | `~/.claude/CLAUDE.md` | `--no-routing` |
| `code-explorer` subagent | `.claude/agents/code-explorer.md` | `~/.claude/agents/code-explorer.md` | `--no-agent` |

Re-running `myc setup-claude` is idempotent, and an opt-out flag removes a
layer that an earlier run installed. Your own entries in these files are kept.
A settings or MCP file that is not valid JSON is reported and left untouched.

### MCP server

Registers myceliums as an MCP server so Claude Code can call tools like `search_code`, `find_callers`, and `impact_of_change`. These nine core tools mark themselves to load up front (`_meta["anthropic/alwaysLoad"]`), so they are in context from the first turn instead of behind tool search; see [Toolsets](../reference/mcp-tools.md#toolsets).

```json
{
  "mcpServers": {
    "myceliums": { "type": "stdio", "command": "myc", "args": ["mcp"] }
  }
}
```

At project scope the command is `myc` when `myc` is on `PATH` (so the file
works on every machine), otherwise the absolute path of the running binary.
Claude Code asks each user once to approve a project's MCP server.

### Hooks

```json
{
  "hooks": {
    "SessionStart": [
      { "matcher": "", "hooks": [{ "type": "command",
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
  re-analyzes (skipping embeddings, at most 5 minutes) and prints a status
  line such as `[myceliums] my-project ready | 312 files · 1,247 symbols`.
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

### Instructions in the status line

When enabled in the setup wizard (`setup.instructions_enabled`), the
SessionStart status line also suggests the myceliums tools for code-structure
questions.

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

The SessionStart hook has a 5-minute timeout (`--timeout 300`). For very large codebases, the initial analysis can take significant time and resources. Run `myc status` to check the size of the indexed project. If analysis consistently takes too long, consider using `--skip-embeddings` in your project's `.myceliums.toml` to speed things up.

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
