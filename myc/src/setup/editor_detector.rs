use std::path::PathBuf;

/// Information about a detected editor
#[derive(Debug, Clone)]
pub struct DetectedEditor {
    pub name: String,
    #[allow(dead_code)]
    pub config_path: PathBuf,
    #[allow(dead_code)]
    pub is_installed: bool,
}

/// Detects installed editors on the system
pub struct EditorDetector;

impl EditorDetector {
    /// Detect all installed editors
    pub fn detect_all() -> Vec<DetectedEditor> {
        let mut editors = Vec::new();

        if let Some(editor) = Self::detect_claude() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_windsurf() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_zed() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_continue() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_vscode() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_jetbrains() {
            editors.push(editor);
        }
        if let Some(editor) = Self::detect_cursor() {
            editors.push(editor);
        }

        editors
    }

    /// Check if executable exists in PATH (simplified check)
    fn has_executable(name: &str) -> bool {
        // Use `command -v` on Unix-like systems, `where` on Windows
        let cmd = if cfg!(target_os = "windows") {
            format!("where {}", name)
        } else {
            format!("command -v {}", name)
        };

        std::process::Command::new(if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        })
        .args(if cfg!(target_os = "windows") {
            vec!["/C", &cmd]
        } else {
            vec!["-c", &cmd]
        })
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    }

    /// Detect Claude Code
    fn detect_claude() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = home.join(".claude.json");
        let is_installed = config_path.exists() || Self::has_executable("claude");

        if is_installed {
            Some(DetectedEditor {
                name: "Claude Code".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }

    /// Detect Windsurf
    fn detect_windsurf() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
            home.join(".windsurf.json")
        } else if cfg!(target_os = "windows") {
            let profile = std::env::var("USERPROFILE").ok()?;
            PathBuf::from(profile).join(".windsurf.json")
        } else {
            home.join(".windsurf.json")
        };

        let is_installed = config_path.exists() || Self::has_executable("windsurf");

        if is_installed {
            Some(DetectedEditor {
                name: "Windsurf".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }

    /// Detect Zed
    fn detect_zed() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = if cfg!(target_os = "windows") {
            let appdata = std::env::var("APPDATA").ok()?;
            PathBuf::from(appdata).join("Zed").join("settings.json")
        } else {
            home.join(".config/zed/settings.json")
        };

        let is_installed = config_path.exists() || Self::has_executable("zed");

        if is_installed {
            Some(DetectedEditor {
                name: "Zed".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }

    /// Detect Continue
    fn detect_continue() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = if cfg!(target_os = "windows") {
            let profile = std::env::var("USERPROFILE").ok()?;
            PathBuf::from(profile).join(".continue").join("config.json")
        } else {
            home.join(".continue/config.json")
        };

        let is_installed = config_path.exists() || Self::has_executable("continue");

        if is_installed {
            Some(DetectedEditor {
                name: "Continue".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }

    /// Detect VS Code
    fn detect_vscode() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Code/User/settings.json")
        } else if cfg!(target_os = "windows") {
            let appdata = std::env::var("APPDATA").ok()?;
            PathBuf::from(appdata)
                .join("Code")
                .join("User")
                .join("settings.json")
        } else {
            home.join(".config/Code/User/settings.json")
        };

        let is_installed = config_path.exists() || Self::has_executable("code");

        if is_installed {
            Some(DetectedEditor {
                name: "VS Code".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }

    /// Detect JetBrains IDEs
    fn detect_jetbrains() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;

        // List of common JetBrains IDEs to check
        let ide_names = if cfg!(target_os = "linux") {
            vec!["idea", "pycharm", "webstorm", "goland", "rubymine"]
        } else {
            // macOS and Windows use the same IDE names
            vec!["IntelliJ IDEA", "PyCharm", "WebStorm", "Goland", "RubyMine"]
        };

        // Check for any JetBrains IDE
        for ide in ide_names {
            let is_installed = if cfg!(target_os = "macos") {
                let app_path = PathBuf::from(format!("/Applications/{}.app", ide));
                app_path.exists()
            } else if cfg!(target_os = "windows") {
                let appdata = std::env::var("APPDATA").ok();
                let localappdata = std::env::var("LOCALAPPDATA").ok();
                let check_appdata = appdata.map(|p| PathBuf::from(p).join("JetBrains").exists());
                let check_local = localappdata.map(|p| PathBuf::from(p).join("JetBrains").exists());

                check_appdata.unwrap_or(false) || check_local.unwrap_or(false)
            } else {
                // Linux: check for IDE executable in PATH
                Self::has_executable(ide.to_lowercase().as_str())
            };

            if is_installed {
                // For JetBrains, we'll use a generic config path
                let config_path = if cfg!(target_os = "macos") {
                    home.join("Library/Application Support/JetBrains")
                } else if cfg!(target_os = "windows") {
                    let appdata = std::env::var("APPDATA").ok()?;
                    PathBuf::from(appdata).join("JetBrains")
                } else {
                    home.join(".config/JetBrains")
                };

                return Some(DetectedEditor {
                    name: "JetBrains IDE".to_string(),
                    config_path,
                    is_installed: true,
                });
            }
        }

        None
    }

    /// Detect Cursor
    fn detect_cursor() -> Option<DetectedEditor> {
        let home = dirs::home_dir()?;
        let config_path = home.join(".cursor").join("mcp.json");

        let is_installed =
            config_path.exists() || home.join(".cursor").exists() || Self::has_executable("cursor");

        if is_installed {
            Some(DetectedEditor {
                name: "Cursor".to_string(),
                config_path,
                is_installed: true,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_editor_detector() {
        let editors = EditorDetector::detect_all();
        // We can't guarantee any editor is installed, but we can check the structure
        for editor in editors {
            assert!(!editor.name.is_empty());
            assert!(editor.is_installed);
        }
    }
}
