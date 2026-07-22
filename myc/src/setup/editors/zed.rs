use super::{remove_mcp_servers, write_user_file, EditorSetup};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Zed editor setup (uses context_servers instead of mcpServers)
pub struct ZedEditor {
    pub config_path: PathBuf,
}

impl ZedEditor {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

/// Get Zed config path based on platform
pub fn get_zed_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("Could not find APPDATA")?;
        Ok(PathBuf::from(appdata).join("Zed").join("settings.json"))
    } else {
        Ok(home.join(".config/zed/settings.json"))
    }
}

/// Create a Zed editor setup handler
pub fn create_zed_editor() -> Result<ZedEditor> {
    let config_path = get_zed_config_path()?;
    Ok(ZedEditor::new(config_path))
}

impl EditorSetup for ZedEditor {
    fn setup(&self, myc_path: &str) -> Result<()> {
        println!("Setting up Zed integration...");
        println!();

        // Ensure parent directory exists
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create directory: {}", parent.display()))?;
        }

        // Read existing config or start fresh
        let mut config: serde_json::Value = if self.config_path.exists() {
            let content = std::fs::read_to_string(&self.config_path)?;
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let obj = config
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

        let servers = obj
            .entry("context_servers".to_string())
            .or_insert_with(|| serde_json::json!({}));

        let servers_obj = servers
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid context_servers object"))?;

        servers_obj.insert(
            "myceliums".to_string(),
            serde_json::json!({
                "command": myc_path,
                "args": ["mcp"]
            }),
        );

        std::fs::write(&self.config_path, serde_json::to_string_pretty(&config)?)?;

        println!(
            "  ✓ MCP server registered in {}",
            self.config_path.display()
        );
        println!();
        println!("  Setup complete! Verify by checking:");
        println!("    {}", self.config_path.display());
        println!();
        println!("  To remove: myc setup-zed --uninstall");

        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        println!("Removing Zed integration...");
        println!();

        if self.config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&self.config_path) {
                if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content) {
                    remove_mcp_servers(&mut config, "context_servers")?;
                    let serialized = serde_json::to_string_pretty(&config)?;
                    write_user_file(&self.config_path, &serialized)?;
                }
            }
            println!("  ✓ MCP server removed from {}", self.config_path.display());
        } else {
            println!(
                "  Nothing to remove ({} not found)",
                self.config_path.display()
            );
        }

        println!();
        println!("  Done. Myceliums integration removed from Zed.");
        Ok(())
    }

    fn config_path(&self) -> &PathBuf {
        &self.config_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zed_path_generation() {
        let path = get_zed_config_path();
        assert!(path.is_ok());
    }
}
