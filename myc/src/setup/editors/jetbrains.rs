use super::{remove_mcp_servers, write_user_file, EditorSetup};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// JetBrains IDE setup
pub struct JetBrainsEditor {
    pub config_base_path: PathBuf,
}

impl JetBrainsEditor {
    pub fn new(config_base_path: PathBuf) -> Self {
        Self { config_base_path }
    }
}

/// Get JetBrains config base path based on platform
pub fn get_jetbrains_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    if cfg!(target_os = "macos") {
        Ok(home.join("Library/Application Support/JetBrains"))
    } else if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("Could not find APPDATA")?;
        Ok(PathBuf::from(appdata).join("JetBrains"))
    } else {
        Ok(home.join(".config/JetBrains"))
    }
}

/// Create a JetBrains editor setup handler
pub fn create_jetbrains_editor() -> Result<JetBrainsEditor> {
    let config_path = get_jetbrains_config_path()?;
    Ok(JetBrainsEditor::new(config_path))
}

impl EditorSetup for JetBrainsEditor {
    fn setup(&self, myc_path: &str) -> Result<()> {
        println!("Setting up JetBrains IDE integration...");
        println!();

        // List of common JetBrains IDEs
        let ide_configs = find_jetbrains_ides(&self.config_base_path)?;

        if ide_configs.is_empty() {
            println!("  ! No JetBrains IDE configurations found");
            println!("  Install an IDE (IntelliJ IDEA, PyCharm, WebStorm, etc.) and try again");
            return Ok(());
        }

        for ide_config in ide_configs {
            let mcp_config_path = ide_config.join("options").join("mcp_config.json");

            // Create directory if it doesn't exist
            if let Some(parent) = mcp_config_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // Read existing config or start fresh
            let mut config: serde_json::Value = if mcp_config_path.exists() {
                let content = std::fs::read_to_string(&mcp_config_path)?;
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            let obj = config
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

            let servers = obj
                .entry("mcpServers".to_string())
                .or_insert_with(|| serde_json::json!({}));

            let servers_obj = servers
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Invalid mcpServers object"))?;

            servers_obj.insert(
                "myceliums".to_string(),
                serde_json::json!({
                    "command": myc_path,
                    "args": ["mcp"]
                }),
            );

            std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&config)?)?;

            println!("  ✓ MCP server registered in {}", mcp_config_path.display());
        }

        println!();
        println!("  Setup complete!");
        println!("  Restart your JetBrains IDE to enable myceliums integration");
        println!();
        println!("  To remove: myc setup-jetbrains --uninstall");

        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        println!("Removing JetBrains IDE integration...");
        println!();

        let ide_configs = find_jetbrains_ides(&self.config_base_path)?;

        if ide_configs.is_empty() {
            println!("  Nothing to remove (no JetBrains IDE configurations found)");
            println!();
            return Ok(());
        }

        for ide_config in ide_configs {
            let mcp_config_path = ide_config.join("options").join("mcp_config.json");

            if mcp_config_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&mcp_config_path) {
                    if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&content) {
                        remove_mcp_servers(&mut config, "mcpServers")?;
                        let serialized = serde_json::to_string_pretty(&config)?;
                        write_user_file(&mcp_config_path, &serialized)?;
                    }
                }
                println!("  ✓ MCP server removed from {}", mcp_config_path.display());
            }
        }

        println!();
        println!("  Done. Myceliums integration removed from JetBrains IDEs.");
        Ok(())
    }

    fn config_path(&self) -> &PathBuf {
        &self.config_base_path
    }
}

/// Find all installed JetBrains IDE config directories
fn find_jetbrains_ides(base_path: &PathBuf) -> Result<Vec<PathBuf>> {
    let mut ides = Vec::new();

    if !base_path.exists() {
        return Ok(ides);
    }

    // On macOS, config is like ~/Library/Application Support/JetBrains/IntelliJIdea2024.1
    // On Linux, it's like ~/.config/JetBrains/IntelliJIdea2024.1
    // On Windows, it's like %APPDATA%/JetBrains/IntelliJIdea2024.1

    if let Ok(entries) = std::fs::read_dir(base_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                // Check for common IDE directory patterns
                if dir_name.contains("IntelliJ")
                    || dir_name.contains("PyCharm")
                    || dir_name.contains("WebStorm")
                    || dir_name.contains("Goland")
                    || dir_name.contains("RubyMine")
                    || dir_name.contains("CLion")
                    || dir_name.contains("DataGrip")
                    || dir_name.contains("Rider")
                {
                    ides.push(path);
                }
            }
        }
    }

    Ok(ides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jetbrains_path_generation() {
        let path = get_jetbrains_config_path();
        assert!(path.is_ok());
    }
}
