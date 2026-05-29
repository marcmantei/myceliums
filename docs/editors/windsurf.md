# Windsurf Integration

## Setup

```bash
myc setup-windsurf
```

Or let `myc setup` auto-detect Windsurf on your system.

## Config path

| Platform | Path |
|----------|------|
| macOS / Linux | `~/.windsurf.json` |
| Windows | `%USERPROFILE%/.windsurf.json` |

## What gets configured

The setup adds a `myceliums` entry to the `mcpServers` section:

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

If the file does not exist, it is created. Existing MCP server entries are preserved.

## After setup

The myceliums tools become available in Windsurf's AI chat. Make sure your project has been analyzed first (`myc analyze .`).

You may need to restart Windsurf for the MCP server to be picked up.

## Uninstall

```bash
myc setup-windsurf --uninstall
```

This removes the `myceliums` entry from the config file. Other MCP servers are preserved.
