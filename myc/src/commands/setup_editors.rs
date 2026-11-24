//! `myc setup-editors`.

use crate::commands;
use crate::repo::data_dir;
use crate::setup;
use anyhow::{Context, Result};

/// Unified setup command that auto-detects or configures specific editors
pub(crate) async fn cmd_setup(editor: Option<String>, uninstall: bool) -> Result<()> {
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
pub(crate) async fn cmd_setup_editor(editor_name: &str, uninstall: bool) -> Result<()> {
    let myc_bin = std::env::current_exe().context("Could not determine myc binary path")?;
    let myc_path = myc_bin.to_string_lossy().to_string();

    if uninstall {
        setup::SetupOrchestrator::uninstall_editor(editor_name)
    } else {
        setup::SetupOrchestrator::setup_editor(editor_name, &myc_path)
    }
}

/// `myc setup-codex`: the Codex MCP config, plus the routing block in the
/// project's AGENTS.md when run inside a git repository (#126).
pub(crate) async fn cmd_setup_codex(uninstall: bool, no_routing: bool) -> Result<()> {
    cmd_setup_editor("codex", uninstall).await?;
    let root = std::env::current_dir()
        .ok()
        .and_then(|cwd| commands::setup_claude::git_toplevel(&cwd));
    let Some(root) = root else {
        if !uninstall && !no_routing {
            println!("  Not inside a git repository: no AGENTS.md routing block written.");
        }
        return Ok(());
    };
    // A home directory under version control is not a project: never write
    // ~/AGENTS.md (the same guard as `myc setup-claude`).
    let is_home = dirs::home_dir()
        .and_then(|h| std::fs::canonicalize(h).ok())
        .is_some_and(|h| std::fs::canonicalize(&root).is_ok_and(|r| r == h));
    if is_home {
        if !uninstall && !no_routing {
            println!(
                "  {} is your home directory: no AGENTS.md routing block written.",
                root.display()
            );
        }
        return Ok(());
    }
    let agents_md = root.join("AGENTS.md");
    use commands::instructions::{self, CodexRouting};
    use commands::routing::FileChange;
    let home = dirs::home_dir();
    let change = if uninstall || no_routing {
        instructions::uninstall_codex_routing(&root, home.as_deref())?
    } else {
        match instructions::install_codex_routing(&root, home.as_deref())? {
            CodexRouting::Done(change) => change,
            CodexRouting::WouldHide { hidden, removed } => {
                let files = instructions::names(&root, &hidden);
                let first = instructions::names(&root, &hidden[..1]);
                if removed {
                    println!(
                        "  \u{2713} Routing block removed: {} held nothing else and hid {files}",
                        agents_md.display()
                    );
                }
                println!(
                    "  No AGENTS.md routing block written. A new AGENTS.md would hide {files} from agents that\n  read one instruction file per directory (Zed, OpenCode, Codex with fallback names).\n  Codex therefore reads no instructions in this project; its myceliums MCP tools still work.\n  Your choice:\n    - let every agent read {first}, with the routing block added to it:\n        ln -s {first} AGENTS.md && myc setup-codex\n    - separate instructions for Codex, which Zed and OpenCode then read instead of {first}:\n        create AGENTS.md with your text, then myc setup-codex\n    - Codex reads {first} without the routing block, only for you:\n        project_doc_fallback_filenames = [\"{first}\"] in ~/.codex/config.toml\n    - leave it as it is"
                );
                return Ok(());
            }
        }
    };
    let memory = root.join("CLAUDE.md");
    if change != FileChange::Absent && same_file::is_same_file(&memory, &agents_md).unwrap_or(false)
    {
        println!(
            "  {} and CLAUDE.md are the same file, which Claude Code reads too. If you use\n  `myc setup-claude` here, re-run it so that Claude Code gets the block once.",
            agents_md.display()
        );
    }
    let what = match change {
        FileChange::Created => "written to new file",
        FileChange::Updated => "written to",
        FileChange::Unchanged => "up to date in",
        FileChange::Removed => "removed from",
        FileChange::Deleted => "removed; deleted the now empty",
        FileChange::Absent => "not present in",
    };
    println!("  \u{2713} Routing block {what} {}", agents_md.display());
    Ok(())
}

/// Generic MCP config setup for platforms that use a simple JSON config file.
pub(crate) fn cmd_setup_mcp_platform(
    platform: &str,
    config_rel_path: &str,
    uninstall: bool,
) -> Result<()> {
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
                    let serialized = serde_json::to_string_pretty(&config)?;
                    std::fs::write(config_path, serialized)
                        .with_context(|| format!("Failed to write {}", config_path.display()))?;
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
