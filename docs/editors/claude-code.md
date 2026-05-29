# Claude Code Integration

Claude Code gets the deepest integration with myceliums. In addition to the MCP server, the setup installs hooks that keep the knowledge graph fresh and format tool output for readability.

## Setup

```bash
myc setup-claude
```

Or let `myc setup` auto-detect Claude Code on your system.

## What gets configured

The setup command writes to two files:

### 1. MCP server in `~/.claude.json`

Registers myceliums as an MCP server so Claude Code can call tools like `context_search`, `symbol_context`, and `detect_impact`.

```json
{
  "mcpServers": {
    "myceliums": {
      "type": "stdio",
      "command": "myc",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

The setup first tries `claude mcp add -s user myceliums -- myc mcp` (the official CLI method). If the `claude` binary is not available, it edits `~/.claude.json` directly.

### 2. Hooks in `~/.claude/settings.json`

Two hooks are installed:

**SessionStart hook** runs on every new Claude Code session:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "myc session . --yes --timeout 300 2>/dev/null; exit 0"
          }
        ]
      }
    ]
  }
}
```

This checks whether the current project has a cached analysis. If the cache is fresh, it outputs a status line. If the cache is stale or missing, it re-analyzes the project automatically. The `--timeout 300` flag limits analysis to 5 minutes to prevent runaway processes on very large codebases.

On session start you will see something like:

```
SessionStart:startup says: [myceliums] my-project ready | 312 files . 1,247 symbols
```

**PostToolUse hook** runs after any myceliums MCP tool call:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "mcp__myceliums__",
        "hooks": [
          {
            "type": "command",
            "command": "myc format-hook 2>/dev/null"
          }
        ]
      }
    ]
  }
}
```

This formats MCP tool output for better readability in the Claude Code interface.

## AI instructions

When enabled via the setup wizard, the session status message includes guidance for Claude to prefer myceliums tools over grep/find for code structure questions. This reduces token usage because myceliums returns structured results in a single call instead of requiring multiple grep+read rounds.

## Token savings

Myceliums reduces token consumption by replacing multi-step file-reading workflows with single structured queries. For details on how this works, see the [Token Savings Guide](../guides/token-savings.md).

## Uninstall

```bash
myc setup-claude --uninstall
```

This removes the MCP server entry from `~/.claude.json` and both hooks from `~/.claude/settings.json`. Other MCP servers and hooks you may have configured are preserved.

## Troubleshooting

### "Hook causes high CPU on session start"

The SessionStart hook has a 5-minute timeout (`--timeout 300`). For very large codebases, the initial analysis can take significant time and resources. Run `myc status` to check the size of the indexed project. If analysis consistently takes too long, consider using `--skip-embeddings` in your project's `.myceliums.toml` to speed things up.

### "I want to disable hooks but keep MCP"

Run `myc setup-claude --uninstall` to remove everything, then manually add just the MCP server entry to `~/.claude.json` (see the JSON snippet above). Alternatively, edit `~/.claude/settings.json` directly and remove the myceliums entries from the `SessionStart` and `PostToolUse` arrays.

### "MCP tools not showing up in Claude Code"

1. Verify that `myc mcp` runs without errors from your terminal.
2. Check that `~/.claude.json` contains the `myceliums` entry under `mcpServers`.
3. Restart Claude Code. MCP servers are loaded on session start.
4. Run `myc doctor` to check for installation issues.

### "SessionStart hook fails silently"

The hook command ends with `; exit 0` so failures do not block your session. To debug, run the hook command manually:

```bash
myc session . --yes --timeout 300
```

This will show any errors directly in your terminal.
