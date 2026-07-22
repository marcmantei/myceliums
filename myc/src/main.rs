use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use myceliums_core::analyzer::{self, Analyzer};
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_core::config::{self, ProjectConfig};
use myceliums_core::{
    attach_graph_edges, check_model_cache, compute_god_nodes, compute_surprising_connections,
    compute_uid_to_community_label, detect_impact, embedder_for_index, embedding_cache_info,
    export_graphml, export_neo4j_cypher, export_wiki, hybrid_search, hybrid_search_explain,
    local_model_code, rerank_results, run_git_diff, search_symbols, search_symbols_explain,
    CommunityDetector, Embedder, GlobalConfig, IndexEmbeddingMeta, ProcessFilter, ProcessTracer,
    RenamePlan, WikiExportConfig,
};
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

mod setup;

/// Progress bar reporter using `indicatif`. Draws on stderr so stdout
/// stays clean for JSON hook output.
struct IndicatifReporter {
    bar: std::sync::Mutex<Option<indicatif::ProgressBar>>,
}

impl IndicatifReporter {
    fn new() -> Self {
        Self {
            bar: std::sync::Mutex::new(None),
        }
    }

    fn set_bar(&self, bar: indicatif::ProgressBar) {
        let mut guard = self.bar.lock().unwrap();
        if let Some(old) = guard.take() {
            old.finish_and_clear();
        }
        *guard = Some(bar);
    }

    fn finish(&self) {
        let mut guard = self.bar.lock().unwrap();
        if let Some(bar) = guard.take() {
            bar.finish_and_clear();
        }
    }
}

impl myceliums_core::progress::ProgressReporter for IndicatifReporter {
    fn report(&self, phase: myceliums_core::progress::AnalysisPhase) {
        use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
        use myceliums_core::progress::AnalysisPhase;

        match phase {
            AnalysisPhase::Discovering => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Discovering files...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::Parsing { current, total } => {
                let mut guard = self.bar.lock().unwrap();
                if let Some(ref bar) = *guard {
                    if bar.length() == Some(total as u64) {
                        // Update existing bar
                        bar.set_position(current as u64);
                        return;
                    }
                }
                // Create new bar for parsing phase
                if let Some(old) = guard.take() {
                    old.finish_and_clear();
                }
                let bar = ProgressBar::new(total as u64)
                    .with_style(
                        ProgressStyle::with_template(
                            "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} files ({eta}) {msg}",
                        )
                        .unwrap()
                        .progress_chars("=>-")
                        .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Parsing...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                bar.set_position(current as u64);
                *guard = Some(bar);
            }
            AnalysisPhase::BuildingRelationships => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Building relationships...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::DetectingCommunities => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Detecting communities...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::TracingProcesses => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Tracing processes...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::GeneratingEmbeddings { current, total } => {
                let mut guard = self.bar.lock().unwrap();
                if let Some(ref bar) = *guard {
                    if bar.length() == Some(total as u64) {
                        bar.set_position(current as u64);
                        return;
                    }
                }
                if let Some(old) = guard.take() {
                    old.finish_and_clear();
                }
                let bar = ProgressBar::new(total as u64)
                    .with_style(
                        ProgressStyle::with_template(
                            "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} embeddings ({eta}) {msg}",
                        )
                        .unwrap()
                        .progress_chars("=>-")
                        .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Generating embeddings...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                bar.set_position(current as u64);
                *guard = Some(bar);
            }
            AnalysisPhase::Complete { symbols, files } => {
                self.finish();
                eprintln!(
                    "\u{2714} Analysis complete: {} symbols, {} files",
                    symbols, files
                );
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "myc", version, about = "Myceliums — Code Knowledge Graph")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
    /// Detect impact of current changes via git diff
    Impact {
        /// Repository ID or path
        #[arg(short, long)]
        repo: Option<String>,
        /// Graph traversal depth (default: 2)
        #[arg(short, long, default_value = "2")]
        depth: u32,
        /// Diff string or path to a .diff/.patch file (if omitted, runs git diff HEAD)
        #[arg(long)]
        diff: Option<String>,
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
    },
    /// Interactive session setup: checks cache, prompts for analysis if needed
    Session {
        /// Path to the project directory (defaults to current directory)
        path: Option<PathBuf>,
        /// Skip interactive prompt and auto-analyze if needed
        #[arg(long)]
        yes: bool,
        /// Maximum runtime in seconds for auto mode (default: 300, 0 = no limit)
        #[arg(long, default_value = "300")]
        timeout: u64,
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
        /// Run as HTTP server instead of stdio (e.g., "0.0.0.0:9999" or "127.0.0.1:3000")
        #[arg(long)]
        http: Option<String>,
    },
    /// Check the health of the Myceliums installation
    Doctor {
        /// Pre-download the fastembed model (~100 MB)
        #[arg(long)]
        download: bool,
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
    /// Set up Claude Code integration (MCP server + hooks) — run once after install
    #[command(name = "setup-claude")]
    SetupClaude {
        /// Remove the myceliums integration from Claude Code
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
    /// Set up OpenAI Codex CLI integration (MCP server config)
    #[command(name = "setup-codex")]
    SetupCodex {
        /// Remove myceliums from Codex CLI
        #[arg(long)]
        uninstall: bool,
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
    /// Compare current graph against a stored snapshot to show drift
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
enum HookAction {
    /// Install post-commit and post-checkout hooks in the current git repo
    Install,
    /// Remove myceliums git hooks from the current git repo
    Uninstall,
}

#[derive(Subcommand)]
enum EmailCommands {
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

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}

fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // First-run detection: offer setup wizard on first interactive use
    if !data_dir().exists() && is_interactive_command(&cli.command) {
        let is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if is_terminal {
            eprintln!();
            eprintln!(
                "  {} Welcome to Myceliums!",
                console::style("✦").cyan().bold()
            );
            eprintln!();
            if prompt_yes_no("  Run the setup wizard to configure your editors?").unwrap_or(false) {
                let _ = setup::wizard::run_wizard(&data_dir()).await;
            } else {
                // Create data dir so we don't ask again
                let _ = std::fs::create_dir_all(data_dir());
            }
        }
    }

    match cli.command {
        Commands::Analyze {
            path,
            force,
            max_age,
            skip_embeddings,
            watch,
            no_git_check,
        } => {
            check_session_safeguards(&path, no_git_check)?;
            cmd_analyze(&path, force, max_age, skip_embeddings).await?;
            if watch {
                cmd_watch(&path, skip_embeddings).await?;
            }
            Ok(())
        }
        Commands::Init { default: _ } => cmd_init().await,
        Commands::List { json } => cmd_list(json).await,
        Commands::Delete { repo } => cmd_delete(&repo).await,
        Commands::Stats { repo } => cmd_stats(&repo).await,
        Commands::Search {
            query,
            repo,
            limit,
            hybrid,
            rerank,
            explain,
        } => cmd_search(&query, repo.as_deref(), limit, hybrid, rerank, explain).await,
        Commands::Communities { repo } => cmd_communities(&repo).await,
        Commands::Processes {
            repo,
            entry,
            filter,
            limit,
            min_steps,
        } => cmd_processes(&repo, entry, filter, limit, min_steps).await,
        Commands::Query { query, repo } => cmd_query(&query, repo.as_deref()).await,
        Commands::Rename {
            symbol_name,
            new_name,
            repo,
            apply,
        } => cmd_rename(&symbol_name, &new_name, repo.as_deref(), apply).await,
        Commands::Impact { repo, depth, diff } => {
            cmd_impact(repo.as_deref(), depth, diff.as_deref()).await
        }
        Commands::SemanticSearch { query, repo, limit } => {
            cmd_semantic_search(&query, repo.as_deref(), limit).await
        }
        Commands::Session {
            path,
            yes,
            timeout,
            no_git_check,
        } => {
            let p = path.unwrap_or_else(|| PathBuf::from("."));
            cmd_session(&p, yes, timeout, no_git_check).await
        }
        Commands::Status => cmd_status().await,
        Commands::Clean {
            repo,
            orphans,
            all,
            cache,
            yes,
        } => cmd_clean(repo, orphans, all, cache, yes).await,
        Commands::Mcp { http } => {
            if let Some(addr) = http {
                eprintln!("MCP server listening on http://{}/mcp", addr);
                myceliums_mcp::run_mcp_http_server(&addr).await
            } else {
                myceliums_mcp::run_mcp_server().await
            }
        }
        Commands::Doctor { download } => cmd_doctor(download).await,
        Commands::Configure { set, reset } => cmd_configure(set.as_deref(), reset).await,
        Commands::FormatHook => cmd_format_hook().await,
        Commands::SetupClaude { uninstall } => cmd_setup_claude(uninstall).await,
        Commands::SetupSpacebot { uninstall } => {
            cmd_setup_mcp_platform("Spacebot", ".spacebot/mcp.json", uninstall)
        }
        Commands::SetupOpenclaw { uninstall } => {
            cmd_setup_mcp_platform("OpenClaw", ".openclaw/mcp.json", uninstall)
        }
        Commands::Export {
            repo,
            output,
            format,
            width,
            height,
        } => cmd_export(&repo, output.as_deref(), &format, width, height).await,
        Commands::Setup { editor, uninstall } => cmd_setup(editor, uninstall).await,
        Commands::SetupWindsurf { uninstall } => cmd_setup_editor("windsurf", uninstall).await,
        Commands::SetupZed { uninstall } => cmd_setup_editor("zed", uninstall).await,
        Commands::SetupContinue { uninstall } => cmd_setup_editor("continue", uninstall).await,
        Commands::SetupVscode { uninstall } => cmd_setup_editor("vscode", uninstall).await,
        Commands::SetupJetbrains { uninstall } => cmd_setup_editor("jetbrains", uninstall).await,
        Commands::SetupGemini { uninstall } => cmd_setup_editor("gemini", uninstall).await,
        Commands::SetupCodex { uninstall } => cmd_setup_editor("codex", uninstall).await,
        Commands::SetupCopilot { uninstall } => cmd_setup_editor("copilot", uninstall).await,
        Commands::SetupAider { uninstall } => cmd_setup_editor("aider", uninstall).await,
        Commands::SetupKiro { uninstall } => cmd_setup_editor("kiro", uninstall).await,
        Commands::SetupCursor { uninstall } => cmd_setup_editor("cursor", uninstall).await,
        Commands::Report { repo, output } => cmd_report(&repo, output.as_deref()).await,
        Commands::Knowledge { query, repo, limit } => {
            cmd_knowledge(&query, repo.as_deref(), limit).await
        }
        Commands::Serve { port, repo } => cmd_serve(port, repo).await,
        Commands::Path {
            from,
            to,
            repo,
            max_depth,
        } => cmd_path(&from, &to, repo.as_deref(), max_depth).await,
        Commands::Hook { action } => match action {
            HookAction::Install => cmd_hook_install(),
            HookAction::Uninstall => cmd_hook_uninstall(),
        },
        Commands::Email(sub) => match sub {
            EmailCommands::Connect { host, user, port } => {
                cmd_email_connect(&host, &user, port).await
            }
            EmailCommands::Sync { account } => cmd_email_sync(account.as_deref()).await,
            EmailCommands::Disconnect { account } => cmd_email_disconnect(&account).await,
        },
        Commands::Diff { repo } => cmd_diff(&repo).await,
        Commands::Wiki {
            repo,
            output,
            format,
        } => cmd_wiki(&repo, &output, format.as_deref()).await,
    }
}

fn resolve_repo(repo: &str) -> Result<(String, RepoInfo)> {
    let registry = RepoRegistry::load(&registry_path())?;
    if let Some(info) = registry.get(repo) {
        return Ok((repo.to_string(), info.clone()));
    }
    if let Some(info) = registry.find_by_path(repo) {
        return Ok((info.id.clone(), info.clone()));
    }
    anyhow::bail!("Repository not found: {}", repo);
}

fn resolve_repo_or_latest(repo: Option<&str>) -> Result<(String, RepoInfo)> {
    if let Some(r) = repo {
        return resolve_repo(r);
    }

    let registry = RepoRegistry::load(&registry_path())?;

    // Try to find a repo that contains the current working directory
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(cwd_canonical) = cwd.canonicalize() {
            for repo_info in registry.list() {
                let repo_path = PathBuf::from(&repo_info.path);
                if let Ok(repo_canonical) = repo_path.canonicalize() {
                    // Check if cwd is within repo_path or is repo_path itself
                    if cwd_canonical.starts_with(&repo_canonical) {
                        eprintln!("Resolved repository: {} ({})", repo_info.name, repo_info.id);
                        return Ok((repo_info.id.clone(), repo_info.clone()));
                    }
                }
            }
        }
    }

    // Fall back to the latest repo if cwd is not in any indexed repo
    let repos = registry.list();
    if let Some(latest) = repos.last() {
        eprintln!(
            "Using latest repository as fallback: {} ({})",
            latest.name, latest.id
        );
        return Ok((latest.id.clone(), (*latest).clone()));
    }
    anyhow::bail!("No repositories analyzed yet. Run: myc analyze <path>");
}

async fn cmd_init() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join(config::CONFIG_FILENAME);

    if config_path.exists() {
        println!("{} already exists.", config::CONFIG_FILENAME);
        return Ok(());
    }

    let dir_name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut cfg = ProjectConfig::default();
    cfg.project.name = dir_name;

    cfg.save(&config_path)?;
    println!("Created {} with default settings.", config::CONFIG_FILENAME);
    println!("Edit it to customize analysis for this project.");
    Ok(())
}

/// Check if a command is interactive (should trigger first-run prompt).
/// Non-interactive commands (MCP server, hooks, auto-session) should never prompt.
fn is_interactive_command(cmd: &Commands) -> bool {
    !matches!(
        cmd,
        Commands::Mcp { .. } | Commands::FormatHook | Commands::Session { yes: true, .. }
    )
}

// Path-safety guards: refuse to analyze the home directory (would index
// hundreds of thousands of files) and non-git directories (unless
// `--no-git-check` is passed).

/// Check if a path is a dangerous ancestor (/, /Users, /home, drive roots).
fn is_dangerous_ancestor(path: &Path) -> bool {
    // Root directories that should never be indexed
    let dangerous = ["/", "/Users", "/home"];
    for ancestor in &dangerous {
        if path.as_os_str() == *ancestor {
            return true;
        }
    }

    // On Windows, check for drive roots (C:\, D:\, etc.)
    #[cfg(windows)]
    {
        if let Some(s) = path.to_str() {
            // Drive root pattern: "C:\", "D:\", etc.
            if s.len() == 3 && s.chars().nth(1) == Some(':') && s.ends_with('\\') {
                return true;
            }
        }
    }

    false
}

/// Check if a target path is a problematic parent of home directory.
/// This catches attempts to analyze /Users, /home, or root on systems
/// where home is a subdirectory.
fn is_home_ancestor(target: &Path, home: &Path) -> bool {
    // Check if target is an ancestor of home (e.g., /Users when home is /Users/marc)
    match home.strip_prefix(target) {
        Ok(_) => target != home, // target is an ancestor of home (but not home itself)
        Err(_) => false,
    }
}

fn check_session_safeguards(path: &Path, no_git_check: bool) -> Result<()> {
    // Fail closed: if canonicalize fails, refuse the operation rather than proceeding
    let abs_path = std::fs::canonicalize(path).with_context(|| {
        format!(
            "Could not canonicalize path: {}. This may indicate a symlink loop or permission issue. Use --no-git-check if you are certain this is safe.",
            path.display()
        )
    })?;

    // Home directory guard — enhanced
    if let Some(home) = dirs::home_dir() {
        if let Ok(canonical_home) = std::fs::canonicalize(&home) {
            // Refuse exact home directory
            if abs_path == canonical_home {
                anyhow::bail!(
                    "Refusing to analyze home directory (~). \
                     This would index hundreds of thousands of files and consume \
                     excessive CPU and memory. Specify a project directory instead:\n\n  \
                     myc analyze ./my-project"
                );
            }

            // Refuse ancestors of home (/, /Users, /home, drive roots)
            if is_home_ancestor(&abs_path, &canonical_home) {
                anyhow::bail!(
                    "Refusing to analyze parent directory of home (~). \
                     Path: {} would index your entire home directory and more. \
                     Specify a project directory instead:\n\n  \
                     myc analyze ./my-project",
                    abs_path.display()
                );
            }

            // Refuse known dangerous ancestors
            if is_dangerous_ancestor(&abs_path) {
                anyhow::bail!(
                    "Refusing to analyze system root or shared user directory: {}. \
                     This would consume excessive resources. \
                     Specify a project directory instead:\n\n  \
                     myc analyze ./my-project",
                    abs_path.display()
                );
            }
        }
    }

    // Git repository guard
    if !no_git_check && !abs_path.join(".git").exists() {
        anyhow::bail!(
            "No .git directory found at {}. \
             Myceliums is designed for git repositories. \
             Use --no-git-check to override.",
            abs_path.display()
        );
    }

    Ok(())
}

async fn cmd_analyze(
    path: &Path,
    force: bool,
    max_age_minutes: u64,
    skip_embeddings: bool,
) -> Result<()> {
    use myceliums_core::lock::{AnalysisLock, LockOutcome};

    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let repo_id = analyzer::repo_id_from_path(&abs_path);
    let repo_name = analyzer::repo_name_from_path(&abs_path);
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

    // Cache check: skip re-analysis if cache is fresh (unless --force)
    if !force {
        let registry = RepoRegistry::load(&registry_path())?;
        if let Some(repo_info) = registry.get(&repo_id) {
            let cache_config = CacheCheckConfig {
                max_age_minutes,
                ..Default::default()
            };
            match cache::check_cache(repo_info, &abs_path, &cache_config) {
                CacheDecision::UseCached { repo_id, reason } => {
                    println!("Using cached analysis for {} ({})", repo_name, repo_id);
                    println!("  Reason: {}", reason);
                    println!("  Analyzed at: {}", repo_info.analyzed_at);
                    println!(
                        "  Symbols: {}, Files: {}",
                        repo_info.symbol_count, repo_info.file_count
                    );
                    println!();
                    println!("Use --force to re-analyze.");
                    return Ok(());
                }
                CacheDecision::ReanalyzeNeeded { reason } => {
                    println!("Re-analysis needed: {}", reason);
                }
            }
        }
    }

    std::fs::create_dir_all(&db_path)?;

    // Acquire analysis lock — prevents concurrent indexing of the same repo
    let _lock = match AnalysisLock::acquire(&data_dir(), &repo_id)? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::AlreadyRunning { pid } => {
            println!(
                "Analysis already in progress for {} (PID {}). Skipping.",
                repo_name, pid
            );
            return Ok(());
        }
    };

    // Check for project-level config
    let config_path = abs_path.join(config::CONFIG_FILENAME);
    let project_config = if config_path.exists() {
        let cfg = ProjectConfig::load(&config_path)?;
        println!("Using config from {}", config_path.display());
        Some(cfg)
    } else {
        None
    };

    // Set up progress display (on stderr so stdout stays clean for hooks)
    let is_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let progress_reporter: std::sync::Arc<dyn myceliums_core::progress::ProgressReporter> =
        if is_terminal {
            std::sync::Arc::new(IndicatifReporter::new())
        } else {
            std::sync::Arc::new(myceliums_core::SilentReporter)
        };

    if is_terminal {
        // Don't print "Analyzing..." when progress bar is active
    } else {
        println!("Analyzing {} ...", abs_path.display());
    }

    let store = Store::open(&db_path, &repo_id).await?;
    store.delete_repo_data().await?;

    let analyzer = if let Some(cfg) = project_config {
        Analyzer::with_config(store, abs_path.clone(), cfg)
    } else {
        Analyzer::new(store, abs_path.clone())
    }
    .set_skip_embeddings(skip_embeddings)
    .with_progress(progress_reporter);
    let result = analyzer.analyze().await?;

    println!("  Symbols:       {}", result.symbol_count);
    println!("  Files:         {}", result.file_count);
    println!("  Relationships: {}", result.relationship_count);
    println!("  Embeddings:    {}", result.embedding_count);
    if result.mentions_count > 0 {
        println!("  Mentions:      {}", result.mentions_count);
    }

    // Display timing breakdown if available
    if let Some(timing) = &result.timing {
        println!();
        println!("{}", timing.format_report());
    }

    // Phase 2: Community detection and process tracing
    let store = Store::open(&db_path, &repo_id).await?;
    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    let communities = CommunityDetector::detect(&symbols, &relationships, &repo_id)?;
    let community_count = store.store_communities(&communities).await?;

    let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)?;
    let process_count = store.store_processes(&processes).await?;

    println!("  Communities:   {}", community_count);
    println!("  Processes:     {}", process_count);

    // Save a lightweight snapshot for future diff comparisons
    let snapshot = myceliums_core::build_snapshot(&repo_id, &symbols, &relationships);
    if let Err(e) = myceliums_core::save_snapshot(&data_dir(), &snapshot) {
        eprintln!("Warning: failed to save snapshot: {}", e);
    }

    // Get current git commit for cache tracking
    let analyzed_commit = cache::get_head_commit(&abs_path).ok();

    // Update registry
    let mut registry = RepoRegistry::load(&registry_path())?;
    registry.register(RepoInfo {
        id: repo_id.clone(),
        name: repo_name,
        path: abs_path.to_string_lossy().to_string(),
        analyzed_at: chrono::Utc::now().to_rfc3339(),
        symbol_count: result.symbol_count as u32,
        file_count: result.file_count as u32,
        analyzed_commit,
    });
    registry.save()?;

    println!();
    println!("Repository ID: {}", repo_id);
    Ok(())
}

async fn cmd_watch(path: &Path, skip_embeddings: bool) -> Result<()> {
    use myceliums_core::watch::{start_watching, WatchEvent};

    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;
    let repo_id = analyzer::repo_id_from_path(&abs_path);
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

    // Load project config if present
    let config_path = abs_path.join(config::CONFIG_FILENAME);
    let project_config = if config_path.exists() {
        ProjectConfig::load(&config_path).ok()
    } else {
        None
    };

    println!(
        "Watching {} for changes (Ctrl+C to stop)...",
        abs_path.display()
    );

    let (mut rx, _debouncer) = start_watching(&abs_path)?;

    while let Some(event) = rx.recv().await {
        match event {
            WatchEvent::FilesChanged(paths) => {
                let rel_paths: Vec<String> = paths
                    .iter()
                    .filter_map(|p| p.strip_prefix(&abs_path).ok())
                    .map(|p| p.display().to_string())
                    .collect();
                println!(
                    "  Re-indexing {} changed file(s): {}",
                    paths.len(),
                    rel_paths.join(", ")
                );

                let store = Store::open(&db_path, &repo_id).await?;
                let a = if let Some(ref cfg) = project_config {
                    Analyzer::with_config(store, abs_path.clone(), cfg.clone())
                } else {
                    Analyzer::new(store, abs_path.clone())
                }
                .set_skip_embeddings(skip_embeddings);

                match a.analyze_files(&paths).await {
                    Ok(result) => {
                        println!(
                            "    Updated: {} symbols, {} files, {} relationships",
                            result.symbol_count, result.file_count, result.relationship_count
                        );
                    }
                    Err(e) => {
                        eprintln!("    Error during incremental analysis: {}", e);
                    }
                }
            }
            WatchEvent::FilesRemoved(paths) => {
                let rel_paths: Vec<String> = paths
                    .iter()
                    .filter_map(|p| p.strip_prefix(&abs_path).ok())
                    .map(|p| p.display().to_string())
                    .collect();
                println!(
                    "  Removing {} deleted file(s): {}",
                    paths.len(),
                    rel_paths.join(", ")
                );

                let store = Store::open(&db_path, &repo_id).await?;
                let a = if let Some(ref cfg) = project_config {
                    Analyzer::with_config(store, abs_path.clone(), cfg.clone())
                } else {
                    Analyzer::new(store, abs_path.clone())
                };

                for p in &paths {
                    if let Err(e) = a.handle_file_deleted(p).await {
                        eprintln!("    Error removing file data: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn cmd_list(json: bool) -> Result<()> {
    let registry = RepoRegistry::load(&registry_path())?;
    let repos = registry.list();

    if json {
        let json_repos: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "path": r.path,
                    "analyzed_at": r.analyzed_at,
                    "symbol_count": r.symbol_count,
                    "file_count": r.file_count,
                    "analyzed_commit": r.analyzed_commit,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_repos)?);
        return Ok(());
    }

    if repos.is_empty() {
        println!("No repositories analyzed yet.");
        println!("Run: myc analyze <path>");
        return Ok(());
    }

    println!("{:<30} {:<10} {:<10} Path", "Name", "Symbols", "Files");
    println!("{}", "-".repeat(80));

    for repo in repos {
        println!(
            "{:<30} {:<10} {:<10} {}",
            repo.name, repo.symbol_count, repo.file_count, repo.path
        );
    }

    Ok(())
}

async fn cmd_delete(repo: &str) -> Result<()> {
    let mut registry = RepoRegistry::load(&registry_path())?;

    let repo_id = if registry.get(repo).is_some() {
        repo.to_string()
    } else if let Some(info) = registry.find_by_path(repo) {
        info.id.clone()
    } else {
        anyhow::bail!("Repository not found: {}", repo);
    };

    let info = registry.remove(&repo_id).unwrap();
    registry.save()?;

    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path)?;
    }

    println!("Deleted: {} ({})", info.name, repo_id);
    Ok(())
}

/// Compute the total size of a directory tree in bytes.
fn dir_size_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Format bytes into a human-readable string (KB, MB, GB).
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Find orphaned data directories (exist on disk but not in the registry).
fn find_orphans() -> Result<Vec<(String, PathBuf, u64)>> {
    let data_path = data_dir().join("data");
    if !data_path.exists() {
        return Ok(vec![]);
    }
    let registry = RepoRegistry::load(&registry_path()).unwrap_or_default();
    let mut orphans = Vec::new();
    for entry in std::fs::read_dir(&data_path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if registry.get(&dir_name).is_none() {
                let size = dir_size_bytes(&entry.path());
                orphans.push((dir_name, entry.path(), size));
            }
        }
    }
    Ok(orphans)
}

async fn cmd_status() -> Result<()> {
    let data = data_dir();

    println!("Myceliums Status");
    println!("{}", "=".repeat(60));
    println!();

    // Repositories
    let registry = RepoRegistry::load(&registry_path()).unwrap_or_default();
    let repos = registry.list();

    if repos.is_empty() {
        println!("  No repositories analyzed yet.");
    } else {
        println!(
            "  {:<24} {:>6} {:>8} {:>10}  Last Analyzed",
            "Name", "Files", "Symbols", "Size"
        );
        println!("  {}", "\u{2500}".repeat(74));

        let mut total_size = 0u64;
        for repo in &repos {
            let db_path = RepoRegistry::repo_db_path(&data, &repo.id);
            let size = if db_path.exists() {
                dir_size_bytes(&db_path)
            } else {
                0
            };
            total_size += size;

            let analyzed = repo
                .analyzed_at
                .split('T')
                .next()
                .unwrap_or(&repo.analyzed_at);
            println!(
                "  {:<24} {:>6} {:>8} {:>10}  {}",
                truncate_str(&repo.name, 24),
                repo.file_count,
                repo.symbol_count,
                format_bytes(size),
                analyzed,
            );
        }

        println!();
        println!(
            "  Total: {} repositories, {}",
            repos.len(),
            format_bytes(total_size)
        );
    }

    // Overall data directory
    println!();
    if data.exists() {
        let total_data_size = dir_size_bytes(&data);
        println!("  Data directory: {}", data.display());
        println!("  Total size:     {}", format_bytes(total_data_size));
    } else {
        println!("  Data directory: not created yet");
    }

    // Orphans
    let orphans = find_orphans()?;
    if !orphans.is_empty() {
        let orphan_total: u64 = orphans.iter().map(|(_, _, s)| s).sum();
        println!(
            "  Orphaned dirs:  {} ({})",
            orphans.len(),
            format_bytes(orphan_total)
        );
        for (name, _, size) in &orphans {
            println!("    - {} ({})", name, format_bytes(*size));
        }
    }

    // FastEmbed model cache
    {
        println!();
        let cache_info = embedding_cache_info();
        println!("  FastEmbed model cache:");
        println!("    Location: {}", cache_info.cache_dir.display());
        println!("    Size:     {}", format_bytes(cache_info.size_bytes));
        if cache_info.is_cached {
            println!("    Status:   cached");
        } else {
            println!("    Status:   not downloaded");
        }
    }

    println!();
    Ok(())
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

async fn cmd_clean(
    repo: Option<String>,
    orphans: bool,
    all: bool,
    cache: bool,
    auto_yes: bool,
) -> Result<()> {
    let data = data_dir();

    if all {
        if data.exists() {
            let total = dir_size_bytes(&data);
            println!("This will delete ALL myceliums data at {}", data.display());
            println!("Will free: {}", format_bytes(total));
            println!();
            if auto_yes || prompt_yes_no("Delete all myceliums data?")? {
                std::fs::remove_dir_all(&data)?;
                println!("Deleted all myceliums data.");
            } else {
                println!("Cancelled.");
            }
        } else {
            println!("No myceliums data directory found.");
        }
        return Ok(());
    }

    if orphans {
        let found = find_orphans()?;
        if found.is_empty() {
            println!("No orphaned data directories found.");
            return Ok(());
        }
        let orphan_total: u64 = found.iter().map(|(_, _, s)| s).sum();
        println!("Found {} orphaned data directories:", found.len());
        for (name, _, size) in &found {
            println!("  - {} ({})", name, format_bytes(*size));
        }
        println!();
        println!("Will free: {}", format_bytes(orphan_total));
        println!();
        if auto_yes || prompt_yes_no("Delete orphaned directories?")? {
            for (name, path, _) in &found {
                std::fs::remove_dir_all(path)?;
                println!("  Deleted: {}", name);
            }
            println!("Done.");
        } else {
            println!("Cancelled.");
        }
        return Ok(());
    }

    if cache {
        let cache_info = embedding_cache_info();
        if cache_info.is_cached {
            println!(
                "FastEmbed model cache: {} ({})",
                cache_info.cache_dir.display(),
                format_bytes(cache_info.size_bytes)
            );
            if auto_yes || prompt_yes_no("Delete model cache?")? {
                std::fs::remove_dir_all(&cache_info.cache_dir)?;
                println!("Deleted model cache.");
            } else {
                println!("Cancelled.");
            }
        } else {
            println!("No model cache found.");
        }
        return Ok(());
    }

    if let Some(repo_ref) = repo {
        // Clean a specific repo (with confirmation, unlike bare delete)
        let registry = RepoRegistry::load(&registry_path())?;
        let repo_id = if registry.get(&repo_ref).is_some() {
            repo_ref.clone()
        } else if let Some(info) = registry.find_by_path(&repo_ref) {
            info.id.clone()
        } else {
            anyhow::bail!("Repository not found: {}", repo_ref);
        };

        let db_path = RepoRegistry::repo_db_path(&data, &repo_id);
        let size = if db_path.exists() {
            dir_size_bytes(&db_path)
        } else {
            0
        };
        println!("Repository: {} ({})", repo_ref, format_bytes(size));
        if auto_yes || prompt_yes_no("Delete this repository's data?")? {
            cmd_delete(&repo_ref).await?;
        } else {
            println!("Cancelled.");
        }
        return Ok(());
    }

    println!("Usage:");
    println!("  myc clean <repo>     — delete a specific repository's data");
    println!("  myc clean --orphans  — remove orphaned data directories");
    println!("  myc clean --all      — remove ALL myceliums data");
    println!("  myc clean --cache    — remove fastembed model cache");
    println!();
    println!("Add --yes to skip confirmation prompts.");
    Ok(())
}

async fn cmd_stats(repo: &str) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let files = store.get_files().await?;
    let rels = store.get_relationships().await?;
    let communities = store.get_communities().await?;
    let processes = store.get_processes().await?;

    println!("Repository: {}", repo_info.name);
    println!("Path:       {}", repo_info.path);
    println!("Analyzed:   {}", repo_info.analyzed_at);
    println!();
    println!("  Symbols:       {}", symbols.len());
    println!("  Files:         {}", files.len());
    println!("  Relationships: {}", rels.len());
    println!("  Communities:   {}", communities.len());
    println!("  Processes:     {}", processes.len());

    // Symbol breakdown
    let mut kind_counts = std::collections::HashMap::new();
    for sym in &symbols {
        *kind_counts.entry(sym.kind.to_string()).or_insert(0u32) += 1;
    }
    println!();
    println!("  Symbol breakdown:");
    let mut kinds: Vec<_> = kind_counts.into_iter().collect();
    kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (kind, count) in kinds {
        println!("    {:<15} {}", kind, count);
    }

    // Relationship breakdown
    let mut rel_counts = std::collections::HashMap::new();
    for rel in &rels {
        *rel_counts.entry(rel.kind.to_string()).or_insert(0u32) += 1;
    }
    if !rel_counts.is_empty() {
        println!();
        println!("  Relationship breakdown:");
        let mut rel_kinds: Vec<_> = rel_counts.into_iter().collect();
        rel_kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (kind, count) in rel_kinds {
            println!("    {:<15} {}", kind, count);
        }
    }

    // Language breakdown
    let mut lang_counts = std::collections::HashMap::new();
    for file in &files {
        *lang_counts.entry(file.language.clone()).or_insert(0u32) += 1;
    }
    println!();
    println!("  Languages:");
    let mut langs: Vec<_> = lang_counts.into_iter().collect();
    langs.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (lang, count) in langs {
        println!("    {:<15} {} files", lang, count);
    }

    Ok(())
}

async fn cmd_export(
    repo: &str,
    output: Option<&Path>,
    format: &str,
    width: u32,
    height: u32,
) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let rels = store.get_relationships().await?;
    let communities = store.get_communities().await?;

    match format {
        "json" => export_json(&repo_info, &symbols, &rels, &communities, output),
        "graphml" => cli_export_graphml(&symbols, &rels, output),
        "cypher" | "neo4j" => cli_export_cypher(&symbols, &rels, output),
        "svg" => export_svg(&repo_info, &symbols, &rels, width, height, output),
        _ => anyhow::bail!(
            "Unknown format '{}'. Supported: json, graphml, cypher/neo4j, svg",
            format
        ),
    }
}

fn export_json(
    repo_info: &RepoInfo,
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    communities: &[myceliums_storage::models::Community],
    output: Option<&Path>,
) -> Result<()> {
    let nodes: Vec<serde_json::Value> = symbols
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.uid,
                "name": s.name,
                "qualified_name": s.qualified_name,
                "kind": s.kind.to_string(),
                "file_path": s.file_path,
                "start_line": s.start_line,
                "end_line": s.end_line,
            })
        })
        .collect();

    let edges: Vec<serde_json::Value> = rels
        .iter()
        .map(|r| {
            serde_json::json!({
                "source": r.source_uid,
                "target": r.target_uid,
                "kind": r.kind.to_string(),
            })
        })
        .collect();

    let community_list: Vec<serde_json::Value> = communities
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.uid,
                "label": c.label,
                "member_count": c.member_count,
                "top_symbols": c.top_symbols,
                "summary": c.summary,
            })
        })
        .collect();

    let export = serde_json::json!({
        "metadata": {
            "repository": repo_info.name,
            "path": repo_info.path,
            "analyzed_at": repo_info.analyzed_at,
            "symbols": nodes.len(),
            "relationships": edges.len(),
            "communities": community_list.len(),
        },
        "nodes": nodes,
        "edges": edges,
        "communities": community_list,
    });

    let json = serde_json::to_string_pretty(&export)?;

    if let Some(path) = output {
        std::fs::write(path, &json)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges, {} communities to {}",
            nodes.len(),
            edges.len(),
            community_list.len(),
            path.display()
        );
    } else {
        println!("{}", json);
    }

    Ok(())
}

/// Escape a string for use inside XML attribute values or text content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn cli_export_graphml(
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    output: Option<&Path>,
) -> Result<()> {
    let xml = export_graphml(symbols, rels);
    if let Some(path) = output {
        std::fs::write(path, &xml)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (GraphML) to {}",
            symbols.len(),
            rels.len(),
            path.display()
        );
    } else {
        println!("{}", xml);
    }
    Ok(())
}

fn svg_truncate(s: &str, max_chars: usize) -> String {
    let mut iter = s.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

fn kind_color(kind: &myceliums_storage::models::SymbolKind) -> &'static str {
    use myceliums_storage::models::SymbolKind;
    // Matches the website's dark theme color palette
    match kind {
        SymbolKind::Function => "#6366F1",     // indigo (accent)
        SymbolKind::Method => "#8B5CF6",       // violet
        SymbolKind::Class => "#EC4899",        // pink
        SymbolKind::Interface => "#06B6D4",    // cyan
        SymbolKind::TypeAlias => "#A855F7",    // purple
        SymbolKind::Variable => "#10B981",     // emerald (success)
        SymbolKind::Constant => "#F59E0B",     // amber (warning)
        SymbolKind::Enum => "#EF4444",         // red (danger)
        SymbolKind::Module => "#14B8A6",       // teal
        SymbolKind::Import => "#64748B",       // slate
        SymbolKind::Section => "#818CF8",      // accent-glow
        SymbolKind::Document => "#34D399",     // emerald-light
        SymbolKind::Rationale => "#FBBF24",    // yellow
        SymbolKind::Email => "#EC7063",        // coral (warm red)
        SymbolKind::Conversation => "#5DADE2", // bright blue
        SymbolKind::Person => "#AF7AC5",       // orchid (light purple)
        SymbolKind::Attachment => "#F8B88B",   // peach
    }
}

fn force_directed_layout(
    n: usize,
    edges: &[(usize, usize)],
    width: u32,
    height: u32,
) -> Vec<(f64, f64)> {
    if n == 0 {
        return Vec::new();
    }
    let w = width as f64;
    let h = height as f64;
    let margin = 60.0_f64;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r_init = ((w - 2.0 * margin).min(h - 2.0 * margin)) / 2.5;
    let mut pos: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let angle = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (cx + r_init * angle.cos(), cy + r_init * angle.sin())
        })
        .collect();
    if n == 1 {
        pos[0] = (cx, cy);
        return pos;
    }
    let area = (w - 2.0 * margin) * (h - 2.0 * margin);
    let k = (area / n as f64).sqrt();
    let iterations: usize = if n <= 100 {
        300
    } else if n <= 500 {
        150
    } else {
        50
    };
    let mut temp = w / 10.0;
    let mut disp: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    for _ in 0..iterations {
        for d in disp.iter_mut() {
            *d = (0.0, 0.0);
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[j].0 - pos[i].0;
                let dy = pos[j].1 - pos[i].1;
                let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                let force = k * k / dist;
                let fx = force * dx / dist;
                let fy = force * dy / dist;
                disp[i].0 -= fx;
                disp[i].1 -= fy;
                disp[j].0 += fx;
                disp[j].1 += fy;
            }
        }
        for &(si, ti) in edges {
            let dx = pos[ti].0 - pos[si].0;
            let dy = pos[ti].1 - pos[si].1;
            let dist = (dx * dx + dy * dy).sqrt().max(0.01);
            let force = dist * dist / k;
            let fx = force * dx / dist;
            let fy = force * dy / dist;
            disp[si].0 += fx;
            disp[si].1 += fy;
            disp[ti].0 -= fx;
            disp[ti].1 -= fy;
        }
        for i in 0..n {
            let mag = (disp[i].0 * disp[i].0 + disp[i].1 * disp[i].1)
                .sqrt()
                .max(0.001);
            let scale = temp.min(mag) / mag;
            pos[i].0 += disp[i].0 * scale;
            pos[i].1 += disp[i].1 * scale;
            pos[i].0 = pos[i].0.clamp(margin, w - margin);
            pos[i].1 = pos[i].1.clamp(margin, h - margin);
        }
        temp *= 0.95;
    }
    pos
}

fn cli_export_cypher(
    symbols: &[myceliums_storage::CodeSymbol],
    rels: &[myceliums_storage::Relationship],
    output: Option<&Path>,
) -> Result<()> {
    let out = export_neo4j_cypher(symbols, rels);
    if let Some(path) = output {
        std::fs::write(path, &out)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (Cypher) to {}",
            symbols.len(),
            rels.len(),
            path.display()
        );
    } else {
        println!("{}", out);
    }
    Ok(())
}

fn export_svg(
    repo_info: &RepoInfo,
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    width: u32,
    height: u32,
    output: Option<&Path>,
) -> Result<()> {
    const MAX_SVG_NODES: usize = 500;
    use std::collections::HashSet;

    // For large graphs, sample the most connected nodes
    let (sampled_symbols, sampled_rels) = if symbols.len() > MAX_SVG_NODES {
        // Count connections per symbol
        let mut conn_count: HashMap<&str, u32> = HashMap::new();
        for r in rels {
            *conn_count.entry(r.source_uid.as_str()).or_insert(0) += 1;
            *conn_count.entry(r.target_uid.as_str()).or_insert(0) += 1;
        }

        // Sort symbols by connection count, take top MAX_SVG_NODES
        let mut ranked: Vec<(usize, u32)> = symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (i, conn_count.get(s.uid.as_str()).copied().unwrap_or(0)))
            .collect();
        ranked.sort_by_key(|b| std::cmp::Reverse(b.1));

        let keep_indices: HashSet<usize> =
            ranked.iter().take(MAX_SVG_NODES).map(|(i, _)| *i).collect();
        let keep_uids: HashSet<&str> = keep_indices
            .iter()
            .map(|&i| symbols[i].uid.as_str())
            .collect();

        let sampled_syms: Vec<myceliums_storage::models::CodeSymbol> = symbols
            .iter()
            .enumerate()
            .filter(|(i, _)| keep_indices.contains(i))
            .map(|(_, s)| s.clone())
            .collect();

        let sampled_rs: Vec<myceliums_storage::models::Relationship> = rels
            .iter()
            .filter(|r| {
                keep_uids.contains(r.source_uid.as_str())
                    && keep_uids.contains(r.target_uid.as_str())
            })
            .cloned()
            .collect();

        eprintln!(
            "  SVG: sampled {} of {} nodes (top by connections), {} edges",
            sampled_syms.len(),
            symbols.len(),
            sampled_rs.len()
        );
        (sampled_syms, sampled_rs)
    } else {
        (symbols.to_vec(), rels.to_vec())
    };

    let symbols = &sampled_symbols;
    let rels = &sampled_rels;

    let uid_to_community = compute_uid_to_community_label(symbols, rels).unwrap_or_default();
    let uid_to_idx: HashMap<&str, usize> = symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.uid.as_str(), i))
        .collect();

    let n = symbols.len();
    let edge_pairs: Vec<(usize, usize)> = rels
        .iter()
        .filter_map(|r| {
            let si = uid_to_idx.get(r.source_uid.as_str()).copied()?;
            let ti = uid_to_idx.get(r.target_uid.as_str()).copied()?;
            (si != ti).then_some((si, ti))
        })
        .collect();
    let pos = force_directed_layout(n, &edge_pairs, width, height);

    const COMMUNITY_COLORS: &[&str] = &[
        "rgba(99,102,241,0.08)",
        "rgba(16,185,129,0.08)",
        "rgba(245,158,11,0.08)",
        "rgba(236,72,153,0.08)",
        "rgba(139,92,246,0.08)",
        "rgba(6,182,212,0.08)",
        "rgba(249,115,22,0.08)",
        "rgba(100,116,139,0.08)",
    ];

    let mut community_color_counter = 0usize;
    let mut community_color_idx: HashMap<String, usize> = HashMap::new();
    let mut community_bounds: HashMap<String, (f64, f64, f64, f64, usize)> = HashMap::new();
    for (i, s) in symbols.iter().enumerate() {
        if let Some(label) = uid_to_community.get(&s.uid) {
            let (px, py) = pos[i];
            if !community_color_idx.contains_key(label) {
                let c = community_color_counter % COMMUNITY_COLORS.len();
                community_color_counter += 1;
                community_color_idx.insert(label.clone(), c);
            }
            let cidx = community_color_idx[label];
            let entry = community_bounds
                .entry(label.clone())
                .or_insert((px, py, px, py, cidx));
            entry.0 = entry.0.min(px);
            entry.1 = entry.1.min(py);
            entry.2 = entry.2.max(px);
            entry.3 = entry.3.max(py);
        }
    }

    let w = width as f64;
    let h = height as f64;
    #[allow(dead_code)]
    const NODE_R: f64 = 8.0;
    const EDGE_PAD: f64 = 18.0;
    let mut svg = String::with_capacity(64 * 1024);

    svg.push_str(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">\n"
    ));
    // Dark background matching website (#09090B)
    svg.push_str("  <defs>\n    <filter id=\"glow\"><feGaussianBlur stdDeviation=\"2\" result=\"blur\"/><feMerge><feMergeNode in=\"blur\"/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>\n  </defs>\n");
    svg.push_str(&format!(
        "  <rect width=\"{width}\" height=\"{height}\" fill=\"#09090B\"/>\n"
    ));
    svg.push_str(&format!(
        "  <text x=\"{:.1}\" y=\"24\" font-size=\"14\" text-anchor=\"middle\" \
         fill=\"rgba(255,255,255,0.9)\" font-family=\"system-ui, -apple-system, sans-serif\" \
         font-weight=\"600\">{}</text>\n",
        w / 2.0,
        xml_escape(&repo_info.name)
    ));

    for (label, (min_x, min_y, max_x, max_y, cidx)) in &community_bounds {
        let rx = (min_x - EDGE_PAD).max(2.0);
        let ry = (min_y - EDGE_PAD).max(2.0);
        let rw = ((max_x + EDGE_PAD).min(w - 2.0) - rx).max(0.0);
        let rh = ((max_y + EDGE_PAD).min(h - 2.0) - ry).max(0.0);
        svg.push_str(&format!(
            "  <rect x=\"{rx:.1}\" y=\"{ry:.1}\" width=\"{rw:.1}\" height=\"{rh:.1}\" \
             rx=\"12\" ry=\"12\" fill=\"{}\" stroke=\"rgba(255,255,255,0.04)\" stroke-width=\"1\"/>\n",
            COMMUNITY_COLORS[*cidx]
        ));
        svg.push_str(&format!(
            "  <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"rgba(255,255,255,0.35)\" \
             font-family=\"system-ui, -apple-system, sans-serif\" letter-spacing=\"0.05em\">{}</text>\n",
            rx + 6.0,
            (ry + 14.0).min(h - 4.0),
            xml_escape(&svg_truncate(label, 20))
        ));
    }

    for &(si, ti) in &edge_pairs {
        let (x1, y1) = pos[si];
        let (x2, y2) = pos[ti];
        let mx = (x1 + x2) / 2.0;
        let my = (y1 + y2) / 2.0;
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let offset = (len * 0.2).min(20.0);
        let qx = mx - dy / len * offset;
        let qy = my + dx / len * offset;
        svg.push_str(&format!(
            "  <path d=\"M{x1:.1},{y1:.1} Q{qx:.1},{qy:.1} {x2:.1},{y2:.1}\" \
             stroke=\"rgba(255,255,255,0.07)\" stroke-width=\"0.5\" fill=\"none\"/>\n"
        ));
    }

    for (i, s) in symbols.iter().enumerate() {
        let (px, py) = pos[i];
        let color = kind_color(&s.kind);
        svg.push_str(&format!(
            "  <circle cx=\"{px:.1}\" cy=\"{py:.1}\" r=\"4\" fill=\"{color}\" \
             opacity=\"0.85\" filter=\"url(#glow)\"><title>{}</title></circle>\n",
            xml_escape(&s.qualified_name)
        ));
        svg.push_str(&format!(
            "  <text x=\"{px:.1}\" y=\"{:.1}\" font-size=\"7\" text-anchor=\"middle\" \
             fill=\"rgba(255,255,255,0.5)\" font-family=\"system-ui, -apple-system, sans-serif\">{}</text>\n",
            py + 4.0 + 8.0,
            xml_escape(&svg_truncate(&s.name, 12))
        ));
    }

    const LEGEND_ENTRIES: &[(&str, &str)] = &[
        ("Function", "#6366F1"),
        ("Method", "#8B5CF6"),
        ("Class", "#EC4899"),
        ("Interface", "#06B6D4"),
        ("Variable", "#10B981"),
        ("Constant", "#F59E0B"),
        ("Enum", "#EF4444"),
        ("Module", "#14B8A6"),
        ("Section", "#818CF8"),
        ("Document", "#34D399"),
        ("Rationale", "#FBBF24"),
    ];
    let legend_x = 10.0_f64;
    let legend_rows = LEGEND_ENTRIES.len() as f64;
    let legend_y = h - legend_rows * 14.0 - 10.0;
    svg.push_str(&format!(
        "  <rect x=\"{:.1}\" y=\"{:.1}\" width=\"100\" height=\"{:.1}\" rx=\"6\" \
         fill=\"rgba(9,9,11,0.8)\" stroke=\"rgba(255,255,255,0.08)\" stroke-width=\"1\"/>\n",
        legend_x - 2.0,
        legend_y - 4.0,
        legend_rows * 14.0 + 8.0
    ));
    for (i, (name, color)) in LEGEND_ENTRIES.iter().enumerate() {
        let ly = legend_y + i as f64 * 14.0 + 10.0;
        svg.push_str(&format!(
            "  <circle cx=\"{:.1}\" cy=\"{ly:.1}\" r=\"3\" fill=\"{color}\" opacity=\"0.9\"/>\
             <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"8\" fill=\"rgba(255,255,255,0.6)\" \
             font-family=\"system-ui, -apple-system, sans-serif\">{name}</text>\n",
            legend_x + 5.0,
            legend_x + 13.0,
            ly + 3.0,
        ));
    }

    svg.push_str("</svg>\n");

    if let Some(path) = output {
        std::fs::write(path, &svg)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (SVG) to {}",
            n,
            edge_pairs.len(),
            path.display()
        );
    } else {
        println!("{}", svg);
    }

    Ok(())
}

async fn cmd_search(
    query: &str,
    repo: Option<&str>,
    limit: usize,
    use_hybrid: bool,
    use_rerank: bool,
    use_explain: bool,
) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;

    if use_hybrid {
        let embedder = embedder_for_index(&store).await?;
        let mut results = if use_explain {
            hybrid_search_explain(&embedder, &symbols, &store, query, limit).await?
        } else {
            hybrid_search(&embedder, &symbols, &store, query, limit).await?
        };

        // Apply reranking if requested, using the reranker recorded at indexing time
        if use_rerank {
            let reranker_id = embedder.meta().reranker.clone();
            results = rerank_results(query, results, reranker_id.as_deref()).await?;
        }

        // Attach graph edges for explain mode
        if use_explain {
            let relationships = store.get_relationships().await?;
            let uid_to_name: std::collections::HashMap<&str, &str> = symbols
                .iter()
                .map(|s| (s.uid.as_str(), s.name.as_str()))
                .collect();
            attach_graph_edges(&mut results, &relationships, &uid_to_name);
        }

        if results.is_empty() {
            println!("No results found for '{}'", query);
            return Ok(());
        }

        let search_type = if use_rerank {
            "Hybrid search (reranked) results"
        } else {
            "Hybrid search results"
        };
        println!("{} for '{}' in {}:", search_type, query, repo_info.name);
        println!();

        for (i, result) in results.iter().enumerate() {
            let sym = &result.symbol;
            let sources = match (result.bm25_rank, result.vector_rank) {
                (Some(br), Some(vr)) => format!("BM25#{} + Vec#{}", br, vr),
                (Some(br), None) => format!("BM25#{}", br),
                (None, Some(vr)) => format!("Vec#{}", vr),
                (None, None) => "?".to_string(),
            };
            println!(
                "  {}. {} ({}) — {}:{}-{}  [rrf: {:.6}] [{}]",
                i + 1,
                sym.qualified_name,
                sym.kind,
                sym.file_path,
                sym.start_line,
                sym.end_line,
                result.combined_score,
                sources,
            );
            if !sym.signature.is_empty() {
                let sig = sym.signature.lines().next().unwrap_or("");
                if sig.len() > 80 {
                    println!("     {:.80}...", sig);
                } else {
                    println!("     {}", sig);
                }
            }
            if use_explain {
                print_explain_trace(&result.explain);
            }
        }
    } else {
        let results = if use_explain {
            search_symbols_explain(&symbols, query)
        } else {
            search_symbols(&symbols, query)
        };

        if results.is_empty() {
            println!("No results found for '{}'", query);
            return Ok(());
        }

        println!("Search results for '{}' in {}:", query, repo_info.name);
        println!();

        for (i, result) in results.iter().take(limit).enumerate() {
            let sym = &result.symbol;
            println!(
                "  {}. {} ({}) — {}:{}-{}  [score: {:.2}]",
                i + 1,
                sym.qualified_name,
                sym.kind,
                sym.file_path,
                sym.start_line,
                sym.end_line,
                result.score
            );
            if !sym.signature.is_empty() {
                let sig = sym.signature.lines().next().unwrap_or("");
                if sig.len() > 80 {
                    println!("     {:.80}...", sig);
                } else {
                    println!("     {}", sig);
                }
            }
            if use_explain {
                if let Some(ref explain) = result.explain {
                    println!("     --- explain ---");
                    println!(
                        "     doc_len: {:.0}  avg_doc_len: {:.0}",
                        explain.doc_len, explain.avg_doc_len
                    );
                    for ts in &explain.term_scores {
                        println!(
                            "     term '{}': tf={:.0} idf={:.4} tf_norm={:.4} -> {:.4} (in: {})",
                            ts.term,
                            ts.tf,
                            ts.idf,
                            ts.tf_norm,
                            ts.contribution,
                            ts.matched_in.join(", ")
                        );
                    }
                }
            }
        }

        let total = results.len();
        if total > limit {
            println!();
            println!("  ... and {} more results", total - limit);
        }
    }

    Ok(())
}

fn print_explain_trace(explain: &Option<myceliums_core::HybridExplain>) {
    if let Some(ref explain) = explain {
        println!("     --- explain ---");
        if let Some(ref bm25) = explain.bm25 {
            println!(
                "     BM25: doc_len={:.0}  avg_doc_len={:.0}",
                bm25.doc_len, bm25.avg_doc_len
            );
            for ts in &bm25.term_scores {
                println!(
                    "       term '{}': tf={:.0} idf={:.4} tf_norm={:.4} -> {:.4} (in: {})",
                    ts.term,
                    ts.tf,
                    ts.idf,
                    ts.tf_norm,
                    ts.contribution,
                    ts.matched_in.join(", ")
                );
            }
        }
        if !explain.graph_edges.is_empty() {
            println!("     Graph paths:");
            for edge in &explain.graph_edges {
                println!(
                    "       {} --[{}]--> {}",
                    edge.source, edge.kind, edge.target
                );
            }
        }
    }
}

async fn cmd_communities(repo: &str) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let communities = store.get_communities().await?;

    if communities.is_empty() {
        println!("No communities detected. Try re-analyzing with: myc analyze <path>");
        return Ok(());
    }

    println!("Communities in {}:", repo_info.name);
    println!();

    for (i, comm) in communities.iter().enumerate() {
        println!(
            "  {}. {} ({} members)",
            i + 1,
            comm.label,
            comm.member_count
        );
        if !comm.top_symbols.is_empty() {
            println!("     Top symbols: {}", comm.top_symbols);
        }
        if !comm.summary.is_empty() {
            println!("     {}", comm.summary);
        }
    }

    Ok(())
}

async fn cmd_query(query: &str, repo: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let executor = myceliums_cypher::CypherExecutor::from_store(&store).await?;
    let results = executor.execute(query)?;

    if results.is_empty() {
        println!("(no results)");
        return Ok(());
    }

    // Print as a table
    let keys: Vec<String> = results[0].keys().cloned().collect();
    let header = keys.join(" | ");
    println!("{}", header);
    println!("{}", "-".repeat(header.len().max(40)));

    for row in &results {
        let vals: Vec<String> = keys
            .iter()
            .map(|k| {
                let v = row.get(k).cloned().unwrap_or(serde_json::Value::Null);
                match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Null => "null".to_string(),
                    other => other.to_string(),
                }
            })
            .collect();
        println!("{}", vals.join(" | "));
    }

    println!();
    println!("{} row(s)", results.len());
    Ok(())
}

async fn cmd_rename(
    symbol_name: &str,
    new_name: &str,
    repo: Option<&str>,
    apply: bool,
) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    let plan = RenamePlan::create(&symbols, &relationships, symbol_name, new_name)?;

    if plan.edits.is_empty() {
        println!("No references found for '{}'.", symbol_name);
        return Ok(());
    }

    println!(
        "Rename Plan: '{}' -> '{}' ({} edit(s))",
        plan.symbol_name,
        plan.new_name,
        plan.edits.len()
    );
    println!("{}", "=".repeat(60));

    // Group edits by file for display
    let mut current_file = String::new();
    for edit in &plan.edits {
        if edit.file_path != current_file {
            println!();
            println!("  {}:", edit.file_path);
            current_file = edit.file_path.clone();
        }
        println!("    L{}: - {}", edit.line, edit.old_text.trim());
        println!("    L{}: + {}", edit.line, edit.new_text.trim());
    }

    println!();

    if apply {
        let applied = plan.apply()?;
        println!("Applied {} edit(s) across {} file(s).", applied, {
            let mut files: Vec<&str> = plan.edits.iter().map(|e| e.file_path.as_str()).collect();
            files.sort_unstable();
            files.dedup();
            files.len()
        });
    } else {
        println!("Preview only. Use --apply to modify files.");
    }

    Ok(())
}

async fn cmd_impact(repo: Option<&str>, depth: u32, diff: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let diff_text = match diff {
        Some(d) => {
            let path = std::path::Path::new(d);
            if path.exists() && path.is_file() {
                std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read diff file: {}", d))?
            } else {
                d.to_string()
            }
        }
        None => run_git_diff(&repo_info.path)
            .with_context(|| format!("Failed to run git diff in {}", repo_info.path))?,
    };

    if diff_text.trim().is_empty() {
        println!("No changes detected (empty diff).");
        return Ok(());
    }

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    let report = detect_impact(&diff_text, &symbols, &relationships, depth);

    println!("Impact Report for {}", repo_info.name);
    println!("{}", "=".repeat(60));

    if report.directly_changed.is_empty() {
        println!("\nNo symbols directly affected by the diff.");
    } else {
        println!(
            "\nDirectly changed symbols ({}):",
            report.directly_changed.len()
        );
        for sym in &report.directly_changed {
            println!(
                "  [{}] {} ({}) -- {}",
                sym.change_type, sym.qualified_name, sym.kind, sym.file_path
            );
        }
    }

    if !report.indirectly_affected.is_empty() {
        println!(
            "\nIndirectly affected symbols ({}):",
            report.indirectly_affected.len()
        );
        for sym in &report.indirectly_affected {
            println!(
                "  [{} at distance {}] {} ({}) -- {}",
                sym.relationship, sym.distance, sym.qualified_name, sym.kind, sym.file_path
            );
        }
    }

    if !report.affected_files.is_empty() {
        println!("\nAffected files ({}):", report.affected_files.len());
        for file in &report.affected_files {
            println!("  {}", file);
        }
    }

    println!("\nRisk score: {:.2} / 10.00", report.risk_score);
    Ok(())
}

async fn cmd_knowledge(query: &str, repo: Option<&str>, limit: usize) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    // Search for matching code symbols
    let search_results = search_symbols(&symbols, query);

    // Build UID lookup
    let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // Find Mentions relationships targeting matched symbols
    let matched_uids: std::collections::HashSet<&str> = search_results
        .iter()
        .take(50)
        .map(|r| r.symbol.uid.as_str())
        .collect();

    let mention_rels: Vec<&myceliums_storage::Relationship> = relationships
        .iter()
        .filter(|r| {
            r.kind == myceliums_storage::RelationshipKind::Mentions
                && matched_uids.contains(r.target_uid.as_str())
        })
        .collect();

    println!("Knowledge Query: \"{}\"", query);
    println!("{}", "=".repeat(60));

    if mention_rels.is_empty() {
        println!("\nNo cross-domain mentions found for \"{}\".", query);
        println!("Tip: Run `myc analyze` on a directory containing both code and");
        println!("     content files (emails, docs, markdown) to build mentions.");
        return Ok(());
    }

    // Group by source symbol
    let mut by_source: HashMap<&str, Vec<&myceliums_storage::Relationship>> = HashMap::new();
    for rel in &mention_rels {
        by_source
            .entry(rel.source_uid.as_str())
            .or_default()
            .push(rel);
    }

    let total_mentions = mention_rels.len();
    let source_count = by_source.len();
    println!(
        "\nFound {} mention(s) across {} source(s):\n",
        total_mentions, source_count
    );

    let mut shown = 0;
    for (source_uid, rels) in &by_source {
        if shown >= limit {
            break;
        }
        if let Some(source) = uid_to_symbol.get(source_uid) {
            shown += 1;
            let kind_label = format!("{}", source.kind);
            println!(
                "  {}. {}: \"{}\" ({})",
                shown, kind_label, source.name, source.file_path
            );

            for rel in rels {
                if let Some(target) = uid_to_symbol.get(rel.target_uid.as_str()) {
                    println!(
                        "     → mentions: {} ({}) at {}:{}",
                        target.name, target.kind, target.file_path, target.start_line
                    );

                    // Parse metadata for context
                    if let Ok(meta) = serde_json::from_str::<
                        myceliums_core::mentions::MentionMetadata,
                    >(&rel.metadata)
                    {
                        if let Some(first_match) = meta.matches.first() {
                            println!("     Context: \"{}\"", first_match.context);
                            println!(
                                "     Source: {}, line {}",
                                source.file_path, first_match.line
                            );
                        }
                    }
                }
            }
            println!();
        }
    }

    Ok(())
}

async fn cmd_processes(
    repo: &str,
    entry: Option<String>,
    filter: Option<String>,
    limit: Option<usize>,
    min_steps: Option<u32>,
) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let processes = store.get_processes().await?;

    if processes.is_empty() {
        println!("No processes detected. Try re-analyzing with: myc analyze <path>");
        return Ok(());
    }

    // Check if any filters are applied
    let has_filters = entry.is_some() || filter.is_some() || limit.is_some() || min_steps.is_some();

    // Apply filters
    let filter_obj = ProcessFilter {
        entry,
        filter,
        limit,
        min_steps,
    };
    let filtered = filter_obj.apply(&processes);

    if filtered.is_empty() {
        println!("No processes match filter criteria.");
        return Ok(());
    }

    println!("Processes in {}:", repo_info.name);
    if has_filters {
        println!("(filtered)");
    }
    println!();

    for (i, proc) in filtered.iter().enumerate() {
        println!(
            "  {}. {} ({} steps, entry: {})",
            i + 1,
            proc.name,
            proc.step_count,
            proc.entry_point
        );
        println!("     Flow: {}", proc.description);
        println!();
    }

    Ok(())
}

async fn cmd_doctor(download: bool) -> Result<()> {
    use myceliums_core::parser::SourceLanguage;
    use std::process::Command;
    use tree_sitter::Parser as TsParser;

    println!("Myceliums Health Check");
    println!("=====================");
    println!();

    let data = data_dir();
    let mut issues = 0u32;

    // ── Rust toolchain (informational) ────────────────────────────────
    match Command::new("rustc").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("\u{2713} Rust toolchain: {}", version.trim());
        }
        _ => {
            println!("- Rust toolchain: not found (optional, only needed for development)");
        }
    }

    match Command::new("cargo").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("\u{2713} Cargo: {}", version.trim());
        }
        _ => {
            println!("- Cargo: not found (optional, only needed for development)");
        }
    }

    // ── Data directory ────────────────────────────────────────────────
    if data.exists() && data.is_dir() {
        let test_file = data.join(".write_test");
        match std::fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_file);
                println!(
                    "\u{2713} Data directory ({}) exists and is writable",
                    data.display()
                );
            }
            Err(_) => {
                println!(
                    "\u{2717} Data directory ({}) exists but is NOT writable",
                    data.display()
                );
                issues += 1;
            }
        }
    } else {
        println!(
            "\u{2717} Data directory ({}) does not exist — run 'myc analyze <path>' to create it",
            data.display()
        );
        issues += 1;
    }

    // ── Registry file ─────────────────────────────────────────────────
    let reg_path = registry_path();
    let registry = if reg_path.exists() {
        match RepoRegistry::load(&reg_path) {
            Ok(reg) => {
                println!("\u{2713} Registry file (repos.json) is valid");
                Some(reg)
            }
            Err(e) => {
                println!(
                    "\u{2717} Registry file (repos.json) exists but is invalid: {}",
                    e
                );
                issues += 1;
                None
            }
        }
    } else {
        println!(
            "\u{2713} Registry file (repos.json) not yet created (will be created on first analyze)"
        );
        None
    };

    // ── LanceDB repository data ───────────────────────────────────────
    let mut total_repos = 0u32;
    let mut healthy_repos = 0u32;
    if let Some(ref registry) = registry {
        let repos = registry.list();
        total_repos = repos.len() as u32;
        for repo in &repos {
            let db_path = RepoRegistry::repo_db_path(&data, &repo.id);
            if db_path.exists() && db_path.is_dir() {
                let table_count = std::fs::read_dir(&db_path)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .count()
                    })
                    .unwrap_or(0);
                println!("\u{2713} LanceDB: {} ({} tables)", repo.name, table_count);
                healthy_repos += 1;
            } else {
                println!(
                    "\u{2717} LanceDB: {} (data directory missing at {})",
                    repo.name,
                    db_path.display()
                );
                issues += 1;
            }
        }
    }

    // ── Orphaned data directories ────────────────────────────────────
    match find_orphans() {
        Ok(orphans) if orphans.is_empty() => {
            println!("\u{2713} No orphaned data directories");
        }
        Ok(orphans) => {
            let total: u64 = orphans.iter().map(|(_, _, s)| s).sum();
            println!(
                "\u{2717} {} orphaned data directories ({}) — run 'myc clean --orphans' to remove",
                orphans.len(),
                format_bytes(total)
            );
            for (name, _, size) in &orphans {
                println!("    {} ({})", name, format_bytes(*size));
            }
            issues += 1;
        }
        Err(e) => {
            println!("\u{2717} Could not check for orphaned directories: {}", e);
            issues += 1;
        }
    }

    // ── Stale analysis locks ──────────────────────────────────────────
    {
        let data_path = data.join("data");
        let mut stale_locks = 0u32;
        if data_path.exists() {
            for entry in std::fs::read_dir(&data_path)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
            {
                let lock_path = entry.path().join("analysis.lock");
                if lock_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&lock_path) {
                        let pid = content
                            .lines()
                            .next()
                            .and_then(|l| l.trim().parse::<u32>().ok());
                        let alive = pid
                            .map(|p| {
                                std::process::Command::new("kill")
                                    .args(["-0", &p.to_string()])
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .map(|s| s.success())
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false);
                        if !alive {
                            stale_locks += 1;
                            println!(
                                "\u{2717} Stale lock: {} (PID {:?})",
                                lock_path.display(),
                                pid
                            );
                        }
                    }
                }
            }
        }
        if stale_locks == 0 {
            println!("\u{2713} No stale analysis locks");
        } else {
            println!("  Run 'myc clean --orphans' or manually remove the .lock files");
            issues += stale_locks;
        }
    }

    // ── Tree-sitter grammars ──────────────────────────────────────────
    let parsers_to_check = [
        ("TypeScript", SourceLanguage::TypeScript),
        ("TSX", SourceLanguage::Tsx),
        ("JavaScript", SourceLanguage::JavaScript),
        ("Python", SourceLanguage::Python),
        ("Go", SourceLanguage::Go),
        ("Rust", SourceLanguage::Rust),
        ("Java", SourceLanguage::Java),
        ("C#", SourceLanguage::CSharp),
        ("C", SourceLanguage::C),
        ("C++", SourceLanguage::Cpp),
        ("Ruby", SourceLanguage::Ruby),
        ("Kotlin", SourceLanguage::Kotlin),
        ("Swift", SourceLanguage::Swift),
        ("PHP", SourceLanguage::Php),
        ("Lua", SourceLanguage::Lua),
        ("Zig", SourceLanguage::Zig),
        ("PowerShell", SourceLanguage::PowerShell),
        ("Elixir", SourceLanguage::Elixir),
        ("Scala", SourceLanguage::Scala),
        ("Objective-C", SourceLanguage::ObjectiveC),
        ("Dart", SourceLanguage::Dart),
        ("Vue", SourceLanguage::Vue),
        ("Svelte", SourceLanguage::Svelte),
    ];
    for (name, lang) in &parsers_to_check {
        let mut parser = TsParser::new();
        let ts_lang: tree_sitter::Language = match lang {
            SourceLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SourceLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SourceLanguage::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            SourceLanguage::Python => tree_sitter_python::LANGUAGE.into(),
            SourceLanguage::Go => tree_sitter_go::LANGUAGE.into(),
            SourceLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
            SourceLanguage::Java => tree_sitter_java::LANGUAGE.into(),
            SourceLanguage::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            SourceLanguage::C => tree_sitter_c::LANGUAGE.into(),
            SourceLanguage::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            SourceLanguage::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            SourceLanguage::Kotlin => tree_sitter_kotlin::LANGUAGE.into(),
            SourceLanguage::Swift => tree_sitter_swift::LANGUAGE.into(),
            SourceLanguage::Php => tree_sitter_php::LANGUAGE_PHP_ONLY.into(),
            SourceLanguage::Lua => tree_sitter_lua::LANGUAGE.into(),
            SourceLanguage::Zig => tree_sitter_zig::LANGUAGE.into(),
            SourceLanguage::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
            SourceLanguage::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            SourceLanguage::Scala => tree_sitter_scala::LANGUAGE.into(),
            SourceLanguage::ObjectiveC => tree_sitter_objc::LANGUAGE.into(),
            SourceLanguage::Dart => tree_sitter_dart::LANGUAGE.into(),
            SourceLanguage::Vue | SourceLanguage::Svelte => {
                // Vue/Svelte parse script blocks as TypeScript
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
            SourceLanguage::Markdown
            | SourceLanguage::Mdx
            | SourceLanguage::PlainText
            | SourceLanguage::Jupyter
            | SourceLanguage::Email
            | SourceLanguage::Mbox => {
                unreachable!("content/notebook languages not in parsers_to_check")
            }
            #[cfg(feature = "pdf")]
            SourceLanguage::Pdf => {
                unreachable!("PDF language not in parsers_to_check")
            }
        };
        match parser.set_language(&ts_lang) {
            Ok(_) => println!("\u{2713} tree-sitter grammar: {}", name),
            Err(e) => {
                println!("\u{2717} tree-sitter grammar: {} — {}", name, e);
                issues += 1;
            }
        }
    }

    // ── Embedding configuration + model cache ─────────────────────────
    // Resolve the embedding config the same way analysis would: from
    // .myceliums.toml in the current directory, falling back to defaults.
    let embedding_cfg = {
        let config_path = std::path::Path::new(config::CONFIG_FILENAME);
        if config_path.exists() {
            match ProjectConfig::load(config_path) {
                Ok(cfg) => cfg.embedding,
                Err(e) => {
                    println!("\u{2717} .myceliums.toml is invalid: {}", e);
                    issues += 1;
                    Default::default()
                }
            }
        } else {
            Default::default()
        }
    };
    match IndexEmbeddingMeta::from_config(&embedding_cfg) {
        Ok(meta) => {
            println!(
                "\u{2713} Embedding config: {} (reranker: {})",
                meta.fingerprint(),
                embedding_cfg.reranker,
            );
            if meta.provider == "local" {
                let model_code = local_model_code(&meta.model)?;
                let cache_info = check_model_cache(&model_code);
                if cache_info.is_cached {
                    println!(
                        "\u{2713} Embedding model: cached ({:.0} MB at {})",
                        cache_info.size_bytes as f64 / 1_000_000.0,
                        cache_info.cache_dir.display(),
                    );
                } else if download {
                    println!("Downloading embedding model {}...", model_code);
                    match Embedder::new(meta.clone()) {
                        Ok(_) => {
                            let updated = check_model_cache(&model_code);
                            println!(
                                "\u{2713} Embedding model: downloaded ({:.0} MB to {})",
                                updated.size_bytes as f64 / 1_000_000.0,
                                updated.cache_dir.display(),
                            );
                        }
                        Err(e) => {
                            println!("\u{2717} Embedding model: download failed — {}", e);
                            issues += 1;
                        }
                    }
                } else {
                    println!(
                        "\u{2717} Embedding model: not downloaded (run `myc doctor --download` or it will download on first use)"
                    );
                    issues += 1;
                }
            }
            println!(
                "  Available local models: {}",
                myceliums_core::EMBEDDING_MODELS
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        Err(e) => {
            println!("\u{2717} Embedding config: {}", e);
            issues += 1;
        }
    }

    // ── Content type support ────────────────────────────────────────────
    let content_types = [
        "Markdown (.md, .markdown)",
        "MDX (.mdx)",
        "Plain text (.txt)",
    ];
    for ct in &content_types {
        println!("\u{2713} Content type supported: {}", ct);
    }

    // ── Global configuration ──────────────────────────────────────────
    match GlobalConfig::load(&data) {
        Ok(_) => println!("\u{2713} Global configuration is valid"),
        Err(e) => {
            println!("\u{2717} Global configuration is invalid: {}", e);
            issues += 1;
        }
    }

    // ── Project config in CWD (if present) ────────────────────────────
    if let Ok(cwd) = std::env::current_dir() {
        let project_config_path = cwd.join(config::CONFIG_FILENAME);
        if project_config_path.exists() {
            match ProjectConfig::load(&project_config_path) {
                Ok(cfg) => {
                    let name = if cfg.project.name.is_empty() {
                        "(default)".to_string()
                    } else {
                        cfg.project.name.clone()
                    };
                    println!(
                        "\u{2713} Project config ({}) is valid — project: {}",
                        config::CONFIG_FILENAME,
                        name
                    );
                }
                Err(e) => {
                    println!(
                        "\u{2717} Project config ({}) is invalid: {}",
                        config::CONFIG_FILENAME,
                        e
                    );
                    issues += 1;
                }
            }
        }
    }

    // ── Summary ───────────────────────────────────────────────────────
    println!();
    if total_repos > 0 {
        let needs_reanalysis = total_repos - healthy_repos;
        if needs_reanalysis > 0 {
            println!(
                "{} repositories registered, {} healthy, {} need re-analysis",
                total_repos, healthy_repos, needs_reanalysis
            );
        } else {
            println!("{} repositories registered, all healthy", total_repos);
        }
    }

    if issues == 0 {
        println!("\u{2713} All checks passed");
    } else {
        println!("\u{2717} {} issue(s) found", issues);
        std::process::exit(1);
    }

    Ok(())
}

async fn cmd_configure(set: Option<&str>, reset: bool) -> Result<()> {
    let data = data_dir();
    let mut config = GlobalConfig::load(&data)?;

    if reset {
        config.reset();
        config.save()?;
        println!("Configuration reset to defaults.");
        println!();
        print!("{}", config.display());
        return Ok(());
    }

    if let Some(kv) = set {
        let (key, value) = kv
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("Invalid format. Use: --set key=value"))?;
        config.set(key.trim(), value.trim())?;
        config.save()?;
        println!("Set {} = {}", key.trim(), value.trim());
        return Ok(());
    }

    // Show current config
    print!("{}", config.display());
    Ok(())
}

async fn cmd_semantic_search(query: &str, repo: Option<&str>, limit: usize) -> Result<()> {
    let (_, ri) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &ri.id);
    let store = Store::open(&db_path, &ri.id).await?;

    let embedder = embedder_for_index(&store)
        .await
        .context("Failed to load embedding model")?;
    let query_vector = embedder
        .embed_query(query)
        .await
        .context("Failed to embed query")?;

    let results = store.vector_search(&query_vector, limit).await?;

    if results.is_empty() {
        println!(
            "No semantic results found for '{}'. Have you run 'myc analyze' first?",
            query
        );
        return Ok(());
    }

    println!("Semantic search results for '{}' in {}:\n", query, ri.name);
    for (i, (sym, score)) in results.iter().enumerate() {
        println!(
            "  {}. {} ({}) -- {}:{}-{}  [similarity: {:.4}]",
            i + 1,
            sym.qualified_name,
            sym.kind,
            sym.file_path,
            sym.start_line,
            sym.end_line,
            score
        );
        if !sym.signature.is_empty() {
            let sig = sym.signature.lines().next().unwrap_or("");
            if sig.len() > 80 {
                println!("     {:.80}...", sig);
            } else {
                println!("     {}", sig);
            }
        }
    }

    Ok(())
}

/// Output a JSON systemMessage for Claude Code hooks to display.
fn hook_system_message(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"systemMessage\":\"{}\"}}", escaped);
}

const INSTRUCTION_SUFFIX: &str =
    " | For code exploration, prefer myceliums MCP tools (context_search, symbol_context, hybrid_search) over grep \u{2014} they return structured results with exact locations in a single call.";

/// Output a hook systemMessage, optionally appending AI usage instructions.
fn hook_system_message_with_instructions(msg: &str) {
    let config = myceliums_core::GlobalConfig::load(&data_dir()).unwrap_or_default();
    if config.setup.instructions_enabled {
        hook_system_message(&format!("{}{}", msg, INSTRUCTION_SUFFIX));
    } else {
        hook_system_message(msg);
    }
}

/// Output a JSON hookSpecificOutput for Claude Code PostToolUse hooks.
fn hook_post_tool_message(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"{}\"}}}}",
        escaped
    );
}

/// Count supported source files in a directory (without parsing them).
fn count_source_files(path: &Path) -> usize {
    use walkdir::WalkDir;

    let supported_extensions = ["ts", "tsx", "js", "jsx", "py"];

    WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
                && name != "node_modules"
                && name != "__pycache__"
                && name != "target"
                && name != "dist"
                && name != "build"
                && name != ".venv"
                && name != "venv"
        })
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| supported_extensions.contains(&ext))
                    .unwrap_or(false)
        })
        .count()
}

/// Estimate analysis time based on file count.
fn estimate_time(file_count: usize) -> &'static str {
    match file_count {
        0 => "no supported files found",
        1..=50 => "~5 seconds",
        51..=100 => "~10-15 seconds",
        101..=500 => "~30-60 seconds",
        501..=1000 => "~1-3 minutes",
        1001..=3000 => "~5-10 minutes",
        _ => "~10-20 minutes",
    }
}

/// Prompt user with a yes/no question, returns true if yes.
fn prompt_yes_no(question: &str) -> Result<bool> {
    print!("{} [y/N] ", question);
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes" || answer == "j" || answer == "ja")
}

async fn cmd_session(
    path: &Path,
    auto_yes: bool,
    timeout_secs: u64,
    no_git_check: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let project_name = abs_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Apply safeguards (home dir, git repo)
    if let Err(e) = check_session_safeguards(path, no_git_check) {
        if auto_yes {
            hook_system_message(&format!("[myceliums] {} skipped | {}", project_name, e));
            return Ok(());
        }
        return Err(e);
    }

    let repo_id = analyzer::repo_id_from_path(&abs_path);

    // In auto mode (--yes), output JSON for Claude Code's SessionStart hook
    if auto_yes {
        let registry = RepoRegistry::load(&registry_path())?;
        if let Some(repo_info) = registry.get(&repo_id) {
            let cache_config = CacheCheckConfig::default();
            match cache::check_cache(repo_info, &abs_path, &cache_config) {
                CacheDecision::UseCached { .. } => {
                    hook_system_message_with_instructions(&format!(
                        "[myceliums] {} ready | {} files \u{00b7} {} symbols",
                        project_name, repo_info.file_count, repo_info.symbol_count,
                    ));
                    return Ok(());
                }
                CacheDecision::ReanalyzeNeeded { .. } => {
                    let analyze_fut = cmd_analyze(&abs_path, true, 60, true);
                    if timeout_secs > 0 {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(timeout_secs),
                            analyze_fut,
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                hook_system_message(&format!(
                                    "[myceliums] {} error | {}",
                                    project_name, e
                                ));
                                return Ok(());
                            }
                            Err(_) => {
                                hook_system_message(&format!(
                                    "[myceliums] {} timed out after {}s",
                                    project_name, timeout_secs
                                ));
                                return Ok(());
                            }
                        }
                    } else {
                        analyze_fut.await?;
                    }
                    let registry = RepoRegistry::load(&registry_path())?;
                    if let Some(repo_info) = registry.get(&repo_id) {
                        hook_system_message_with_instructions(&format!(
                            "[myceliums] {} updated | {} files \u{00b7} {} symbols",
                            project_name, repo_info.file_count, repo_info.symbol_count,
                        ));
                    }
                    return Ok(());
                }
            }
        }

        // No analysis exists — auto-analyze
        let file_count = count_source_files(&abs_path);
        if file_count == 0 {
            hook_system_message(&format!(
                "[myceliums] {} skipped | no supported files found",
                project_name
            ));
            return Ok(());
        }
        let analyze_fut = cmd_analyze(&abs_path, true, 60, true);
        if timeout_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), analyze_fut)
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    hook_system_message(&format!("[myceliums] {} error | {}", project_name, e));
                    return Ok(());
                }
                Err(_) => {
                    hook_system_message(&format!(
                        "[myceliums] {} timed out after {}s",
                        project_name, timeout_secs
                    ));
                    return Ok(());
                }
            }
        } else {
            analyze_fut.await?;
        }
        let registry = RepoRegistry::load(&registry_path())?;
        if let Some(repo_info) = registry.get(&repo_id) {
            hook_system_message_with_instructions(&format!(
                "[myceliums] {} analyzed | {} files \u{00b7} {} symbols",
                project_name, repo_info.file_count, repo_info.symbol_count,
            ));
        }
        return Ok(());
    }

    // Interactive mode — verbose output
    println!();
    println!("  Myceliums Session — {}", project_name);
    println!("  {}", "─".repeat(40));

    // Check if analysis already exists and is fresh
    let registry = RepoRegistry::load(&registry_path())?;
    if let Some(repo_info) = registry.get(&repo_id) {
        let cache_config = CacheCheckConfig::default();
        match cache::check_cache(repo_info, &abs_path, &cache_config) {
            CacheDecision::UseCached { reason, .. } => {
                println!("  Cache:     fresh ({})", reason);
                println!("  Analyzed:  {}", repo_info.analyzed_at);
                println!("  Symbols:   {}", repo_info.symbol_count);
                println!("  Files:     {}", repo_info.file_count);
                println!();
                println!("  Ready. MCP tools available via 'myc mcp'.");
                println!();
                return Ok(());
            }
            CacheDecision::ReanalyzeNeeded { reason } => {
                println!("  Cache:     stale ({})", reason);
                println!(
                    "  Previous:  {} symbols, {} files",
                    repo_info.symbol_count, repo_info.file_count
                );
                println!();

                if !prompt_yes_no("  Re-analyze now?")? {
                    println!("  Skipped. Using stale cache.");
                    return Ok(());
                }

                // Skip embeddings for fast session startup
                cmd_analyze(&abs_path, true, 60, true).await?;
                println!();
                println!("  Ready. MCP tools available via 'myc mcp'.");
                println!();
                return Ok(());
            }
        }
    }

    // No analysis exists — count files and offer to create one
    let file_count = count_source_files(&abs_path);
    let estimate = estimate_time(file_count);

    println!("  Status:    no analysis found");
    println!("  Files:     {} supported source files", file_count);
    println!("  Estimate:  {}", estimate);
    println!();

    if file_count == 0 {
        println!("  No TypeScript, JavaScript, or Python files found.");
        println!("  Myceliums currently supports: .ts, .tsx, .js, .jsx, .py");
        println!();
        return Ok(());
    }

    if !prompt_yes_no("  Analyze now?")? {
        println!("  Skipped. Run 'myc analyze .' later to create the index.");
        return Ok(());
    }

    println!();
    // Skip embeddings for fast session startup
    cmd_analyze(&abs_path, true, 60, true).await?;

    println!();
    println!("  Ready. MCP tools available via 'myc mcp'.");
    println!("  Tip: Run 'myc analyze . --force' later to add semantic search embeddings.");
    println!();

    Ok(())
}

async fn cmd_format_hook() -> Result<()> {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);

    let tool_name = json["tool_name"].as_str().unwrap_or("");
    let tool_input = &json["tool_input"];
    let tool_response = &json["tool_response"];

    let short_name = tool_name
        .strip_prefix("mcp__myceliums__")
        .unwrap_or(tool_name);

    let result_text = tool_response
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("");

    let result: serde_json::Value =
        serde_json::from_str(result_text).unwrap_or(serde_json::Value::Null);

    let msg = format_hook_message(short_name, tool_input, &result);
    if !msg.is_empty() {
        hook_post_tool_message(&msg);
    }

    Ok(())
}

fn format_hook_message(
    short_name: &str,
    tool_input: &serde_json::Value,
    result: &serde_json::Value,
) -> String {
    fn truncate(s: &str, max: usize) -> String {
        if s.len() > max {
            format!("{}...", &s[..max])
        } else {
            s.to_string()
        }
    }

    // All tools now return { "text": "..." } with formatted output.
    // Extract the first line as a concise summary for the hook.
    let result_text = result["text"].as_str().unwrap_or("");
    let first_line = result_text.lines().next().unwrap_or("");

    match short_name {
        "context_search" | "search_documents" | "semantic_search" => {
            let query = tool_input["query"].as_str().unwrap_or("?");
            // First line is like "Found 15 results for "query":"
            if first_line.starts_with("Found") || first_line.starts_with("No results") {
                format!("[myceliums] {}", first_line)
            } else {
                format!("[myceliums] Search: \"{}\"", truncate(query, 40))
            }
        }
        "hybrid_search" => {
            if first_line.starts_with("Found") || first_line.starts_with("No results") {
                format!("[myceliums] {}", first_line)
            } else {
                let query = tool_input["query"].as_str().unwrap_or("?");
                format!("[myceliums] Hybrid search: \"{}\"", truncate(query, 40))
            }
        }
        "symbol_context" => {
            // First line is like "Symbol: AppInner (Function)"
            if first_line.starts_with("Symbol:") {
                format!("[myceliums] {}", first_line)
            } else {
                let name = tool_input["symbol_name"].as_str().unwrap_or("?");
                format!("[myceliums] Symbol: {}", name)
            }
        }
        "analyze" => {
            // First line is like "Analysis complete (cached):"
            if first_line.starts_with("Analysis") {
                format!("[myceliums] {}", first_line.trim_end_matches(':'))
            } else {
                "[myceliums] Analysis complete".to_string()
            }
        }
        "detect_impact" => {
            if first_line.starts_with("Impact") {
                format!("[myceliums] {}", first_line.trim_end_matches(':'))
            } else {
                "[myceliums] Impact analysis complete".to_string()
            }
        }
        "cypher_query" => {
            let query = tool_input["query"].as_str().unwrap_or("?");
            format!("[myceliums] Cypher: {}", truncate(query, 50))
        }
        "get_processes" => {
            if first_line.starts_with("Found") || first_line.starts_with("No ") {
                format!("[myceliums] {}", first_line)
            } else {
                "[myceliums] Processes loaded".to_string()
            }
        }
        "rename_symbol" => {
            if first_line.starts_with("Rename:") {
                format!("[myceliums] {}", first_line)
            } else {
                let from = tool_input["symbol_name"].as_str().unwrap_or("?");
                let to = tool_input["new_name"].as_str().unwrap_or("?");
                format!("[myceliums] Rename: {} -> {}", from, to)
            }
        }
        _ => format!("[myceliums] {}", short_name),
    }
}

/// Unified setup command that auto-detects or configures specific editors
async fn cmd_setup(editor: Option<String>, uninstall: bool) -> Result<()> {
    let myc_bin = std::env::current_exe().context("Could not determine myc binary path")?;
    let myc_path = myc_bin.to_string_lossy().to_string();

    if uninstall {
        if let Some(editor_name) = editor {
            setup::SetupOrchestrator::uninstall_editor(&editor_name)
        } else {
            setup::SetupOrchestrator::uninstall_all()
        }
    } else if let Some(editor_name) = editor {
        setup::SetupOrchestrator::setup_editor(&editor_name, &myc_path)
    } else {
        // No flags → run interactive wizard
        let is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if is_terminal {
            setup::wizard::run_wizard(&data_dir()).await?;
            Ok(())
        } else {
            // Non-interactive: fall back to auto-detect all
            setup::wizard::run_setup_all(&data_dir()).await
        }
    }
}

/// Setup a specific editor
async fn cmd_setup_editor(editor_name: &str, uninstall: bool) -> Result<()> {
    let myc_bin = std::env::current_exe().context("Could not determine myc binary path")?;
    let myc_path = myc_bin.to_string_lossy().to_string();

    if uninstall {
        setup::SetupOrchestrator::uninstall_editor(editor_name)
    } else {
        setup::SetupOrchestrator::setup_editor(editor_name, &myc_path)
    }
}

async fn cmd_setup_claude(uninstall: bool) -> Result<()> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let myc_bin = std::env::current_exe().context("Could not determine myc binary path")?;
    let myc_path = myc_bin.to_string_lossy().to_string();

    if uninstall {
        return cmd_setup_claude_uninstall(&home, &myc_path);
    }

    println!("Setting up Claude Code integration...");
    println!();

    // 1. Register MCP server via `claude mcp add`
    let mcp_result = setup_mcp_server(&myc_path);
    match &mcp_result {
        Ok(_) => println!("  \u{2713} MCP server registered"),
        Err(e) => println!("  \u{2717} MCP server: {}", e),
    }

    // 2. Add hooks to ~/.claude/settings.json
    let hooks_result = setup_hooks(&home, &myc_path);
    match &hooks_result {
        Ok(_) => println!("  \u{2713} Hooks configured (SessionStart + PostToolUse)"),
        Err(e) => println!("  \u{2717} Hooks: {}", e),
    }

    println!();

    if mcp_result.is_ok() && hooks_result.is_ok() {
        println!("  Setup complete! Start Claude Code in any git project:");
        println!("    claude");
        println!();
        println!("  You should see:");
        println!("    SessionStart:startup says: [myceliums] <project> ready | ...");
        println!();
        println!("  To remove: myc setup-claude --uninstall");
    } else {
        println!("  Setup completed with errors. Check messages above.");
    }

    Ok(())
}

fn setup_mcp_server(myc_path: &str) -> Result<()> {
    // Try `claude mcp add` first (the official way)
    let output = std::process::Command::new("claude")
        .args([
            "mcp",
            "add",
            "-s",
            "user",
            "myceliums",
            "--",
            myc_path,
            "mcp",
        ])
        .output();

    match output {
        Ok(result) if result.status.success() => Ok(()),
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            // If server already exists, try removing and re-adding
            if stderr.contains("already exists") {
                let _ = std::process::Command::new("claude")
                    .args(["mcp", "remove", "-s", "user", "myceliums"])
                    .output();
                let retry = std::process::Command::new("claude")
                    .args([
                        "mcp",
                        "add",
                        "-s",
                        "user",
                        "myceliums",
                        "--",
                        myc_path,
                        "mcp",
                    ])
                    .output()?;
                if retry.status.success() {
                    return Ok(());
                }
            }
            // Fallback: edit ~/.claude.json directly
            setup_mcp_server_manual(myc_path)
        }
        Err(_) => {
            // `claude` CLI not found, edit manually
            setup_mcp_server_manual(myc_path)
        }
    }
}

fn setup_mcp_server_manual(myc_path: &str) -> Result<()> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let config_path = home.join(".claude.json");

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = config
        .as_object_mut()
        .context("Invalid .claude.json")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    servers
        .as_object_mut()
        .context("Invalid mcpServers")?
        .insert(
            "myceliums".to_string(),
            serde_json::json!({
                "type": "stdio",
                "command": myc_path,
                "args": ["mcp"],
                "env": {}
            }),
        );

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn setup_hooks(home: &std::path::Path, myc_path: &str) -> Result<()> {
    let settings_dir = home.join(".claude");
    std::fs::create_dir_all(&settings_dir)?;
    let settings_path = settings_dir.join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let obj = settings.as_object_mut().context("Invalid settings.json")?;

    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("Invalid hooks object")?;

    // SessionStart hook
    let session_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{
            "type": "command",
            "command": format!("{} session . --yes --timeout 300 2>/dev/null; exit 0", myc_path)
        }]
    }]);

    // PostToolUse hook
    let post_tool_hook = serde_json::json!([{
        "matcher": "mcp__myceliums__",
        "hooks": [{
            "type": "command",
            "command": format!("{} format-hook 2>/dev/null", myc_path)
        }]
    }]);

    // Merge with existing hooks (don't overwrite other hooks)
    merge_hook_array(hooks, "SessionStart", session_hook, myc_path);
    merge_hook_array(hooks, "PostToolUse", post_tool_hook, myc_path);

    std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
    Ok(())
}

fn merge_hook_array(
    hooks: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    new_hooks: serde_json::Value,
    myc_path: &str,
) {
    if let Some(existing) = hooks.get_mut(event) {
        if let Some(arr) = existing.as_array_mut() {
            // Remove any existing myceliums hooks
            arr.retain(|entry| {
                let is_myceliums = entry["hooks"]
                    .as_array()
                    .map(|h| {
                        h.iter().any(|hook| {
                            hook["command"]
                                .as_str()
                                .map(|c| c.contains("myc ") || c.contains(myc_path))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                !is_myceliums
            });
            // Add new hooks
            if let Some(new_arr) = new_hooks.as_array() {
                arr.extend(new_arr.iter().cloned());
            }
            return;
        }
    }
    hooks.insert(event.to_string(), new_hooks);
}

/// Generic MCP config setup for platforms that use a simple JSON config file.
fn cmd_setup_mcp_platform(platform: &str, config_rel_path: &str, uninstall: bool) -> Result<()> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let myc_bin = std::env::current_exe().context("Could not determine myc binary path")?;
    let myc_path = myc_bin.to_string_lossy().to_string();
    let config_path = home.join(config_rel_path);

    if uninstall {
        return cmd_uninstall_mcp_platform(platform, &config_path);
    }

    println!("Setting up {} integration...", platform);
    println!();

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Could not create directory: {}", parent.display()))?;
    }

    // Read existing config or start fresh
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = config
        .as_object_mut()
        .context("Invalid MCP config")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    servers
        .as_object_mut()
        .context("Invalid mcpServers")?
        .insert(
            "myceliums".to_string(),
            serde_json::json!({
                "command": myc_path,
                "args": ["mcp"]
            }),
        );

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    println!(
        "  \u{2713} MCP server registered in {}",
        config_path.display()
    );
    println!();
    println!("  Setup complete! Verify by checking:");
    println!("    {}", config_path.display());
    println!();
    println!(
        "  To remove: myc setup-{} --uninstall",
        platform.to_lowercase()
    );

    Ok(())
}

fn cmd_uninstall_mcp_platform(platform: &str, config_path: &std::path::Path) -> Result<()> {
    println!("Removing {} integration...", platform);
    println!();

    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(config_path) {
            if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut())
                {
                    servers.remove("myceliums");
                    let _ = std::fs::write(
                        config_path,
                        serde_json::to_string_pretty(&config).unwrap_or_default(),
                    );
                }
            }
        }
        println!(
            "  \u{2713} MCP server removed from {}",
            config_path.display()
        );
    } else {
        println!("  Nothing to remove ({} not found)", config_path.display());
    }

    println!();
    println!("  Done. Myceliums integration removed from {}.", platform);
    Ok(())
}

async fn cmd_report(repo: &str, output: Option<&Path>) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;
    let communities = store.get_communities().await?;
    let processes = store.get_processes().await?;
    let files = store.get_files().await?;

    let god_nodes = compute_god_nodes(&symbols, &relationships, 10, 20);
    let surprising = compute_surprising_connections(&symbols, &relationships, 0.1, 50)?;

    let out_path = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("GRAPH_REPORT.md"));

    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let mut md = String::new();

    // ── Header ─────────────────────────────────────────────────────────
    md.push_str(&format!(
        "# Graph Report — {}\n\nGenerated: {}  |  Repo ID: `{}`\n\n",
        repo_info.name, now, repo_info.id
    ));

    // ── Overview ───────────────────────────────────────────────────────
    md.push_str("## Overview\n\n");
    md.push_str(&format!(
        "| Metric | Count |\n|--------|-------|\n\
         | Symbols | {} |\n\
         | Files | {} |\n\
         | Relationships | {} |\n\
         | Communities | {} |\n\
         | Processes | {} |\n\n",
        symbols.len(),
        files.len(),
        relationships.len(),
        communities.len(),
        processes.len(),
    ));

    // ── God Nodes ──────────────────────────────────────────────────────
    md.push_str("## God Nodes (Top 10 by Degree Centrality)\n\n");
    if god_nodes.is_empty() {
        md.push_str(
            "_No function calls detected. Graph structure limited to imports/containment._\n\n",
        );
    } else {
        md.push_str("| # | Name | Kind | Degree | In | Out | File | High Coupling |\n");
        md.push_str("|---|------|------|--------|----|-----|------|---------------|\n");
        for (i, n) in god_nodes.iter().enumerate() {
            md.push_str(&format!(
                "| {} | `{}` | {} | {} | {} | {} | `{}` | {} |\n",
                i + 1,
                n.name,
                n.kind,
                n.degree,
                n.in_degree,
                n.out_degree,
                n.file_path,
                if n.is_high_coupling {
                    "⚠️ yes"
                } else {
                    "no"
                },
            ));
        }
        md.push('\n');
        let high = god_nodes.iter().filter(|n| n.is_high_coupling).count();
        if high > 0 {
            md.push_str(&format!(
                "> **{} high-coupling node(s)** (degree > 20) detected. Consider splitting \
                 responsibilities or introducing abstraction layers.\n\n",
                high
            ));
        }
    }

    // ── Surprising Connections ─────────────────────────────────────────
    md.push_str("## Surprising Cross-Community Connections\n\n");
    if surprising.is_empty() {
        if communities.len() < 2 {
            md.push_str(
                "_All symbols are in a single community — no cross-community edges exist._\n\n",
            );
        } else {
            md.push_str("_No surprising connections found above the minimum threshold._\n\n");
        }
    } else {
        md.push_str(
            "Ranked by surprise score (1.0 = unique link between rarely-interacting communities):\n\n",
        );
        md.push_str("| # | Score | Source | Target | Communities |\n");
        md.push_str("|---|-------|--------|--------|-------------|\n");
        for (i, c) in surprising.iter().enumerate() {
            md.push_str(&format!(
                "| {} | {:.3} | `{}` | `{}` | {} → {} |\n",
                i + 1,
                c.surprise_score,
                c.source_name,
                c.target_name,
                c.source_community,
                c.target_community,
            ));
        }
        md.push('\n');
    }

    // ── Community Summary ──────────────────────────────────────────────
    md.push_str("## Community Summary\n\n");
    if communities.is_empty() {
        md.push_str("_No communities detected. Re-run `myc analyze` to generate them._\n\n");
    } else {
        md.push_str("| # | Label | Members | Top Symbols | Summary |\n");
        md.push_str("|---|-------|---------|-------------|----------|\n");
        for (i, c) in communities.iter().enumerate() {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                i + 1,
                c.label,
                c.member_count,
                c.top_symbols.replace('|', "\\|"),
                c.summary.replace('|', "\\|"),
            ));
        }
        md.push('\n');
    }

    // ── Suggested Queries ──────────────────────────────────────────────
    md.push_str("## Suggested Cypher Queries\n\n");
    md.push_str("Use `myc query` or the `cypher_query` MCP tool with these queries:\n\n");
    md.push_str("```cypher\n// Find all callers of a god node\nMATCH (caller)-[:CALLS]->(target {name: \"<god_node_name>\"})\nRETURN caller.name, caller.file_path\nORDER BY caller.name\n```\n\n");
    md.push_str("```cypher\n// List functions with the most outgoing calls\nMATCH (s)-[:CALLS]->()\nRETURN s.name, s.kind, s.file_path, count(*) AS out_degree\nORDER BY out_degree DESC\nLIMIT 20\n```\n\n");
    md.push_str("```cypher\n// Find all symbols in a specific file\nMATCH (s {file_path: \"<path/to/file>\"})\nRETURN s.name, s.kind, s.start_line\nORDER BY s.start_line\n```\n\n");
    md.push_str("```cypher\n// Find dead code (functions with no callers)\nMATCH (s)\nWHERE s.kind = \"Function\"\nAND NOT ()-[:CALLS]->(s)\nRETURN s.name, s.file_path\nLIMIT 50\n```\n");

    std::fs::write(&out_path, &md)
        .with_context(|| format!("Failed to write report to {}", out_path.display()))?;

    println!("Report written to {}", out_path.display());
    println!(
        "  {} symbols, {} communities, {} god nodes, {} surprising connections",
        symbols.len(),
        communities.len(),
        god_nodes.len(),
        surprising.len(),
    );

    Ok(())
}

async fn cmd_serve(port: u16, repo: Option<String>) -> Result<()> {
    // Resolve repo_id: explicit flag > auto-detect from cwd > None (show all repos)
    let repo_id = if let Some(r) = repo {
        let (id, _) = resolve_repo(&r)?;
        Some(id)
    } else {
        // Try to auto-detect from current working directory
        let cwd = std::env::current_dir()?;
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let candidate_id = analyzer::repo_id_from_path(&cwd);
        let registry = RepoRegistry::load(&registry_path())?;
        if registry.get(&candidate_id).is_some() {
            Some(candidate_id)
        } else {
            None
        }
    };

    if let Some(ref id) = repo_id {
        println!("Serving repo: {}", id);
    }
    println!("Visualization ready at http://localhost:{}", port);

    myceliums_http::start_server(port, repo_id).await
}

fn cmd_setup_claude_uninstall(home: &std::path::Path, myc_path: &str) -> Result<()> {
    println!("Removing Claude Code integration...");
    println!();

    // 1. Remove MCP server
    let mcp_result = std::process::Command::new("claude")
        .args(["mcp", "remove", "-s", "user", "myceliums"])
        .output();
    match mcp_result {
        Ok(r) if r.status.success() => println!("  \u{2713} MCP server removed"),
        _ => {
            // Manual removal from .claude.json
            let config_path = home.join(".claude.json");
            if config_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&config_path) {
                    if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(servers) =
                            config.get_mut("mcpServers").and_then(|s| s.as_object_mut())
                        {
                            servers.remove("myceliums");
                            let _ = std::fs::write(
                                &config_path,
                                serde_json::to_string_pretty(&config).unwrap_or_default(),
                            );
                        }
                    }
                }
            }
            println!("  \u{2713} MCP server removed");
        }
    }

    // 2. Remove hooks
    let settings_path = home.join(".claude").join("settings.json");
    if settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                    for event in ["SessionStart", "PostToolUse"] {
                        if let Some(arr) = hooks.get_mut(event).and_then(|a| a.as_array_mut()) {
                            arr.retain(|entry| {
                                let is_myceliums = entry["hooks"]
                                    .as_array()
                                    .map(|h| {
                                        h.iter().any(|hook| {
                                            hook["command"]
                                                .as_str()
                                                .map(|c| c.contains("myc ") || c.contains(myc_path))
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false);
                                !is_myceliums
                            });
                        }
                    }
                    // Clean up empty hook arrays
                    let empty_events: Vec<String> = hooks
                        .iter()
                        .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
                        .map(|(k, _)| k.clone())
                        .collect();
                    for event in empty_events {
                        hooks.remove(&event);
                    }
                }
                let _ = std::fs::write(
                    &settings_path,
                    serde_json::to_string_pretty(&settings).unwrap_or_default(),
                );
            }
        }
    }
    println!("  \u{2713} Hooks removed");

    println!();
    println!("  Done. Myceliums integration removed from Claude Code.");
    Ok(())
}

const HOOK_BEGIN: &str = "# BEGIN myceliums";
const HOOK_END: &str = "# END myceliums";

fn post_commit_script(myc_path: &str) -> String {
    format!(
        "# BEGIN myceliums\n\
         _myc_changed=$(git diff --name-only HEAD~1 2>/dev/null)\n\
         if [ -n \"$_myc_changed\" ]; then\n\
             {} analyze . --skip-embeddings >/dev/null 2>&1 &\n\
         fi\n\
         # END myceliums",
        myc_path
    )
}

fn post_checkout_script(myc_path: &str) -> String {
    format!(
        "# BEGIN myceliums\n\
         if [ \"$3\" = \"1\" ]; then\n\
             {} analyze . >/dev/null 2>&1 &\n\
         fi\n\
         # END myceliums",
        myc_path
    )
}

fn find_git_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            return Ok(git);
        }
        dir = dir
            .parent()
            .context("Not inside a git repository (no .git directory found)")?;
    }
}

fn install_hook(hook_path: &Path, script_block: &str) -> Result<()> {
    if hook_path.exists() {
        let existing = std::fs::read_to_string(hook_path)?;
        let new_content = if existing.contains(HOOK_BEGIN) {
            replace_hook_block(&existing, script_block)
        } else {
            let sep = if existing.ends_with('\n') { "" } else { "\n" };
            format!("{}{}{}\n", existing, sep, script_block)
        };
        std::fs::write(hook_path, new_content)?;
    } else {
        std::fs::write(hook_path, format!("#!/bin/sh\n{}\n", script_block))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(hook_path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(hook_path, perms)?;
    }

    Ok(())
}

fn replace_hook_block(existing: &str, new_block: &str) -> String {
    if let (Some(start), Some(end_pos)) = (existing.find(HOOK_BEGIN), existing.find(HOOK_END)) {
        let end = end_pos + HOOK_END.len();
        format!("{}{}{}", &existing[..start], new_block, &existing[end..])
    } else {
        format!("{}\n{}\n", existing, new_block)
    }
}

fn remove_hook(hook_path: &Path) -> Result<()> {
    if !hook_path.exists() {
        return Ok(());
    }
    let existing = std::fs::read_to_string(hook_path)?;
    if !existing.contains(HOOK_BEGIN) {
        return Ok(());
    }
    let new_content = remove_hook_block(&existing);
    let trimmed = new_content.trim();
    if trimmed.is_empty() || trimmed == "#!/bin/sh" {
        std::fs::remove_file(hook_path)?;
    } else {
        std::fs::write(hook_path, new_content)?;
    }
    Ok(())
}

fn remove_hook_block(content: &str) -> String {
    if let (Some(start), Some(end_pos)) = (content.find(HOOK_BEGIN), content.find(HOOK_END)) {
        let end = end_pos + HOOK_END.len();
        let before = content[..start].trim_end_matches('\n');
        let after = content[end..].trim_start_matches('\n');
        if before.is_empty() {
            format!("{}\n", after)
        } else if after.is_empty() {
            format!("{}\n", before)
        } else {
            format!("{}\n{}\n", before, after)
        }
    } else {
        content.to_string()
    }
}

fn cmd_hook_install() -> Result<()> {
    let git_dir = find_git_dir()?;
    let hooks_dir = git_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let myc_path = std::env::current_exe()
        .context("Could not determine myc binary path")?
        .to_string_lossy()
        .to_string();

    install_hook(
        &hooks_dir.join("post-commit"),
        &post_commit_script(&myc_path),
    )?;
    install_hook(
        &hooks_dir.join("post-checkout"),
        &post_checkout_script(&myc_path),
    )?;

    println!("  \u{2713} post-commit hook installed");
    println!("  \u{2713} post-checkout hook installed");
    println!();
    println!(
        "  The knowledge graph will rebuild automatically after each commit or branch switch."
    );
    println!("  To remove: myc hook uninstall");
    Ok(())
}

fn cmd_hook_uninstall() -> Result<()> {
    let git_dir = find_git_dir()?;
    let hooks_dir = git_dir.join("hooks");

    remove_hook(&hooks_dir.join("post-commit"))?;
    remove_hook(&hooks_dir.join("post-checkout"))?;

    println!("  \u{2713} Myceliums git hooks removed.");
    Ok(())
}

async fn cmd_path(from: &str, to: &str, repo: Option<&str>, max_depth: u32) -> Result<()> {
    let (repo_id, _info) = resolve_repo_or_latest(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
    let store = Store::open(&db_path, &repo_id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    // Resolve symbols using same pattern as MCP tool
    let from_symbol = resolve_symbol_cli(&symbols, from)?;
    let to_symbol = resolve_symbol_cli(&symbols, to)?;

    // Build bidirectional adjacency list across ALL relationship types
    let mut adjacency: std::collections::HashMap<&str, Vec<(&str, String)>> =
        std::collections::HashMap::new();
    for rel in &relationships {
        let edge_label = rel.kind.to_string();
        adjacency
            .entry(rel.source_uid.as_str())
            .or_default()
            .push((rel.target_uid.as_str(), edge_label.clone()));
        adjacency
            .entry(rel.target_uid.as_str())
            .or_default()
            .push((rel.source_uid.as_str(), format!("{}_REV", edge_label)));
    }

    let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // BFS
    let mut visited = std::collections::HashSet::new();
    let mut parent: std::collections::HashMap<&str, (&str, String)> =
        std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back((from_symbol.uid.as_str(), 0u32));
    visited.insert(from_symbol.uid.as_str());

    let mut found = false;
    while let Some((uid, depth)) = queue.pop_front() {
        if uid == to_symbol.uid.as_str() {
            found = true;
            break;
        }
        if depth >= max_depth {
            continue;
        }
        if let Some(neighbors) = adjacency.get(uid) {
            for (neighbor_uid, edge_type) in neighbors {
                if !visited.contains(neighbor_uid) {
                    visited.insert(neighbor_uid);
                    parent.insert(neighbor_uid, (uid, edge_type.clone()));
                    queue.push_back((neighbor_uid, depth + 1));
                }
            }
        }
    }

    if !found {
        println!(
            "No path found between '{}' and '{}' within {} hops.",
            from, to, max_depth
        );
        return Ok(());
    }

    // Reconstruct path
    let mut path_uids: Vec<(&str, String)> = Vec::new();
    let mut current = to_symbol.uid.as_str();
    while current != from_symbol.uid.as_str() {
        if let Some((prev, edge)) = parent.get(current) {
            path_uids.push((current, edge.clone()));
            current = prev;
        } else {
            break;
        }
    }
    path_uids.reverse();

    println!(
        "Shortest path from '{}' to '{}' ({} hops):\n",
        from_symbol.name,
        to_symbol.name,
        path_uids.len()
    );
    println!(
        " [start] {} ({}) {}:{}",
        from_symbol.name, from_symbol.kind, from_symbol.file_path, from_symbol.start_line,
    );
    for (i, (uid, edge)) in path_uids.iter().enumerate() {
        if let Some(sym) = sym_by_uid.get(uid) {
            println!("    --[{}]-->", edge);
            println!(
                " [{}] {} ({}) {}:{}",
                i + 1,
                sym.name,
                sym.kind,
                sym.file_path,
                sym.start_line,
            );
        }
    }

    Ok(())
}

fn resolve_symbol_cli<'a>(
    symbols: &'a [myceliums_storage::CodeSymbol],
    name: &str,
) -> Result<&'a myceliums_storage::CodeSymbol> {
    let matches: Vec<_> = symbols
        .iter()
        .filter(|s| s.qualified_name == name)
        .collect();
    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    if matches.is_empty() {
        let by_name: Vec<_> = symbols.iter().filter(|s| s.name == name).collect();
        match by_name.len() {
            0 => anyhow::bail!("Symbol not found: {}", name),
            1 => Ok(by_name[0]),
            _ => {
                let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                anyhow::bail!("Ambiguous symbol '{}'. Matches: {}", name, names.join(", "));
            }
        }
    } else {
        Ok(matches[0])
    }
}

async fn cmd_wiki(repo: &str, output: &Path, format: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;
    let communities = store.get_communities().await?;

    let config = WikiExportConfig {
        obsidian_vault: format == Some("obsidian"),
        ..Default::default()
    };

    let result = export_wiki(
        &symbols,
        &relationships,
        &communities,
        &repo_info.name,
        output,
        &config,
    )?;

    eprintln!(
        "Exported {} communities, {} symbols, {} relationships to {}",
        result.community_count,
        result.symbol_count,
        result.relationship_count,
        output.display()
    );

    Ok(())
}

async fn cmd_diff(repo: &str) -> Result<()> {
    let (repo_id, _repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

    // Load the previous snapshot
    let previous = myceliums_core::load_snapshot(&data_dir(), &repo_id)?;
    let previous = match previous {
        Some(s) => s,
        None => {
            println!(
                "No previous snapshot found for '{}'.\n\
                 Run `myc analyze <path>` first to create a baseline snapshot.",
                repo_id
            );
            return Ok(());
        }
    };

    // Build a snapshot from the current graph state
    let store = Store::open(&db_path, &repo_id).await?;
    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;
    let current = myceliums_core::build_snapshot(&repo_id, &symbols, &relationships);

    // Diff
    let diff = myceliums_core::diff_snapshots(&previous, &current);

    // Print
    let total_changes = diff.added_symbols.len()
        + diff.removed_symbols.len()
        + diff.added_edges.len()
        + diff.removed_edges.len();

    if total_changes == 0 {
        println!(
            "No changes detected since last snapshot ({}).",
            diff.previous_snapshot_at
        );
        return Ok(());
    }

    println!(
        "Graph diff for '{}' (snapshot {} -> now):\n",
        repo_id, diff.previous_snapshot_at
    );

    if !diff.added_symbols.is_empty() {
        println!("+ Added symbols ({}):", diff.added_symbols.len());
        for entry in &diff.added_symbols {
            println!("    + {}", entry.label);
        }
        println!();
    }

    if !diff.removed_symbols.is_empty() {
        println!("- Removed symbols ({}):", diff.removed_symbols.len());
        for entry in &diff.removed_symbols {
            println!("    - {}", entry.label);
        }
        println!();
    }

    if !diff.added_edges.is_empty() {
        println!("+ Added relationships ({}):", diff.added_edges.len());
        for entry in &diff.added_edges {
            println!("    + {}", entry.label);
        }
        println!();
    }

    if !diff.removed_edges.is_empty() {
        println!("- Removed relationships ({}):", diff.removed_edges.len());
        for entry in &diff.removed_edges {
            println!("    - {}", entry.label);
        }
        println!();
    }

    println!(
        "Summary: +{} -{} symbols, +{} -{} relationships",
        diff.added_symbols.len(),
        diff.removed_symbols.len(),
        diff.added_edges.len(),
        diff.removed_edges.len(),
    );

    Ok(())
}

// ── Email commands ───────────────────────────────────────────────────

fn imap_config_dir() -> PathBuf {
    data_dir().join("imap_config")
}

async fn cmd_email_connect(host: &str, user: &str, port: u16) -> Result<()> {
    use myceliums_core::imap::{self, ImapConfig};

    // Prompt for password
    eprint!("Password for {}@{}: ", user, host);
    std::io::stderr().flush()?;
    let password = rpassword::read_password().context("Failed to read password")?;

    let config = ImapConfig {
        host: host.to_string(),
        port,
        username: user.to_string(),
        password: password.clone(),
        use_tls: true,
        folders: vec!["INBOX".to_string()],
    };

    // Test connection
    println!("Testing connection to {}:{}...", host, port);
    let mut session = imap::connect(&config).await?;
    println!("Connected successfully!");
    session
        .logout()
        .await
        .map_err(|e| anyhow::anyhow!("logout failed: {}", e))?;

    // Save config
    let account_id = user.replace('@', "_at_").replace('.', "_");
    let config_dir = imap_config_dir();
    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join(format!("{}.json", account_id));
    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, json)?;

    println!(
        "Saved IMAP config for account '{}' to {}",
        account_id,
        config_path.display()
    );
    println!(
        "Run `myc email sync --account {}` to fetch emails.",
        account_id
    );
    Ok(())
}

async fn cmd_email_sync(account: Option<&str>) -> Result<()> {
    use myceliums_core::imap::{self, ImapConfig};

    let config_dir = imap_config_dir();
    if !config_dir.exists() {
        anyhow::bail!("No email accounts configured. Run `myc email connect` first.");
    }

    // Collect account configs to sync
    let configs: Vec<(String, ImapConfig)> = if let Some(acct) = account {
        let path = config_dir.join(format!("{}.json", acct));
        if !path.exists() {
            anyhow::bail!(
                "Account '{}' not found. Run `myc email connect` first.",
                acct
            );
        }
        let data = std::fs::read_to_string(&path)?;
        let cfg: ImapConfig = serde_json::from_str(&data)?;
        vec![(acct.to_string(), cfg)]
    } else {
        let mut all = Vec::new();
        for entry in std::fs::read_dir(&config_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let acct_id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let data = std::fs::read_to_string(&path)?;
                let cfg: ImapConfig = serde_json::from_str(&data)?;
                all.push((acct_id, cfg));
            }
        }
        if all.is_empty() {
            anyhow::bail!("No email accounts configured. Run `myc email connect` first.");
        }
        all
    };

    let state_dir = data_dir();
    for (acct_id, config) in &configs {
        println!("Syncing account '{}'...", acct_id);
        let mut session = imap::connect(config).await?;
        let mut state = imap::load_sync_state(&state_dir, acct_id)?;

        let mut total_emails = 0usize;
        for folder in &config.folders {
            let emails = imap::fetch_new_emails(&mut session, &mut state, folder).await?;
            println!("  {} — {} new emails", folder, emails.len());
            total_emails += emails.len();

            // Run emails through the analyzer pipeline
            if !emails.is_empty() {
                let repo_id = format!("email-{}", acct_id);
                let db_path = myceliums_storage::RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                let store = myceliums_storage::Store::open(&db_path, &repo_id).await?;

                // Write each email to a temp file and analyze
                let temp_dir = std::env::temp_dir().join("myceliums-email-sync");
                std::fs::create_dir_all(&temp_dir)?;
                let mut paths = Vec::new();
                for (i, email) in emails.iter().enumerate() {
                    let eml_path = temp_dir.join(format!("{}_{}.eml", acct_id, i));
                    // Reconstruct a minimal EML for the analyzer
                    let eml_content = format!(
                        "From: {}\r\nTo: {}\r\nSubject: {}\r\nMessage-ID: <{}>\r\n{}{}\r\n\r\n{}",
                        email.from,
                        email.to.join(", "),
                        email.subject,
                        email.message_id,
                        email
                            .in_reply_to
                            .as_ref()
                            .map(|r| format!("In-Reply-To: <{}>\r\n", r))
                            .unwrap_or_default(),
                        email
                            .date
                            .as_ref()
                            .map(|d| format!("Date: {}\r\n", d))
                            .unwrap_or_default(),
                        email.body,
                    );
                    std::fs::write(&eml_path, eml_content)?;
                    paths.push(eml_path);
                }

                let analyzer = Analyzer::new(store, temp_dir.clone()).set_skip_embeddings(true);
                let result = analyzer.analyze_files(&paths).await?;
                println!(
                    "  Indexed: {} symbols, {} relationships",
                    result.symbol_count, result.relationship_count
                );

                // Register in repo registry
                let mut registry = myceliums_storage::RepoRegistry::load(&registry_path())?;
                registry.register(myceliums_storage::RepoInfo {
                    id: repo_id.clone(),
                    name: format!("Email: {}", acct_id),
                    path: temp_dir.to_string_lossy().to_string(),
                    analyzed_at: chrono::Utc::now().to_rfc3339(),
                    symbol_count: result.symbol_count as u32,
                    file_count: result.file_count as u32,
                    analyzed_commit: None,
                });
                registry.save()?;

                // Clean up temp files
                let _ = std::fs::remove_dir_all(&temp_dir);
            }
        }

        imap::save_sync_state(&state_dir, &state)?;
        session
            .logout()
            .await
            .map_err(|e| anyhow::anyhow!("logout failed: {}", e))?;
        println!(
            "Done — {} total new emails synced for '{}'",
            total_emails, acct_id
        );
    }

    Ok(())
}

async fn cmd_email_disconnect(account: &str) -> Result<()> {
    let config_path = imap_config_dir().join(format!("{}.json", account));
    if !config_path.exists() {
        anyhow::bail!("Account '{}' not found.", account);
    }
    std::fs::remove_file(&config_path)?;
    println!("Removed IMAP config for account '{}'.", account);

    // Also remove sync state if it exists
    let state_path = data_dir()
        .join("imap_state")
        .join(format!("{}.json", account));
    if state_path.exists() {
        std::fs::remove_file(&state_path)?;
        println!("Removed sync state for account '{}'.", account);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_safeguard_rejects_home_directory() {
        let home = dirs::home_dir().expect("need home dir for test");
        let result = check_session_safeguards(&home, false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("home directory"),
            "Expected home directory error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_safeguard_rejects_non_git_directory() {
        let tmp = TempDir::new().unwrap();
        let result = check_session_safeguards(tmp.path(), false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".git"),
            "Expected git error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_safeguard_allows_non_git_with_override() {
        let tmp = TempDir::new().unwrap();
        let result = check_session_safeguards(tmp.path(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_safeguard_allows_git_directory() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let result = check_session_safeguards(tmp.path(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_dangerous_ancestor() {
        // Test known dangerous ancestors
        assert!(is_dangerous_ancestor(Path::new("/")));
        assert!(is_dangerous_ancestor(Path::new("/Users")));
        assert!(is_dangerous_ancestor(Path::new("/home")));

        // Test that regular paths are not dangerous
        assert!(!is_dangerous_ancestor(Path::new("/Users/marc")));
        assert!(!is_dangerous_ancestor(Path::new("/home/user")));
        assert!(!is_dangerous_ancestor(Path::new("/var/tmp")));
    }

    #[test]
    fn test_is_home_ancestor() {
        // Create temporary paths for testing
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();

        // Parent should be detected as ancestor
        if let Some(parent) = home.parent() {
            assert!(is_home_ancestor(parent, home));
        }

        // Home itself should not be ancestor of itself
        assert!(!is_home_ancestor(home, home));

        // Non-parent should not be ancestor
        let other = TempDir::new().unwrap();
        assert!(!is_home_ancestor(other.path(), home));
    }

    #[test]
    fn test_safeguard_nonexistent_path_fails() {
        let result = check_session_safeguards(Path::new("/nonexistent/path/abcdef"), false);
        // Should fail (fail closed on canonicalize error)
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("canonicalize"),
            "Expected canonicalize error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello...");
        assert_eq!(truncate_str("hi", 2), "hi");
    }

    #[test]
    fn test_find_orphans_runs() {
        // Verify it doesn't panic
        let result = find_orphans();
        assert!(result.is_ok());
    }

    #[test]
    fn test_dir_size_bytes_empty() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 0);
    }

    #[test]
    fn test_dir_size_bytes_with_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "world!").unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 11); // 5 + 6
    }
}
