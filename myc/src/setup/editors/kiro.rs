use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get AWS Kiro IDE config path
pub fn get_kiro_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".kiro").join("mcp.json"))
}

/// Create a Kiro editor setup handler
pub fn create_kiro_editor() -> Result<GenericMcpEditor> {
    let config_path = get_kiro_config_path()?;
    Ok(GenericMcpEditor::new("Kiro", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kiro_path_generation() {
        let path = get_kiro_config_path();
        assert!(path.is_ok());
    }
}
