# Zed Integration

## Setup

```bash
myc setup-zed
```

Or let `myc setup` auto-detect Zed on your system.

## Config path

| Platform | Path |
|----------|------|
| macOS / Linux | `~/.config/zed/settings.json` |
| Windows | `%APPDATA%/Zed/settings.json` |

## What gets configured

Zed uses the `context_servers` key instead of `mcpServers`. The setup adds a `myceliums` entry accordingly:

```json
{
  "context_servers": {
    "myceliums": {
      "command": "myc",
      "args": ["mcp"]
    }
  }
}
```

If the file already exists, only the `myceliums` entry is added inside the existing `context_servers` object. All other settings are preserved.

## Manual configuration

If you prefer to configure manually, open `~/.config/zed/settings.json` (or the equivalent path on your platform) and add the `myceliums` entry shown above inside the `context_servers` object.

## After setup

Restart Zed for the context server to be loaded. Once active, the myceliums tools become available through Zed's AI assistant.

Make sure your project has been analyzed first (`myc analyze .`) so the knowledge graph has data to query.

## Uninstall

```bash
myc setup-zed --uninstall
```

This removes the `myceliums` entry from `context_servers` in the Zed settings file. Other settings and context servers are preserved.
