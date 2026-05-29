use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get Windsurf config path based on platform
pub fn get_windsurf_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    if cfg!(target_os = "windows") {
        let profile = std::env::var("USERPROFILE").context("Could not find USERPROFILE")?;
        Ok(PathBuf::from(profile).join(".windsurf.json"))
    } else {
        Ok(home.join(".windsurf.json"))
    }
}

/// Create a Windsurf editor setup handler
pub fn create_windsurf_editor() -> Result<GenericMcpEditor> {
    let config_path = get_windsurf_config_path()?;
    Ok(GenericMcpEditor::new("Windsurf", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windsurf_path_generation() {
        let path = get_windsurf_config_path();
        assert!(path.is_ok());
    }
}
