//! Interactive setup wizard for Myceliums.
//!
//! Guides the user through editor detection, AI instruction preferences,
//! and analysis mode selection with a branded CLI experience.

use anyhow::Result;
use console::style;
use dialoguer::{Confirm, Select};
use myceliums_core::global_config::{GlobalConfig, SetupConfig};
use std::path::Path;

use super::editor_detector::EditorDetector;
use super::SetupOrchestrator;

const BANNER: &str = r#"
    ╔══════════════════════════════════════════════════════╗
    ║                                                      ║
    ║   ┏┳┓         ┓•                                     ║
    ║   ┃┃┃┓┏┏┏┓┃┓┏┏┳┃┏                                   ║
    ║   ┛ ┗┗┫┗┗━┗┗┗┛┗┗┛                                   ║
    ║       ┛                                              ║
    ║   The code knowledge graph for AI agents             ║
    ║                                                      ║
    ╚══════════════════════════════════════════════════════╝
"#;

/// Run the interactive setup wizard.
///
/// Returns `Ok(true)` if setup was completed, `Ok(false)` if cancelled.
pub async fn run_wizard(data_dir: &Path) -> Result<bool> {
    print_banner();

    let myc_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "myc".to_string());

    // Step 1: Editor Detection
    println!(
        "  {} {}",
        style("Step 1/3").cyan().bold(),
        style("Editor Detection").bold()
    );
    println!("  {}", style("─".repeat(50)).dim());
    println!();

    let detected = EditorDetector::detect_all();

    if detected.is_empty() {
        println!(
            "  {} No supported editors detected.",
            style("!").yellow().bold()
        );
        println!("  You can configure editors manually later with:");
        println!(
            "    {}",
            style("myc setup --editor <name>").cyan()
        );
        println!();
    } else {
        println!(
            "  {} Detected {} editor(s):",
            style("✓").green().bold(),
            detected.len()
        );
        for editor in &detected {
            println!("    {} {}", style("•").cyan(), editor.name);
        }
        println!();
    }

    let configure_editors = if !detected.is_empty() {
        Confirm::new()
            .with_prompt(format!(
                "  Configure {} editor(s) with Myceliums MCP?",
                detected.len()
            ))
            .default(true)
            .interact()?
    } else {
        false
    };

    let mut configured_editors = Vec::new();
    if configure_editors {
        for editor in &detected {
            let editor_name = editor.name.to_lowercase().replace(" ide", "").replace(" code", "");
            match SetupOrchestrator::setup_editor(&editor_name, &myc_path) {
                Ok(_) => {
                    println!(
                        "    {} {} configured",
                        style("✓").green().bold(),
                        editor.name
                    );
                    configured_editors.push(editor.name.clone());
                }
                Err(e) => {
                    println!(
                        "    {} {} failed: {}",
                        style("✗").red().bold(),
                        editor.name,
                        e
                    );
                }
            }
        }
        println!();
    }

    // Step 2: AI Instructions
    println!(
        "  {} {}",
        style("Step 2/3").cyan().bold(),
        style("AI Instructions").bold()
    );
    println!("  {}", style("─".repeat(50)).dim());
    println!();
    println!(
        "  Myceliums can instruct your AI agent to prefer graph-based"
    );
    println!(
        "  tools over grep/file search, improving token efficiency."
    );
    println!();

    let instructions_enabled = Confirm::new()
        .with_prompt("  Enable AI instructions in your editor config?")
        .default(true)
        .interact()?;

    println!();

    // Step 3: Analysis Mode
    println!(
        "  {} {}",
        style("Step 3/3").cyan().bold(),
        style("Analysis Mode").bold()
    );
    println!("  {}", style("─".repeat(50)).dim());
    println!();

    let analysis_options = vec![
        "On session start — auto-analyze when your editor opens (Recommended)",
        "Watch mode — continuously re-index on file changes",
        "Manual — run 'myc analyze' yourself",
    ];

    let analysis_selection = Select::new()
        .with_prompt("  When should Myceliums update its knowledge graph?")
        .items(&analysis_options)
        .default(0)
        .interact()?;

    let analysis_mode = match analysis_selection {
        0 => "session-start",
        1 => "watch",
        _ => "manual",
    }
    .to_string();

    println!();

    // Save preferences
    let mut config = GlobalConfig::load(data_dir).unwrap_or_default();
    config.setup = SetupConfig {
        completed: true,
        instructions_enabled,
        analysis_mode: analysis_mode.clone(),
        configured_editors: configured_editors.clone(),
    };
    // Ensure data dir exists before saving
    std::fs::create_dir_all(data_dir)?;
    let config_path = GlobalConfig::config_path(data_dir);
    config.set_path(config_path);
    config.save()?;

    // Summary
    print_summary(&configured_editors, instructions_enabled, &analysis_mode);

    Ok(true)
}

/// Run setup in non-interactive mode (--all flag).
pub async fn run_setup_all(data_dir: &Path) -> Result<()> {
    let myc_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "myc".to_string());

    SetupOrchestrator::setup_all(&myc_path)?;

    let detected = EditorDetector::detect_all();
    let configured_editors: Vec<String> = detected.iter().map(|e| e.name.clone()).collect();

    let mut config = GlobalConfig::load(data_dir).unwrap_or_default();
    config.setup = SetupConfig {
        completed: true,
        instructions_enabled: true,
        analysis_mode: "session-start".to_string(),
        configured_editors,
    };
    std::fs::create_dir_all(data_dir)?;
    let config_path = GlobalConfig::config_path(data_dir);
    config.set_path(config_path);
    config.save()?;

    Ok(())
}

fn print_banner() {
    eprintln!("{}", style(BANNER).cyan());
}

fn print_summary(
    configured_editors: &[String],
    instructions_enabled: bool,
    analysis_mode: &str,
) {
    println!("  {}", style("═".repeat(54)).dim());
    println!(
        "  {}",
        style("Setup Complete!").green().bold()
    );
    println!("  {}", style("═".repeat(54)).dim());
    println!();
    if configured_editors.is_empty() {
        println!(
            "    {} No editors configured",
            style("○").dim()
        );
    } else {
        println!(
            "    {} {} editor(s) configured ({})",
            style("✓").green().bold(),
            configured_editors.len(),
            configured_editors.join(", ")
        );
    }
    println!(
        "    {} AI instructions: {}",
        style(if instructions_enabled { "✓" } else { "○" })
            .green()
            .bold(),
        if instructions_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    let mode_display = match analysis_mode {
        "session-start" => "on session start",
        "watch" => "watch mode",
        _ => "manual",
    };
    println!(
        "    {} Analysis mode: {}",
        style("✓").green().bold(),
        mode_display
    );
    println!();
    println!(
        "  {} Zed and JetBrains may require a restart.",
        style("Note:").dim()
    );
    println!(
        "  {} Run {} to reverse.",
        style("Tip:").dim(),
        style("myc setup --uninstall").cyan()
    );
    println!();
}
