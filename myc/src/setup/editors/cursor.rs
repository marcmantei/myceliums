use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get Cursor config path (~/.cursor/mcp.json)
pub fn get_cursor_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".cursor").join("mcp.json"))
}

/// Create a Cursor editor setup handler
pub fn create_cursor_editor() -> Result<GenericMcpEditor> {
    let config_path = get_cursor_config_path()?;
    Ok(GenericMcpEditor::new("Cursor", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_path_generation() {
        let path = get_cursor_config_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains(".cursor"));
        assert!(path.to_string_lossy().ends_with("mcp.json"));
    }
}
