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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Write content the user asked us to persist (an editor config, a settings file),
/// surfacing any failure with the offending path.
///
/// This is the single choke point for user-facing writes in the setup/uninstall
/// flows. Callers must serialize their content into a `String` *before* calling
/// this — never pass `unwrap_or_default()` output, or a serialization failure
/// would silently truncate the user's config to an empty file.
pub fn write_user_file(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_user_file_writes_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        write_user_file(&path, "{\"ok\":true}").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn write_user_file_reports_failure_and_leaves_config_untouched() {
        // A write can only fail structurally (not via permissions) when running
        // as root, so target a path whose parent component is a regular file:
        // the OS rejects it with ENOTDIR for every user.
        let dir = tempfile::tempdir().unwrap();

        let config_path = dir.path().join("config.json");
        let original = "{\"mcpServers\":{\"myceliums\":{}}}";
        std::fs::write(&config_path, original).unwrap();

        // `config.json` is a file, so `config.json/nested.json` cannot be created.
        let unwritable = config_path.join("nested.json");
        let result = write_user_file(&unwritable, "");

        // The write is reported as a failure, and the original config survives.
        assert!(result.is_err(), "write under a non-directory should error");
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            original,
            "user config must be left untouched on write failure"
        );
    }
}
