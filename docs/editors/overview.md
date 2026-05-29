# Editor Integration Overview

Myceliums runs as an [MCP](https://modelcontextprotocol.io/) server over stdio. Any editor or AI assistant that supports the Model Context Protocol can connect to it. The `myc setup` command handles registration automatically.

## What setup does

Running a setup command registers myceliums as an MCP server in the editor's configuration file. This tells the editor to spawn `myc mcp` as a background process when a session starts. The process is lightweight and stays idle until the AI assistant calls a tool.

Claude Code is a special case: in addition to the MCP server, it also installs **SessionStart** and **PostToolUse** hooks. See [Claude Code Integration](claude-code.md) for details.

## Supported editors

| Editor | Setup command | Config path | Notes |
|--------|--------------|-------------|-------|
| Claude Code | `myc setup-claude` | `~/.claude.json` + `~/.claude/settings.json` | MCP server + hooks (SessionStart, PostToolUse) |
| Cursor | `myc setup-cursor` | `~/.cursor/mcp.json` | Restart Cursor after setup |
| VS Code | `myc setup-vscode` | Platform-specific `settings.json` | Works with Copilot Chat and other MCP extensions |
| Windsurf | `myc setup-windsurf` | `~/.windsurf.json` | |
| Zed | `myc setup-zed` | `~/.config/zed/settings.json` | Uses `context_servers` key instead of `mcpServers` |
| JetBrains IDEs | `myc setup-jetbrains` | Platform-specific JetBrains config dir | IntelliJ, PyCharm, WebStorm, GoLand, RubyMine, CLion, DataGrip, Rider |
| Gemini CLI | `myc setup-gemini` | `~/.gemini/settings.json` | |
| Codex CLI | `myc setup-codex` | `~/.codex/config.json` | |
| Copilot CLI | `myc setup-copilot` | `~/.github-copilot/config.json` | |
| Aider | `myc setup-aider` | `~/.aider/mcp.json` | |
| Kiro | `myc setup-kiro` | `~/.kiro/mcp.json` | |
| Continue | `myc setup-continue` | `~/.continue/config.json` | |
| Any MCP-compatible editor | Manual | Varies | Point to `myc mcp` as a stdio command |

> Config paths shown are for macOS/Linux. Windows paths differ for VS Code (`%APPDATA%/Code/User/settings.json`), JetBrains (`%APPDATA%/JetBrains/`), Zed (`%APPDATA%/Zed/settings.json`), and Windsurf (`%USERPROFILE%/.windsurf.json`).

## Auto-detection

```bash
myc setup
```

This scans your system for installed editors and configures each one it finds. It checks for editor executables in your PATH and for existing config directories. You will see a confirmation for each editor that gets configured.

## Manual setup (single editor)

```bash
myc setup --editor <name>
# or use the shorthand:
myc setup-claude
myc setup-cursor
myc setup-vscode
# ... etc.
```

## Uninstall

Add `--uninstall` to any setup command to remove the myceliums integration:

```bash
myc setup-claude --uninstall
myc setup-cursor --uninstall
myc setup-jetbrains --uninstall
```

This removes the `myceliums` entry from the editor's MCP server config. For Claude Code, it also removes the hooks from `~/.claude/settings.json`.

## Generic MCP config

For editors not listed above, add this to the editor's MCP configuration:

```json
{
  "mcpServers": {
    "myceliums": {
      "command": "myc",
      "args": ["mcp"]
    }
  }
}
```

If `myc` is not in your PATH, use the full path to the binary (e.g., `/usr/local/bin/myc`).
