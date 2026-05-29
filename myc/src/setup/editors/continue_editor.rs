use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get Continue config path based on platform
pub fn get_continue_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    if cfg!(target_os = "windows") {
        let profile = std::env::var("USERPROFILE").context("Could not find USERPROFILE")?;
        Ok(PathBuf::from(profile).join(".continue").join("config.json"))
    } else {
        Ok(home.join(".continue/config.json"))
    }
}

/// Create a Continue editor setup handler
pub fn create_continue_editor() -> Result<GenericMcpEditor> {
    let config_path = get_continue_config_path()?;
    Ok(GenericMcpEditor::new("Continue", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_continue_path_generation() {
        let path = get_continue_config_path();
        assert!(path.is_ok());
    }
}
