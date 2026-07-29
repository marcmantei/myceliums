# Getting Started with Myceliums

Get your codebase indexed and queryable in under 2 minutes.

## Prerequisites

- **Rust/Cargo 1.70+** (for building from source) or a pre-built binary
- A codebase you want to analyze

## Installation

Pick one of the three methods below.

### Option A: Install with Cargo

`myc` is not published to crates.io yet, so install it from the git tag of a
release. Pin the tag — installing from the default branch gives you whatever is
on `main` at that moment, which is not reproducible.

```bash
cargo install --git https://github.com/marcmantei/myceliums --tag v0.3.2 --locked myc
```

### Option B: Build from source

```bash
git clone https://github.com/marcmantei/myceliums.git
cd myceliums
cargo build --release
# The binary is at ./target/release/myc
```

### Option C: Docker

```bash
docker pull myceliums/myc:latest
docker run --rm -v "$PWD:/repo" myceliums/myc:latest analyze /repo
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

On the first run, the fastembed model (~100 MB) downloads automatically. This is cached for all future runs. If you want to skip embeddings entirely, pass `--skip-embeddings`.

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

After running `myc setup`, the MCP tools are available inside your editor's AI assistant. You do not need to do anything extra. The assistant can call tools like `context_search`, `symbol_context`, `detect_impact`, and `cypher_query` to answer questions about your codebase directly in the chat.

## Next Steps

- [Editor setup details](editors/overview.md)
- [How myceliums saves tokens](guides/token-savings.md)
- [Full command reference](reference/commands.md)
