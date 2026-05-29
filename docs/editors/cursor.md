# Cursor Integration

## Setup

```bash
myc setup-cursor
```

Or let `myc setup` auto-detect Cursor on your system.

## Config path

```
~/.cursor/mcp.json
```

## Manual configuration

If you prefer to configure manually, add the following to `~/.cursor/mcp.json`:

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

If the file does not exist yet, create it with the content above. If it already exists and contains other MCP servers, add the `"myceliums"` entry inside the existing `"mcpServers"` object.

## After setup

Cursor loads the MCP server when it starts. The myceliums tools (`context_search`, `symbol_context`, `detect_impact`, `cypher_query`, etc.) become available in Cursor's AI chat.

You may need to restart Cursor after running the setup command for the MCP server to be picked up.

## Tips

- Add Cursor rules (`.cursorrules` or project-level rules) to guide the AI toward using myceliums tools for code structure questions. For example: "For questions about code architecture, symbol dependencies, or impact analysis, prefer myceliums tools over grep."
- Make sure your project has been analyzed first (`myc analyze .`) so the knowledge graph has data to query.

## Uninstall

```bash
myc setup-cursor --uninstall
```

This removes the `myceliums` entry from `~/.cursor/mcp.json`. Other MCP servers are preserved.
