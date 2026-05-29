use super::{merge_mcp_servers, remove_mcp_servers, EditorSetup};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Generic MCP editor setup (for editors that use a simple JSON config with mcpServers key)
pub struct GenericMcpEditor {
    pub name: String,
    pub config_path: PathBuf,
    pub server_key: String, // Usually "mcpServers"
}

impl GenericMcpEditor {
    pub fn new(name: impl Into<String>, config_path: PathBuf) -> Self {
        Self {
            name: name.into(),
            config_path,
            server_key: "mcpServers".to_string(),
        }
    }

    #[allow(dead_code)]
    pub fn with_server_key(mut self, key: impl Into<String>) -> Self {
        self.server_key = key.into();
        self
    }
}

impl EditorSetup for GenericMcpEditor {
    fn setup(&self, myc_path: &str) -> Result<()> {
        println!("Setting up {} integration...", self.name);
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

        merge_mcp_servers(&mut config, myc_path, &self.server_key)?;

        std::fs::write(&self.config_path, serde_json::to_string_pretty(&config)?)?;

        println!(
            "  ✓ MCP server registered in {}",
            self.config_path.display()
        );
        println!();
        println!("  Setup complete! Verify by checking:");
        println!("    {}", self.config_path.display());
        println!();
        println!(
            "  To remove: myc setup-{} --uninstall",
            self.name.to_lowercase()
        );

        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        println!("Removing {} integration...", self.name);
        println!();

        if self.config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&self.config_path) {
                if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content) {
                    remove_mcp_servers(&mut config, &self.server_key)?;
                    std::fs::write(
                        &self.config_path,
                        serde_json::to_string_pretty(&config).unwrap_or_default(),
                    )?;
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
        println!("  Done. Myceliums integration removed from {}.", self.name);
        Ok(())
    }

    fn config_path(&self) -> &PathBuf {
        &self.config_path
    }
}
