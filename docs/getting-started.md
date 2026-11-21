# Getting Started with Myceliums

Get your codebase indexed and queryable in under 2 minutes.

## Prerequisites

- **Rust/Cargo 1.70+** (for building from source) or a pre-built binary
- A codebase you want to analyze

## Installation

Pick one of the three methods below.

### Option A: Install from crates.io

```bash
cargo install myc
```

To pin an exact version rather than tracking the latest release, add
`--version 0.3.2 --locked`.

### Option B: Homebrew

```bash
brew install marcmantei/tap/myc
```

Installs a pre-built binary, so there is nothing to compile. Available for macOS
(Apple Silicon and Intel) and Linux (aarch64 and x86_64).

### Option C: Build from source

```bash
git clone https://github.com/marcmantei/myceliums.git
cd myceliums
cargo build --release
# The binary is at ./target/release/myc
```

## Step 1: Run the Setup Wizard

The setup wizard auto-detects your installed editors and configures MCP integration for each one.

```bash
myc setup
```

This creates `~/.myceliums/config.toml` with sensible defaults and wires up MCP for every supported editor it finds on your machine.

## Step 2: Analyze Your First Project

Point `myc` at a project directory to build the knowledge graph.

```bash
myc analyze ./my-project
```

On the first run, the embedding model downloads automatically, with a progress bar: about 644 MB for the default model, `jina-embeddings-v2-base-code` (sizes per model are in the README's model table). It is cached for all future runs. The first analysis of a project embeds every symbol and takes the longest; later runs reuse the vectors of unchanged symbols. For a faster first index, set `model = "multilingual-e5-small"` and `chunking = false` in the `[embedding]` section of `.myceliums.toml`. If you want to skip embeddings entirely, pass `--skip-embeddings`.

## Step 3: Check the Results

Once analysis finishes, you can explore the knowledge graph from the command line.

**View stats about the indexed codebase:**

```bash
myc stats
```

**Search for a symbol by name:**

```bash
myc search "function_name"
```

**List detected code communities (clusters of related symbols):**

```bash
myc communities
```

## Using with Your Editor

After running `myc setup`, the MCP tools are available inside your editor's AI assistant. You do not need to do anything extra. The assistant can call tools like `search_code`, `find_callers`, `impact_of_change`, and `explain_symbol` to answer questions about your codebase directly in the chat. These core tools are on by default; more groups are opt-in with `myc mcp --toolsets` (see [Toolsets](reference/mcp-tools.md#toolsets)).

## Next Steps

- [Editor setup details](editors/overview.md)
- [How myceliums saves tokens](guides/token-savings.md)
- [Full command reference](reference/commands.md)
