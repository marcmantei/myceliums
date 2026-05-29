pub mod aider;
pub mod claude;
pub mod codex;
pub mod continue_editor;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod generic_mcp;
pub mod jetbrains;
pub mod kiro;
pub mod vscode;
pub mod windsurf;
pub mod zed;

use anyhow::Result;
use std::path::PathBuf;

/// Trait for editor setup implementations
pub trait EditorSetup {
    /// Setup the editor with myceliums MCP integration
    fn setup(&self, myc_path: &str) -> Result<()>;

    /// Remove myceliums from the editor
    fn uninstall(&self) -> Result<()>;

    /// Get the config path for this editor
    #[allow(dead_code)]
    fn config_path(&self) -> &PathBuf;
}

/// Helper to get the user's home directory
#[allow(dead_code)]
pub fn get_home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))
}

/// Helper to get the config directory (platform-aware)
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf> {
    dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))
}

/// Merge JSON MCP server entries preserving existing servers
pub fn merge_mcp_servers(
    config: &mut serde_json::Value,
    myc_path: &str,
    server_key: &str,
) -> Result<()> {
    let obj = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

    let servers = obj
        .entry(server_key.to_string())
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid {} object", server_key))?;

    servers_obj.insert(
        "myceliums".to_string(),
        serde_json::json!({
            "command": myc_path,
            "args": ["mcp"]
        }),
    );

    Ok(())
}

/// Remove myceliums from MCP servers
pub fn remove_mcp_servers(config: &mut serde_json::Value, server_key: &str) -> Result<()> {
    let obj = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

    if let Some(servers) = obj.get_mut(server_key).and_then(|s| s.as_object_mut()) {
        servers.remove("myceliums");
    }

    Ok(())
}
