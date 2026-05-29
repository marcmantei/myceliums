# VS Code Integration

## Setup

```bash
myc setup-vscode
```

Or let `myc setup` auto-detect VS Code on your system.

## Config path

The VS Code settings file location is platform-specific:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/Code/User/settings.json` |
| Linux | `~/.config/Code/User/settings.json` |
| Windows | `%APPDATA%/Code/User/settings.json` |

## What gets configured

The setup adds a `myceliums` entry to the `mcpServers` section of your VS Code settings:

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

Existing settings are preserved. Only the `myceliums` entry is added or updated.

## Compatible extensions

Myceliums works with any VS Code extension that supports the Model Context Protocol, including:

- **GitHub Copilot Chat** (with MCP support enabled)
- Other AI extensions that implement MCP client capabilities

Once the MCP server is registered, these extensions can call myceliums tools directly from the chat interface.

## After setup

Make sure your project has been analyzed (`myc analyze .`) before expecting results. You may need to reload VS Code (`Cmd+Shift+P` > "Reload Window") for the MCP server registration to take effect.

## Uninstall

```bash
myc setup-vscode --uninstall
```

This removes the `myceliums` entry from your VS Code settings. Other settings and MCP servers are preserved.
