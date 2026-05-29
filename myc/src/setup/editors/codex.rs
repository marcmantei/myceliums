use super::generic_mcp::GenericMcpEditor;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get OpenAI Codex CLI config path
pub fn get_codex_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex").join("config.json"))
}

/// Create a Codex editor setup handler
pub fn create_codex_editor() -> Result<GenericMcpEditor> {
    let config_path = get_codex_config_path()?;
    Ok(GenericMcpEditor::new("Codex", config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_path_generation() {
        let path = get_codex_config_path();
        assert!(path.is_ok());
    }
}
