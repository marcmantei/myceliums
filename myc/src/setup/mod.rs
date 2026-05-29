pub mod editor_detector;
pub mod editors;
pub mod wizard;

use anyhow::Result;
use editor_detector::EditorDetector;
use editors::{
    aider, claude, codex, continue_editor, copilot, cursor, gemini, jetbrains, kiro, vscode,
    windsurf, zed, EditorSetup,
};

/// Setup orchestration for all editors
pub struct SetupOrchestrator;

impl SetupOrchestrator {
    /// Auto-detect and setup all installed editors
    pub fn setup_all(myc_path: &str) -> Result<()> {
        let detected = EditorDetector::detect_all();

        if detected.is_empty() {
            println!("No installed editors detected.");
            println!();
            println!("Supported editors: Claude Code, Windsurf, Zed, Continue, VS Code, JetBrains");
            println!();
            println!("Install an editor and run 'myc setup' again to configure.");
            return Ok(());
        }

        println!("Detected {} editor(s):", detected.len());
        for editor in &detected {
            println!("  • {}", editor.name);
        }
        println!();

        let mut success_count = 0;
        let mut failed = Vec::new();

        for editor in detected {
            let result = match editor.name.as_str() {
                "Claude Code" => claude::create_claude_editor().and_then(|e| e.setup(myc_path)),
                "Windsurf" => windsurf::create_windsurf_editor().and_then(|e| e.setup(myc_path)),
                "Zed" => zed::create_zed_editor().and_then(|e| e.setup(myc_path)),
                "Continue" => {
                    continue_editor::create_continue_editor().and_then(|e| e.setup(myc_path))
                }
                "VS Code" => vscode::create_vscode_editor().and_then(|e| e.setup(myc_path)),
                "JetBrains IDE" => {
                    jetbrains::create_jetbrains_editor().and_then(|e| e.setup(myc_path))
                }
                "Cursor" => cursor::create_cursor_editor().and_then(|e| e.setup(myc_path)),
                _ => Ok(()),
            };

            match result {
                Ok(_) => success_count += 1,
                Err(e) => failed.push((editor.name.clone(), e.to_string())),
            }

            println!();
        }

        // Summary
        println!("═══════════════════════════════════════════════════════════");
        println!("Setup Summary:");
        println!("  ✓ {} editor(s) configured", success_count);
        if !failed.is_empty() {
            println!("  ✗ {} editor(s) failed:", failed.len());
            for (name, error) in failed {
                println!("    - {}: {}", name, error);
            }
        }
        println!("═══════════════════════════════════════════════════════════");

        Ok(())
    }

    /// Setup a specific editor
    pub fn setup_editor(editor_name: &str, myc_path: &str) -> Result<()> {
        match editor_name.to_lowercase().as_str() {
            "claude" => claude::create_claude_editor()?.setup(myc_path),
            "windsurf" => windsurf::create_windsurf_editor()?.setup(myc_path),
            "zed" => zed::create_zed_editor()?.setup(myc_path),
            "continue" => continue_editor::create_continue_editor()?.setup(myc_path),
            "vscode" | "code" => vscode::create_vscode_editor()?.setup(myc_path),
            "jetbrains" => jetbrains::create_jetbrains_editor()?.setup(myc_path),
            "gemini" => gemini::create_gemini_editor()?.setup(myc_path),
            "codex" => codex::create_codex_editor()?.setup(myc_path),
            "copilot" => copilot::create_copilot_editor()?.setup(myc_path),
            "aider" => aider::create_aider_editor()?.setup(myc_path),
            "kiro" => kiro::create_kiro_editor()?.setup(myc_path),
            "cursor" => cursor::create_cursor_editor()?.setup(myc_path),
            _ => Err(anyhow::anyhow!(
                "Unknown editor: {}. Supported: claude, cursor, windsurf, zed, continue, vscode, jetbrains, gemini, codex, copilot, aider, kiro",
                editor_name
            )),
        }
    }

    /// Uninstall from a specific editor
    pub fn uninstall_editor(editor_name: &str) -> Result<()> {
        match editor_name.to_lowercase().as_str() {
            "claude" => claude::create_claude_editor()?.uninstall(),
            "windsurf" => windsurf::create_windsurf_editor()?.uninstall(),
            "zed" => zed::create_zed_editor()?.uninstall(),
            "continue" => continue_editor::create_continue_editor()?.uninstall(),
            "vscode" | "code" => vscode::create_vscode_editor()?.uninstall(),
            "jetbrains" => jetbrains::create_jetbrains_editor()?.uninstall(),
            "gemini" => gemini::create_gemini_editor()?.uninstall(),
            "codex" => codex::create_codex_editor()?.uninstall(),
            "copilot" => copilot::create_copilot_editor()?.uninstall(),
            "aider" => aider::create_aider_editor()?.uninstall(),
            "kiro" => kiro::create_kiro_editor()?.uninstall(),
            "cursor" => cursor::create_cursor_editor()?.uninstall(),
            _ => Err(anyhow::anyhow!(
                "Unknown editor: {}. Supported: claude, cursor, windsurf, zed, continue, vscode, jetbrains, gemini, codex, copilot, aider, kiro",
                editor_name
            )),
        }
    }

    /// Uninstall from all editors
    pub fn uninstall_all() -> Result<()> {
        let detected = EditorDetector::detect_all();

        if detected.is_empty() {
            println!("No installed editors detected.");
            return Ok(());
        }

        println!("Removing myceliums from {} editor(s):", detected.len());
        for editor in &detected {
            println!("  • {}", editor.name);
        }
        println!();

        let mut success_count = 0;
        let mut failed = Vec::new();

        for editor in detected {
            let result = match editor.name.as_str() {
                "Claude Code" => claude::create_claude_editor().and_then(|e| e.uninstall()),
                "Windsurf" => windsurf::create_windsurf_editor().and_then(|e| e.uninstall()),
                "Zed" => zed::create_zed_editor().and_then(|e| e.uninstall()),
                "Continue" => continue_editor::create_continue_editor().and_then(|e| e.uninstall()),
                "VS Code" => vscode::create_vscode_editor().and_then(|e| e.uninstall()),
                "JetBrains IDE" => jetbrains::create_jetbrains_editor().and_then(|e| e.uninstall()),
                _ => Ok(()),
            };

            match result {
                Ok(_) => success_count += 1,
                Err(e) => failed.push((editor.name.clone(), e.to_string())),
            }

            println!();
        }

        // Summary
        println!("═══════════════════════════════════════════════════════════");
        println!("Uninstall Summary:");
        println!("  ✓ {} editor(s) cleaned", success_count);
        if !failed.is_empty() {
            println!("  ✗ {} editor(s) failed:", failed.len());
            for (name, error) in failed {
                println!("    - {}: {}", name, error);
            }
        }
        println!("═══════════════════════════════════════════════════════════");

        Ok(())
    }
}
