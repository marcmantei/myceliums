pub mod aider;
pub mod claude;
pub mod codex;
pub mod continue_editor;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod generic_mcp;
pub mod jetbrains;
pub mod kiro;
pub mod vscode;
pub mod windsurf;
pub mod zed;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Write content the user asked us to persist (an editor config, a settings file,
/// CLAUDE.md), surfacing any failure with the offending path.
///
/// This is the single choke point for user-facing writes in the setup/uninstall
/// flows. Callers must serialize their content into a `String` *before* calling
/// this — never pass `unwrap_or_default()` output, or a serialization failure
/// would silently truncate the user's config to an empty file.
///
/// The write is atomic: the content goes to a temporary file in the same
/// directory, which then replaces the target by rename, so a crash or a full
/// disk never leaves a truncated file (and Claude Code never reads a
/// half-written `~/.claude.json`). A symlink is followed and its target
/// replaced, so a `CLAUDE.md -> AGENTS.md` link survives; an existing file's
/// permissions are kept. A file with several hard links is written in place
/// instead: a rename would split it from its other names.
pub fn write_user_file(path: &Path, content: &str) -> Result<()> {
    let target = resolve_symlinks(path);
    if has_hard_links(&target) {
        return std::fs::write(&target, content)
            .with_context(|| format!("Failed to write {}", path.display()));
    }
    write_atomically(&target, content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// Whether `path` is one of several names of its file.
#[cfg(unix)]
fn has_hard_links(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
}

#[cfg(not(unix))]
fn has_hard_links(_path: &Path) -> bool {
    false
}

/// Follows symlinks from `path` to the file they finally name (which may not
/// exist yet). Relative links resolve against the link's directory.
fn resolve_symlinks(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    // A bound instead of cycle detection: the OS gives up at 40 hops too.
    for _ in 0..40 {
        let is_link = std::fs::symlink_metadata(&current)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            break;
        }
        match std::fs::read_link(&current) {
            Ok(link) if link.is_absolute() => current = link,
            Ok(link) => {
                current = current
                    .parent()
                    .map_or_else(|| link.clone(), |dir| dir.join(&link));
            }
            Err(_) => break,
        }
    }
    current
}

fn write_atomically(target: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let permissions = std::fs::metadata(target).ok().map(|m| m.permissions());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.myc-{}-{nanos}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = permissions {
            std::fs::set_permissions(&tmp, permissions)?;
        }
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        // best-effort: the temp file is ours; the error below is what counts.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Trait for editor setup implementations
pub trait EditorSetup {
    /// Setup the editor with myceliums MCP integration
    fn setup(&self, myc_path: &str) -> Result<()>;

    /// Remove myceliums from the editor
    fn uninstall(&self) -> Result<()>;

    /// Get the config path for this editor
    #[allow(dead_code)]
    fn config_path(&self) -> &PathBuf;
}

/// Helper to get the user's home directory
#[allow(dead_code)]
pub fn get_home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))
}

/// Helper to get the config directory (platform-aware)
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf> {
    dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))
}

/// Merge JSON MCP server entries preserving existing servers
pub fn merge_mcp_servers(
    config: &mut serde_json::Value,
    myc_path: &str,
    server_key: &str,
) -> Result<()> {
    let obj = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

    let servers = obj
        .entry(server_key.to_string())
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid {} object", server_key))?;

    servers_obj.insert(
        "myceliums".to_string(),
        serde_json::json!({
            "command": myc_path,
            "args": ["mcp"]
        }),
    );

    Ok(())
}

/// Remove myceliums from MCP servers
pub fn remove_mcp_servers(config: &mut serde_json::Value, server_key: &str) -> Result<()> {
    let obj = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid config format"))?;

    if let Some(servers) = obj.get_mut(server_key).and_then(|s| s.as_object_mut()) {
        servers.remove("myceliums");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_user_file_writes_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        write_user_file(&path, "{\"ok\":true}").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn write_user_file_replaces_content_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "a much longer original content").unwrap();
        write_user_file(&path, "short").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "short");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn write_user_file_follows_symlinks_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("AGENTS.md");
        let claude = dir.path().join("CLAUDE.md");
        std::fs::write(&agents, "rules\n").unwrap();
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o600)).unwrap();
        // A relative link, as `ln -s AGENTS.md CLAUDE.md` makes it.
        std::os::unix::fs::symlink("AGENTS.md", &claude).unwrap();

        write_user_file(&claude, "rules\nmore\n").unwrap();
        assert!(
            std::fs::symlink_metadata(&claude)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must survive"
        );
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), "rules\nmore\n");
        let mode = std::fs::metadata(&agents).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        // A dangling link gets its target created, not replaced by a file.
        let dangling = dir.path().join("dangling.md");
        std::os::unix::fs::symlink("created-by-write.md", &dangling).unwrap();
        write_user_file(&dangling, "x").unwrap();
        assert!(std::fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("created-by-write.md")).unwrap(),
            "x"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_user_file_keeps_hard_links_together() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("AGENTS.md");
        let claude = dir.path().join("CLAUDE.md");
        std::fs::write(&agents, "rules\n").unwrap();
        std::fs::hard_link(&agents, &claude).unwrap();
        write_user_file(&claude, "more\n").unwrap();
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), "more\n");
        assert!(same_file::is_same_file(&agents, &claude).unwrap());
    }

    #[test]
    fn write_user_file_reports_failure_and_leaves_config_untouched() {
        // A write can only fail structurally (not via permissions) when running
        // as root, so target a path whose parent component is a regular file:
        // the OS rejects it with ENOTDIR for every user.
        let dir = tempfile::tempdir().unwrap();

        let config_path = dir.path().join("config.json");
        let original = "{\"mcpServers\":{\"myceliums\":{}}}";
        std::fs::write(&config_path, original).unwrap();

        // `config.json` is a file, so `config.json/nested.json` cannot be created.
        let unwritable = config_path.join("nested.json");
        let result = write_user_file(&unwritable, "");

        // The write is reported as a failure, and the original config survives.
        assert!(result.is_err(), "write under a non-directory should error");
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            original,
            "user config must be left untouched on write failure"
        );
    }
}
