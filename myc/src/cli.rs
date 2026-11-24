//! Command-line types: the clap parser and its subcommand enums.

use crate::commands;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "myc", version, about = "Myceliums — Code Knowledge Graph")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Analyze a codebase and build its knowledge graph
    Analyze {
        /// Path to the project directory
        path: PathBuf,
        /// Force full re-analysis even if cache is fresh
        #[arg(long)]
        force: bool,
        /// Maximum cache age in minutes before re-analysis (default: 60)
        #[arg(long, default_value = "60")]
        max_age: u64,
        /// Skip embedding generation (much faster, BM25/Cypher still work)
        #[arg(long)]
        skip_embeddings: bool,
        /// Exit non-zero if the index is not fully embedded (for CI: fail the
        /// build on a partial index instead of silently shipping missing
        /// vectors). Cannot be combined with --skip-embeddings.
        #[arg(long, conflicts_with = "skip_embeddings")]
        strict_embeddings: bool,
        /// Skip community detection (as `[community] enabled = false` does).
        /// Search, callers/callees, impact and flows do not need it; only the
        /// community tools stay empty
        #[arg(long)]
        skip_communities: bool,
        /// Watch for file changes and re-index incrementally
        #[arg(long)]
        watch: bool,
        /// Allow analyzing directories without a .git repository
        #[arg(long)]
        no_git_check: bool,
    },
    /// List all analyzed repositories
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a repository's analysis data
    Delete {
        /// Repository ID or path
        repo: String,
    },
    /// Show statistics for a repository
    Stats {
        /// Repository ID or path
        repo: String,
    },
    /// Search symbols in a repository
    Search {
        /// Search query
        query: String,
        /// Repository ID or path (uses most recent if omitted)
        #[arg(short, long)]
        repo: Option<String>,
        /// Maximum results to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Use hybrid search (BM25 + vector with RRF)
        #[arg(long)]
        hybrid: bool,
        /// Apply cross-encoder reranking to hybrid search results
        #[arg(long)]
        rerank: bool,
        /// Show scoring breakdown and graph paths for each result
        #[arg(long)]
        explain: bool,
        /// How to rank test code (test files and directories, #[cfg(test)]
        /// modules): below product code, like it, or above it
        #[arg(long, value_enum, default_value_t = TestsArg::Demote)]
        tests: TestsArg,
    },
    /// Show detected communities for a repository
    Communities {
        /// Repository ID or path
        repo: String,
    },
    /// Show traced processes for a repository
    Processes {
        /// Repository ID or path
        repo: String,
        /// Filter by entry point name (case-insensitive substring match)
        #[arg(long)]
        entry: Option<String>,
        /// Filter by keyword in process description/flow (case-insensitive substring match)
        #[arg(long)]
        filter: Option<String>,
        /// Limit number of processes to display
        #[arg(long)]
        limit: Option<usize>,
        /// Show only processes with N or more steps
        #[arg(long)]
        min_steps: Option<u32>,
    },
    /// Execute a Cypher query against the knowledge graph
    Query {
        /// Cypher query string
        query: String,
        /// Repository ID or path
        #[arg(short, long)]
        repo: Option<String>,
    },
    /// Initialize a .myceliums.toml config file in the current directory
    Init {
        /// Create the config with defaults without prompting
        #[arg(long)]
        default: bool,
    },
    /// Rename a symbol across the codebase
    Rename {
        /// Name of the symbol to rename
        symbol_name: String,
        /// New name for the symbol
        new_name: String,
        /// Repository ID or path (uses most recent if omitted)
        #[arg(short, long)]
        repo: Option<String>,
        /// Apply the rename (default: preview only)
        #[arg(long)]
        apply: bool,
    },
    /// What depends on a symbol or file, or the impact of the current git diff
    ///
    /// With a TARGET (symbol name or file path): every symbol that calls or
    /// imports it, transitively up to --depth, and the files they live in.
    /// Without one: the symbols touched by `git diff HEAD` (or --diff) and
    /// what they reach.
    Impact {
        /// Symbol (`parse_query`, `Item.AsString`) or file (`src/query.rs`)
        #[arg(conflicts_with = "diff")]
        target: Option<String>,
        /// Repository ID or path
        #[arg(short, long)]
        repo: Option<String>,
        /// Graph traversal depth (default: 2)
        #[arg(short, long, default_value = "2")]
        depth: u32,
        /// Diff string or path to a .diff/.patch file (if omitted, runs git diff HEAD)
        #[arg(long)]
        diff: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// With a TARGET: budget for the text output in estimated tokens
        /// (--json returns the same items)
        #[arg(long, default_value_t = commands::nav::DEFAULT_BUDGET)]
        budget: usize,
    },
    /// List the callers of a symbol with file:line (compact, for agents and hooks)
    ///
    /// Ambiguous names list the candidate definitions (exit code 2); unknown
    /// names suggest similar ones (exit code 1). The index used is the one
    /// covering the working directory, never another repository.
    Callers {
        /// Symbol name, qualified name or suffix (`parse_query`, `Item.AsString`)
        symbol: String,
        /// Follow calls transitively up to this many hops (1 = direct callers)
        #[arg(short, long, default_value = "1")]
        depth: u32,
        /// Repository ID or path (default: the index covering the working directory)
        #[arg(short, long)]
        repo: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Budget for the text output in estimated tokens (1 token ~ 4
        /// characters); --json returns the same items
        #[arg(long, default_value_t = commands::nav::DEFAULT_BUDGET)]
        budget: usize,
    },
    /// List what a symbol calls, with file:line (compact, for agents and hooks)
    Callees {
        /// Symbol name, qualified name or suffix
        symbol: String,
        /// Follow calls transitively up to this many hops (1 = direct callees)
        #[arg(short, long, default_value = "1")]
        depth: u32,
        /// Repository ID or path (default: the index covering the working directory)
        #[arg(short, long)]
        repo: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Budget for the text output in estimated tokens (1 token ~ 4
        /// characters); --json returns the same items
        #[arg(long, default_value_t = commands::nav::DEFAULT_BUDGET)]
        budget: usize,
    },
    /// Token-budgeted context pack for a task: relevant symbols, signatures,
    /// callers/callees and files. `--json` is the prefetch API for custom
    /// agent harnesses.
    Context {
        /// The task or question, as free text (quotes optional)
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Repository ID or path (default: the index covering the working directory)
        #[arg(short, long)]
        repo: Option<String>,
        /// Rank with hybrid BM25 + vector search (needs embeddings; loads the
        /// embedding model, so it takes seconds)
        #[arg(long)]
        hybrid: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Budget for the text output in estimated tokens (1 token ~ 4
        /// characters); --json returns the same items
        #[arg(long, default_value_t = commands::nav::DEFAULT_BUDGET)]
        budget: usize,
    },
    /// Semantic search for symbols using vector embeddings
    SemanticSearch {
        /// Search query (natural language)
        query: String,
        /// Repository ID or path (uses most recent if omitted)
        #[arg(short, long)]
        repo: Option<String>,
        /// Maximum results to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// How to rank test code (test files and directories, #[cfg(test)]
        /// modules): below product code, like it, or above it
        #[arg(long, value_enum, default_value_t = TestsArg::Demote)]
        tests: TestsArg,
    },
    /// Interactive session setup: checks cache, prompts for analysis if needed
    Session {
        /// Path to the project directory (defaults to current directory)
        path: Option<PathBuf>,
        /// Skip interactive prompt and auto-analyze if needed
        #[arg(long)]
        yes: bool,
        /// Ignored. Auto mode used to re-analyze in the foreground for at most
        /// this many seconds; since #152 it re-indexes in the background and
        /// returns at once. Kept so installed hooks that pass it keep working.
        #[arg(long, default_value = "300")]
        timeout: u64,
        /// Generate embeddings during session bootstrap and fail if the index
        /// is not fully embedded. Session startup skips embeddings by default
        /// for speed, so this opts into the slower, verified path.
        #[arg(long)]
        strict_embeddings: bool,
        /// Allow analyzing directories without a .git repository
        #[arg(long)]
        no_git_check: bool,
    },
    /// Show overview of all myceliums data, storage usage, and health
    Status,
    /// Clean up myceliums data (orphans, caches, or specific repos)
    Clean {
        /// Repository ID or path to clean
        repo: Option<String>,
        /// Remove orphaned data directories not tracked in the registry
        #[arg(long)]
        orphans: bool,
        /// Remove ALL myceliums data
        #[arg(long)]
        all: bool,
        /// Remove the fastembed model cache
        #[arg(long)]
        cache: bool,
        /// Skip confirmation prompts
        #[arg(long)]
        yes: bool,
    },
    /// Start the MCP server (stdio transport by default, or HTTP with --http)
    Mcp {
        /// Run as HTTP server instead of stdio on this address (e.g. "127.0.0.1:9999").
        /// Non-loopback addresses are refused unless --allow-remote is given.
        #[arg(long)]
        http: Option<String>,
        /// Allow a non-loopback --http address (e.g. "0.0.0.0:9999"). Every request
        /// then needs `Authorization: Bearer <token>`, taken from MYC_MCP_TOKEN or
        /// generated and printed at startup.
        #[arg(long, requires = "http")]
        allow_remote: bool,
        /// Tool groups to expose, comma-separated: core (always on), graph, review,
        /// architecture, knowledge, email, cross_repo, admin, deprecated, or all.
        /// Default: the `mcp.toolsets` config key, else core.
        #[arg(long, value_name = "LIST")]
        toolsets: Option<String>,
        /// Log format on stderr: `text` (default) or `json` (one object per line).
        /// Each tool call logs one INFO line (tool, repo, duration, result size,
        /// outcome); RUST_LOG overrides the default level.
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        log_format: LogFormat,
        /// Write local tool-call statistics (per-tool calls, errors, p50/p95
        /// latency, result bytes) as JSON to this file: every 30 s while calls
        /// come in, and at shutdown. Nothing is sent over the network.
        #[arg(long, value_name = "PATH")]
        stats_file: Option<PathBuf>,
    },
    /// Check the health of the Myceliums installation
    Doctor {
        /// Pre-download the fastembed model (~100 MB)
        #[arg(long)]
        download: bool,
        /// Repair instruction files that an older myc wrote and that hide the
        /// project's own (CLAUDE.md over AGENTS.md, or the reverse), in the
        /// current project
        #[arg(long)]
        fix: bool,
        /// With --fix: repair every indexed project too, not only the current
        #[arg(long, requires = "fix")]
        all_projects: bool,
    },
    /// Manage global configuration
    Configure {
        /// Set a configuration value (format: key=value)
        #[arg(short, long)]
        set: Option<String>,
        /// Reset configuration to defaults
        #[arg(short, long)]
        reset: bool,
    },
    /// Format MCP tool output for Claude Code PostToolUse hooks (reads JSON from stdin)
    #[command(name = "format-hook")]
    FormatHook,
    /// Claude Code PreToolUse hook: suggests the myc call when Grep or grep/rg
    /// searches for a known symbol (reads hook JSON from stdin, never blocks)
    #[command(name = "hook-grep")]
    HookGrep,
    /// Set up Claude Code integration: MCP server, hooks, routing block and
    /// code-explorer subagent (project scope by default)
    #[command(name = "setup-claude")]
    SetupClaude {
        /// Where to install: the project (.mcp.json, .claude/; commit them for
        /// the team) or the user (~/.claude.json, ~/.claude/)
        #[arg(long, value_enum, default_value = "project")]
        scope: commands::setup_claude::Scope,
        /// Project root for --scope project (default: git top level of the cwd)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Do not write the routing block (removes an existing one)
        #[arg(long)]
        no_routing: bool,
        /// Do not install the PreToolUse grep-redirect hook (removes an existing one)
        #[arg(long)]
        no_redirect_hook: bool,
        /// Do not install the code-explorer subagent (removes an existing one)
        #[arg(long)]
        no_agent: bool,
        /// Keep every myceliums tool behind tool search ("alwaysLoad": false)
        /// instead of loading the core tools up front
        #[arg(long)]
        no_always_load: bool,
        /// Remove everything myceliums installed at this scope
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Spacebot integration (MCP server config)
    #[command(name = "setup-spacebot")]
    SetupSpacebot {
        /// Remove myceliums from Spacebot
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up OpenClaw integration (MCP server config)
    #[command(name = "setup-openclaw")]
    SetupOpenclaw {
        /// Remove myceliums from OpenClaw
        #[arg(long)]
        uninstall: bool,
    },
    /// Export graph data (symbols, relationships, communities)
    Export {
        /// Repository ID or path
        repo: String,
        /// Output file path (writes to stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Output format: json, graphml, or svg (default: json)
        #[arg(short, long, default_value = "json")]
        format: String,
        /// SVG canvas width in pixels
        #[arg(long, default_value = "1200")]
        width: u32,
        /// SVG canvas height in pixels
        #[arg(long, default_value = "800")]
        height: u32,
    },
    /// Auto-detect and set up all installed editors, or configure a specific editor
    Setup {
        /// Configure a specific editor (claude, windsurf, zed, continue, vscode, jetbrains)
        /// If omitted, auto-detects all installed editors
        #[arg(long)]
        editor: Option<String>,
        /// Remove myceliums from all (or specified) editors
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Windsurf integration (MCP server config)
    #[command(name = "setup-windsurf")]
    SetupWindsurf {
        /// Remove myceliums from Windsurf
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Zed integration (MCP server config)
    #[command(name = "setup-zed")]
    SetupZed {
        /// Remove myceliums from Zed
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Continue integration (MCP server config)
    #[command(name = "setup-continue")]
    SetupContinue {
        /// Remove myceliums from Continue
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up VS Code integration (MCP server config)
    #[command(name = "setup-vscode")]
    SetupVscode {
        /// Remove myceliums from VS Code
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up JetBrains IDE integration (MCP server config)
    #[command(name = "setup-jetbrains")]
    SetupJetbrains {
        /// Remove myceliums from JetBrains
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Gemini CLI integration (MCP server config)
    #[command(name = "setup-gemini")]
    SetupGemini {
        /// Remove myceliums from Gemini CLI
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up OpenAI Codex CLI integration (MCP server config, plus the
    /// routing block in the project's AGENTS.md when run inside a git repo)
    #[command(name = "setup-codex")]
    SetupCodex {
        /// Remove myceliums from Codex (and the AGENTS.md routing block)
        #[arg(long)]
        uninstall: bool,
        /// Do not write the routing block into AGENTS.md
        #[arg(long)]
        no_routing: bool,
    },
    /// Set up GitHub Copilot CLI integration (MCP server config)
    #[command(name = "setup-copilot")]
    SetupCopilot {
        /// Remove myceliums from Copilot CLI
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Aider integration (MCP server config)
    #[command(name = "setup-aider")]
    SetupAider {
        /// Remove myceliums from Aider
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Kiro IDE integration (MCP server config)
    #[command(name = "setup-kiro")]
    SetupKiro {
        /// Remove myceliums from Kiro
        #[arg(long)]
        uninstall: bool,
    },
    /// Set up Cursor integration (MCP server)
    #[command(name = "setup-cursor")]
    SetupCursor {
        /// Remove myceliums from Cursor
        #[arg(long)]
        uninstall: bool,
    },
    /// Generate a GRAPH_REPORT.md with god nodes, surprising connections, and community summary
    Report {
        /// Repository ID or path
        repo: String,
        /// Output file path (default: GRAPH_REPORT.md in the current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Query across all knowledge — find which emails, docs, and code mention a symbol
    Knowledge {
        /// Query term (symbol name or keyword)
        query: String,
        /// Repository ID or path
        #[arg(short, long)]
        repo: Option<String>,
        /// Maximum results to return
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Start the interactive graph visualization server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "8888")]
        port: u16,
        /// Repository ID or path (auto-detects from current directory if omitted)
        #[arg(short, long)]
        repo: Option<String>,
    },
    /// Find the shortest path between two symbols in the knowledge graph
    Path {
        /// Start symbol name or qualified name
        from: String,
        /// End symbol name or qualified name
        to: String,
        /// Repository ID or path (uses most recent if omitted)
        #[arg(short, long)]
        repo: Option<String>,
        /// Maximum BFS depth (default: 10)
        #[arg(short, long, default_value = "10")]
        max_depth: u32,
    },
    /// Manage git hooks for automatic knowledge-graph rebuilding
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Compare the current graph with the previous analysis to show drift
    Diff {
        /// Repository ID or path
        repo: String,
    },
    /// Manage email connections and sync
    #[command(subcommand)]
    Email(EmailCommands),
    /// Export the knowledge graph as an Obsidian-compatible wiki
    Wiki {
        /// Repository ID or path
        repo: String,
        /// Output directory for the wiki files
        #[arg(short, long)]
        output: PathBuf,
        /// Generate Obsidian vault structure (with .obsidian config)
        #[arg(long, value_name = "FORMAT")]
        format: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum HookAction {
    /// Install post-commit and post-checkout hooks in the current git repo
    Install,
    /// Remove myceliums git hooks from the current git repo
    Uninstall,
}

#[derive(Subcommand)]
pub(crate) enum EmailCommands {
    /// Configure an IMAP email connection
    Connect {
        /// IMAP server hostname (e.g. imap.gmail.com)
        #[arg(long)]
        host: String,
        /// Login username (usually the full email address)
        #[arg(long)]
        user: String,
        /// IMAP server port
        #[arg(long, default_value = "993")]
        port: u16,
    },
    /// Sync new emails from configured IMAP connections
    Sync {
        /// Specific account to sync (syncs all if omitted)
        #[arg(long)]
        account: Option<String>,
    },
    /// Remove an IMAP connection
    Disconnect {
        /// Account ID to remove
        account: String,
    },
}

/// Log output format of `myc mcp` (`--log-format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum LogFormat {
    Text,
    Json,
}

/// How `myc search` and `myc semantic-search` rank test code (`--tests`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum TestsArg {
    /// Rank test code below product code (default)
    Demote,
    /// Rank test code like product code
    Include,
    /// Rank test code above product code
    Prefer,
}

impl From<TestsArg> for myceliums_core::TestPolicy {
    fn from(tests: TestsArg) -> Self {
        myceliums_core::TestPolicy::new(match tests {
            TestsArg::Demote => myceliums_core::TestRanking::Demote,
            TestsArg::Include => myceliums_core::TestRanking::Include,
            TestsArg::Prefer => myceliums_core::TestRanking::Prefer,
        })
    }
}
