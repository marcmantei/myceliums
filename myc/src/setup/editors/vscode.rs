use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get VS Code config path based on platform
pub fn get_vscode_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    if cfg!(target_os = "macos") {
        Ok(home.join("Library/Application Support/Code/User/settings.json"))
    } else if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("Could not find APPDATA")?;
        Ok(PathBuf::from(appdata)
            .join("Code")
            .join("User")
            .join("settings.json"))
    } else {
        Ok(home.join(".config/Code/User/settings.json"))
    }
}

/// Create a VS Code editor setup handler
pub fn create_vscode_editor() -> Result<GenericMcpEditor> {
    let config_path = get_vscode_config_path()?;
    Ok(GenericMcpEditor::new("VS Code", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vscode_path_generation() {
        let path = get_vscode_config_path();
        assert!(path.is_ok());
    }
}
