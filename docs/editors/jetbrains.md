# JetBrains IDE Integration

## Setup

```bash
myc setup-jetbrains
```

Or let `myc setup` auto-detect JetBrains IDEs on your system.

## Supported IDEs

The setup command detects and configures all installed JetBrains IDEs:

- IntelliJ IDEA
- PyCharm
- WebStorm
- GoLand
- RubyMine
- CLion
- DataGrip
- Rider

Each detected IDE gets its own MCP configuration file.

## Config path

The base config directory is platform-specific:

| Platform | Base path |
|----------|-----------|
| macOS | `~/Library/Application Support/JetBrains/` |
| Linux | `~/.config/JetBrains/` |
| Windows | `%APPDATA%/JetBrains/` |

Within the base directory, each IDE version has its own folder (e.g., `IntelliJIdea2024.1`, `PyCharm2024.2`). The setup command writes the MCP config to:

```
<base>/<IDE version>/options/mcp_config.json
```

## What gets configured

Each detected IDE receives the following in its `mcp_config.json`:

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

If the file already exists and contains other MCP servers, only the `myceliums` entry is added or updated.

## After setup

Restart your JetBrains IDE for the MCP server to be loaded. The myceliums tools become available through the IDE's AI assistant features.

## Uninstall

```bash
myc setup-jetbrains --uninstall
```

This removes the `myceliums` entry from `mcp_config.json` in all detected JetBrains IDE config directories. Other MCP server entries are preserved.
