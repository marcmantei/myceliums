use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get Google Gemini CLI config path
pub fn get_gemini_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".gemini").join("settings.json"))
}

/// Create a Gemini editor setup handler
pub fn create_gemini_editor() -> Result<GenericMcpEditor> {
    let config_path = get_gemini_config_path()?;
    Ok(GenericMcpEditor::new("Gemini", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_path_generation() {
        let path = get_gemini_config_path();
        assert!(path.is_ok());
    }
}
