use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get GitHub Copilot CLI config path
pub fn get_copilot_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".github-copilot").join("config.json"))
}

/// Create a Copilot editor setup handler
pub fn create_copilot_editor() -> Result<GenericMcpEditor> {
    let config_path = get_copilot_config_path()?;
    Ok(GenericMcpEditor::new("Copilot", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copilot_path_generation() {
        let path = get_copilot_config_path();
        assert!(path.is_ok());
    }
}
