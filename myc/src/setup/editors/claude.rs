use super::{remove_mcp_servers, write_user_file, EditorSetup};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Claude Code editor setup (special case: includes hooks in addition to MCP)
pub struct ClaudeEditor {
    #[allow(dead_code)]
    pub config_path: PathBuf,
    pub home: PathBuf,
}

impl ClaudeEditor {
    pub fn new(config_path: PathBuf, home: PathBuf) -> Self {
        Self { config_path, home }
    }
}

/// Get Claude config path
pub fn get_claude_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".claude.json"))
}

/// Create a Claude editor setup handler
pub fn create_claude_editor() -> Result<ClaudeEditor> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let config_path = get_claude_config_path()?;
    Ok(ClaudeEditor::new(config_path, home))
}

impl EditorSetup for ClaudeEditor {
    fn setup(&self, myc_path: &str) -> Result<()> {
        println!("Setting up Claude Code integration...");
        println!();

        // 1. Register MCP server via `claude mcp add`
        let mcp_result = setup_mcp_server(myc_path);
        match &mcp_result {
            Ok(_) => println!("  ✓ MCP server registered"),
            Err(e) => println!("  ✗ MCP server: {}", e),
        }

        // 2. Add hooks to ~/.claude/settings.json
        let hooks_result = setup_hooks(&self.home, myc_path);
        match &hooks_result {
            Ok(_) => println!("  ✓ Hooks configured (SessionStart + PostToolUse)"),
            Err(e) => println!("  ✗ Hooks: {}", e),
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

    fn uninstall(&self) -> Result<()> {
        println!("Removing Claude Code integration...");
        println!();

        // 1. Remove MCP server
        let mcp_result = std::process::Command::new("claude")
            .args(["mcp", "remove", "-s", "user", "myceliums"])
            .output();
        match mcp_result {
            Ok(r) if r.status.success() => println!("  ✓ MCP server removed"),
            _ => {
                // Manual removal from .claude.json
                let config_path = self.home.join(".claude.json");
                if config_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&config_path) {
                        if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content)
                        {
                            remove_mcp_servers(&mut config, "mcpServers")?;
                            let serialized = serde_json::to_string_pretty(&config)?;
                            write_user_file(&config_path, &serialized)?;
                        }
                    }
                }
                println!("  ✓ MCP server removed");
            }
        }

        // 2. Remove hooks from settings.json
        let settings_path = self.home.join(".claude").join("settings.json");
        if settings_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&settings_path) {
                if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(hooks) = settings
                        .as_object_mut()
                        .and_then(|obj| obj.get_mut("hooks"))
                        .and_then(|h| h.as_object_mut())
                    {
                        remove_hooks(hooks);
                        let serialized = serde_json::to_string_pretty(&settings)?;
                        write_user_file(&settings_path, &serialized)?;
                    }
                }
            }
        }
        println!("  ✓ Hooks removed");

        println!();
        println!("  Done. Myceliums integration removed from Claude Code.");
        Ok(())
    }

    fn config_path(&self) -> &PathBuf {
        &self.config_path
    }
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
                // best-effort: remove the stale server before re-adding; the retry
                // below is the authoritative operation whose result we check.
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
        .entry("mcpServers".to_string())
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
        .entry("hooks".to_string())
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
        }
    } else {
        // No existing hooks for this event, create new
        if let Some(new_arr) = new_hooks.as_array() {
            hooks.insert(event.to_string(), serde_json::json!(new_arr.clone()));
        }
    }
}

fn remove_hooks(hooks: &mut serde_json::Map<String, serde_json::Value>) {
    for event in ["SessionStart", "PostToolUse"].iter() {
        if let Some(existing) = hooks.get_mut(*event) {
            if let Some(arr) = existing.as_array_mut() {
                arr.retain(|entry| {
                    let is_myceliums = entry["hooks"]
                        .as_array()
                        .map(|h| {
                            h.iter().any(|hook| {
                                hook["command"]
                                    .as_str()
                                    .map(|c| c.contains("myc ") || c.contains("myceliums"))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);
                    !is_myceliums
                });
            }
        }
    }
}
