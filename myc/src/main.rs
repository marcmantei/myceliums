use crate::cli::{Cli, Commands, EmailCommands, HookAction, LogFormat};
use crate::commands::analyze::{check_session_safeguards, cmd_analyze, cmd_watch, EmbeddingPolicy};
use crate::commands::communities::cmd_communities;
use crate::commands::configure::cmd_configure;
use crate::commands::diff::cmd_diff;
use crate::commands::doctor::cmd_doctor;
use crate::commands::email::{cmd_email_connect, cmd_email_disconnect, cmd_email_sync};
use crate::commands::export::cmd_export;
use crate::commands::format_hook::cmd_format_hook;
use crate::commands::git_hook::{cmd_hook_install, cmd_hook_uninstall};
use crate::commands::impact::cmd_impact;
use crate::commands::init::cmd_init;
use crate::commands::knowledge::cmd_knowledge;
use crate::commands::path::cmd_path;
use crate::commands::processes::cmd_processes;
use crate::commands::query::cmd_query;
use crate::commands::rename::cmd_rename;
use crate::commands::report::cmd_report;
use crate::commands::repos::{cmd_clean, cmd_delete, cmd_list, cmd_status};
use crate::commands::search::{cmd_search, cmd_semantic_search};
use crate::commands::serve::cmd_serve;
use crate::commands::session::cmd_session;
use crate::commands::setup_editors::{
    cmd_setup, cmd_setup_codex, cmd_setup_editor, cmd_setup_mcp_platform,
};
use crate::commands::stats::cmd_stats;
use crate::commands::wiki::cmd_wiki;
use crate::repo::data_dir;
use crate::ui::prompt_yes_no;
use anyhow::Result;
use clap::Parser;
use myceliums_core::GlobalConfig;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

mod cli;
mod commands;
mod progress;
mod repo;
mod setup;
mod ui;

/// The default log filter when `RUST_LOG` is unset. The MCP server logs one
/// INFO line per tool call (and its own lifecycle) and the core's warnings
/// (skipped files, index problems); everything else, and every other
/// command, prints errors only. Logs go to stderr: stdout is the
/// stdio MCP transport.
fn default_log_filter(command: &Commands) -> &'static str {
    match command {
        Commands::Mcp { .. } => "error,myceliums_core=warn,myceliums_mcp=info",
        _ => "error",
    }
}

fn init_tracing(command: &Commands) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_log_filter(command)));
    let json = matches!(
        command,
        Commands::Mcp {
            log_format: LogFormat::Json,
            ..
        }
    );
    // MCP clients usually write the server's stderr to a log file: colour
    // codes only on a terminal.
    let fmt = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr);
    if json {
        fmt.json()
            .with_current_span(true)
            .with_span_list(false)
            .init();
    } else {
        fmt.init();
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.command);
    // The grep hook runs once per tool call and must answer fast: a
    // single-threaded runtime starts without spawning a worker per core.
    let mut runtime = if matches!(cli.command, Commands::HookGrep) {
        tokio::runtime::Builder::new_current_thread()
    } else {
        tokio::runtime::Builder::new_multi_thread()
    };
    let runtime = runtime.enable_all().build()?;
    let result = runtime.block_on(run(cli));
    // A background embedding of the MCP server may still be ending on a
    // blocking thread: it was asked to stop when serving ended, and stops
    // within a model batch. Never wait for it longer than this (#157).
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    result.map_err(repo::hint_for_missing_index)
}

/// How long the process waits, after its work, for blocking threads still
/// running (a background embedding finishing its batch) before it exits.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

async fn run(cli: Cli) -> Result<()> {
    // Restore or delete what interrupted index rebuilds of dead processes
    // left in the data directory. Skipped by the grep hook, which must
    // answer fast and only reads.
    if !matches!(cli.command, Commands::HookGrep) {
        myceliums_core::recover_leftovers(&data_dir().join("data"));
    }
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
                if let Err(e) = setup::wizard::run_wizard(&data_dir()).await {
                    eprintln!("  error: setup wizard failed: {:#}", e);
                }
            } else {
                // best-effort: create data dir so we don't prompt again; a failure
                // here only means we re-ask on the next run, not lost user data.
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
            strict_embeddings,
            skip_communities,
            watch,
            no_git_check,
        } => {
            check_session_safeguards(&path, no_git_check)?;
            let embedding_policy = EmbeddingPolicy::from_flags(skip_embeddings, strict_embeddings);
            cmd_analyze(&path, force, max_age, embedding_policy, skip_communities).await?;
            if watch {
                cmd_watch(&path, skip_embeddings, skip_communities).await?;
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
            tests,
        } => {
            cmd_search(
                &query,
                repo.as_deref(),
                limit,
                hybrid,
                rerank,
                explain,
                tests.into(),
            )
            .await
        }
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
        Commands::Impact {
            target,
            repo,
            depth,
            diff,
            json,
            budget,
        } => match target {
            Some(target) => {
                let opts = commands::nav::OutputOptions { json, budget };
                commands::nav::cmd_impact_target(&target, depth, repo.as_deref(), opts).await
            }
            None => cmd_impact(repo.as_deref(), depth, diff.as_deref(), json).await,
        },
        Commands::Callers {
            symbol,
            depth,
            repo,
            json,
            budget,
        } => {
            let opts = commands::nav::OutputOptions { json, budget };
            let dir = myceliums_core::navigation::CallDirection::Callers;
            commands::nav::cmd_calls(&symbol, dir, depth, repo.as_deref(), opts).await
        }
        Commands::Callees {
            symbol,
            depth,
            repo,
            json,
            budget,
        } => {
            let opts = commands::nav::OutputOptions { json, budget };
            let dir = myceliums_core::navigation::CallDirection::Callees;
            commands::nav::cmd_calls(&symbol, dir, depth, repo.as_deref(), opts).await
        }
        Commands::Context {
            query,
            repo,
            hybrid,
            json,
            budget,
        } => {
            let opts = commands::nav::OutputOptions { json, budget };
            commands::nav::cmd_context(&query.join(" "), repo.as_deref(), hybrid, opts).await
        }
        Commands::SemanticSearch {
            query,
            repo,
            limit,
            tests,
        } => cmd_semantic_search(&query, repo.as_deref(), limit, tests.into()).await,
        Commands::Session {
            path,
            yes,
            timeout: _,
            strict_embeddings,
            no_git_check,
        } => {
            let p = path.unwrap_or_else(|| PathBuf::from("."));
            cmd_session(&p, yes, strict_embeddings, no_git_check).await
        }
        Commands::Status => cmd_status().await,
        Commands::Clean {
            repo,
            orphans,
            all,
            cache,
            yes,
        } => cmd_clean(repo, orphans, all, cache, yes).await,
        Commands::Mcp {
            http,
            allow_remote,
            toolsets,
            log_format: _,
            stats_file,
        } => {
            let opts = myceliums_mcp::ServeOptions {
                toolsets: mcp_toolsets(toolsets.as_deref())?,
                stats_file,
            };
            if let Some(addr) = http {
                myceliums_mcp::run_mcp_http_server(
                    myceliums_mcp::McpHttpOptions {
                        addr,
                        allow_remote,
                        token: std::env::var(myceliums_mcp::http_security::TOKEN_ENV).ok(),
                    },
                    opts,
                )
                .await
            } else {
                myceliums_mcp::run_mcp_server(opts).await
            }
        }
        Commands::Doctor { download, fix } => cmd_doctor(download, fix).await,
        Commands::Configure { set, reset } => cmd_configure(set.as_deref(), reset).await,
        Commands::FormatHook => cmd_format_hook().await,
        Commands::HookGrep => commands::hook_grep::cmd_hook_grep().await,
        Commands::SetupClaude {
            scope,
            project,
            no_routing,
            no_redirect_hook,
            no_agent,
            no_always_load,
            uninstall,
        } => {
            let layers = commands::setup_claude::Layers {
                routing: !no_routing,
                redirect_hook: !no_redirect_hook,
                agent: !no_agent,
                tool_loading: commands::setup_claude::ToolLoading::for_config(no_always_load),
            };
            commands::setup_claude::cmd_setup_claude(scope, project.as_deref(), layers, uninstall)
        }
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
        Commands::SetupCodex {
            uninstall,
            no_routing,
        } => cmd_setup_codex(uninstall, no_routing).await,
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

/// Check if a command is interactive (should trigger first-run prompt).
/// Non-interactive commands (MCP server, hooks, auto-session) should never prompt.
fn is_interactive_command(cmd: &Commands) -> bool {
    !matches!(
        cmd,
        Commands::Mcp { .. }
            | Commands::FormatHook
            | Commands::HookGrep
            | Commands::Session { yes: true, .. }
    )
}

// Path-safety guards: refuse to analyze the home directory (would index
// hundreds of thousands of files) and non-git directories (unless
// `--no-git-check` is passed).

/// The toolsets `myc mcp` exposes: `--toolsets`, else the `mcp.toolsets`
/// config key, else core. An unreadable config falls back to core (with a
/// warning on stderr, which is not the protocol channel); an unknown toolset
/// name is an error.
fn mcp_toolsets(flag: Option<&str>) -> Result<myceliums_mcp::Toolsets> {
    let names: Vec<String> = match flag {
        Some(list) => list.split(',').map(str::to_string).collect(),
        None => {
            match GlobalConfig::load(&data_dir()) {
                Ok(config) => config.mcp.toolsets,
                Err(e) => {
                    eprintln!("warning: cannot read the myceliums config ({e:#}); serving the core toolset");
                    Vec::new()
                }
            }
        }
    };
    myceliums_mcp::Toolsets::parse_names(names.iter().map(String::as_str))
        .map_err(|e| anyhow::anyhow!("--toolsets / mcp.toolsets: {e}"))
}
