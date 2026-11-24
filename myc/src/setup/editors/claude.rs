use super::EditorSetup;
use crate::commands::setup_claude::{self, Layers, McpRegistrar, Targets, ToolLoading};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Claude Code editor setup for `myc setup` and the wizard.
///
/// Delegates to the `myc setup-claude` installer at user scope, so the wizard
/// and `myc setup-claude --scope user` write the same files and uninstall the
/// same way.
pub struct ClaudeEditor {
    #[allow(dead_code)]
    pub config_path: PathBuf,
    pub home: PathBuf,
}

impl ClaudeEditor {
    pub fn new(config_path: PathBuf, home: PathBuf) -> Self {
        Self { config_path, home }
    }
}

/// Get Claude config path
pub fn get_claude_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".claude.json"))
}

/// Create a Claude editor setup handler
pub fn create_claude_editor() -> Result<ClaudeEditor> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let config_path = get_claude_config_path()?;
    Ok(ClaudeEditor::new(config_path, home))
}

impl EditorSetup for ClaudeEditor {
    fn setup(&self, myc_path: &str) -> Result<()> {
        println!("Setting up Claude Code integration (user scope)...");
        println!();
        let report = setup_claude::install(
            &Targets::user(&self.home),
            myc_path,
            Layers {
                tool_loading: ToolLoading::for_config(false),
                ..Layers::ALL
            },
            McpRegistrar::ClaudeCli,
        )?;
        for (layer, outcome) in &report.lines {
            println!("  ✓ {layer:<11} {outcome}");
        }
        for note in &report.notes {
            println!("  ! {note}");
        }
        println!();
        println!("  Setup complete! Start Claude Code in any git project:");
        println!("    claude");
        println!();
        println!("  For a per-project install to commit: myc setup-claude (in the repo)");
        println!("  To remove: myc setup-claude --scope user --uninstall");
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        println!("Removing Claude Code integration (user scope)...");
        println!();
        let report = setup_claude::uninstall(&Targets::user(&self.home), McpRegistrar::ClaudeCli)?;
        for (layer, outcome) in &report.lines {
            println!("  ✓ {layer:<11} {outcome}");
        }
        for note in &report.notes {
            println!("  ! {note}");
        }
        println!();
        println!("  Done. Myceliums integration removed from Claude Code.");
        Ok(())
    }

    fn config_path(&self) -> &PathBuf {
        &self.config_path
    }
}
