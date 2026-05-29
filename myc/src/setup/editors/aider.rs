use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get Aider config path
pub fn get_aider_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".aider").join("mcp.json"))
}

/// Create an Aider editor setup handler
pub fn create_aider_editor() -> Result<GenericMcpEditor> {
    let config_path = get_aider_config_path()?;
    Ok(GenericMcpEditor::new("Aider", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aider_path_generation() {
        let path = get_aider_config_path();
        assert!(path.is_ok());
    }
}
