//! The routing block: a short, versioned instruction block that `myc
//! setup-claude` / `myc setup-codex` keep in a project's CLAUDE.md or
//! AGENTS.md between `<!-- BEGIN myceliums -->` and `<!-- END myceliums -->`
//! (#126, layer 1).
//!
//! The block tells the agent which questions go to the code graph and which
//! stay with grep. It is loaded on every turn, so it is kept under 15 lines.
//! Everything between the markers belongs to myceliums: installing replaces
//! it, uninstalling removes it, and text outside the markers is never touched.

use std::path::Path;

use anyhow::{bail, Context as _, Result};

/// First line of the managed block.
pub const BEGIN: &str = "<!-- BEGIN myceliums -->";
/// Last line of the managed block.
pub const END: &str = "<!-- END myceliums -->";
/// Version of the block's wording; bump it when the text changes.
pub const ROUTING_VERSION: u32 = 2;

/// Which agent the block is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Claude Code's CLAUDE.md; mentions the `code-explorer` subagent when it
    /// is installed.
    Claude {
        /// Whether `.claude/agents/code-explorer.md` is installed alongside.
        subagent: bool,
    },
    /// AGENTS.md for Codex and other agents that read it.
    Agents,
}

/// The block, markers included, with `\n` line endings.
pub fn routing_block(flavor: Flavor) -> String {
    let (setup, also) = match flavor {
        Flavor::Claude { subagent: true } => (
            "myc setup-claude",
            "The myceliums MCP tools and the `code-explorer` subagent answer the same questions.",
        ),
        Flavor::Claude { subagent: false } => (
            "myc setup-claude",
            "The myceliums MCP tools (`find_callers`, `find_callees`, `impact_of_change`, `search_code`) answer the same questions.",
        ),
        Flavor::Agents => (
            "myc setup-codex",
            "The myceliums MCP tools (`find_callers`, `find_callees`, `impact_of_change`, `search_code`) answer the same questions.",
        ),
    };
    format!(
        "{BEGIN}\n\
         <!-- v{ROUTING_VERSION}, written by `{setup}`: edits inside these markers are overwritten; `{setup} --uninstall` removes them -->\n\
         ## Code navigation: myceliums\n\
         Structural questions go to the myceliums code graph first, not grep/rg:\n\
         - callers / callees of X: `myc callers X`, `myc callees X` (`--depth 3` for chains)\n\
         - what breaks if X or a file changes: `myc impact X`, `myc impact path/to/file`\n\
         - where or how something is implemented: `myc context \"<question>\"`\n\
         {also}\n\
         grep/rg stay right for string literals, config values, log messages and non-code files.\n\
         {END}\n"
    )
}

/// Byte ranges of every managed block in `text`, each from the start of its
/// BEGIN line to the end of its END line (including the line break).
///
/// Markers count only when they stand alone on a line, so prose that quotes
/// them (`<!-- BEGIN myceliums -->` in backticks) is left alone.
fn block_ranges(text: &str) -> Result<Vec<(usize, usize)>> {
    let mut ranges = Vec::new();
    let mut open: Option<usize> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        match line.trim() {
            BEGIN if open.is_some() => {
                bail!("found a second `{BEGIN}` before `{END}`; fix the file by hand, then re-run")
            }
            BEGIN => open = Some(start),
            END => match open.take() {
                Some(begin) => ranges.push((begin, offset)),
                None => {
                    bail!("found `{END}` without a `{BEGIN}`; fix the file by hand, then re-run")
                }
            },
            _ => {}
        }
    }
    if open.is_some() {
        bail!("found `{BEGIN}` without a matching `{END}`; fix the file by hand, then re-run");
    }
    Ok(ranges)
}

/// `text` with the managed block set to `block`.
///
/// An existing block is replaced in place (extra copies are dropped);
/// otherwise the block is appended after a blank line. CRLF files get CRLF.
pub fn upsert_block(text: &str, block: &str) -> Result<String> {
    let block = if text.contains("\r\n") {
        block.replace('\n', "\r\n")
    } else {
        block.to_string()
    };
    let ranges = block_ranges(text)?;
    if ranges.is_empty() {
        // One blank line separates the block from the text before it: a line
        // break after the last line (when missing) plus one more.
        // `remove_block` takes exactly that one extra line break away again.
        let mut out = text.to_string();
        if !out.is_empty() {
            let nl = newline_of(text);
            if !out.ends_with('\n') {
                out.push_str(nl);
            }
            out.push_str(nl);
        }
        out.push_str(&block);
        return Ok(out);
    }
    let mut out = String::with_capacity(text.len() + block.len());
    let mut last = 0;
    for (i, (start, end)) in ranges.iter().enumerate() {
        out.push_str(&text[last..*start]);
        if i == 0 {
            out.push_str(&block);
        }
        last = *end;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// `text` without any managed block, or `None` when it holds none. The blank
/// line that [`upsert_block`] put before an appended block goes too.
pub fn remove_block(text: &str) -> Result<Option<String>> {
    let ranges = block_ranges(text)?;
    if ranges.is_empty() {
        return Ok(None);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in ranges {
        out.push_str(&text[last..start]);
        last = end;
    }
    let rest = &text[last..];
    if rest.trim().is_empty() {
        // The block was at the end: drop the one line break `upsert_block`
        // added as a separator, and nothing else of the user's text.
        let separator = format!("{nl}{nl}", nl = newline_of(text));
        if out.ends_with(&separator) {
            out.truncate(out.len() - separator.len() / 2);
        }
    }
    out.push_str(rest);
    Ok(Some(out))
}

/// The line break `text` uses.
fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// What happened to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// The file did not exist and was created.
    Created,
    /// The file changed.
    Updated,
    /// The file already had the wanted content.
    Unchanged,
    /// Our part was removed; other content remains.
    Removed,
    /// Our part was removed and the file, now empty, was deleted.
    Deleted,
    /// There was nothing of ours to remove.
    Absent,
}

/// Writes or refreshes the managed block in the file at `path`.
pub fn install_block(path: &Path, block: &str) -> Result<FileChange> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };
    let current = existing.as_deref().unwrap_or("");
    let updated = upsert_block(current, block).with_context(|| path.display().to_string())?;
    if existing.is_some() && updated == current {
        return Ok(FileChange::Unchanged);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    crate::setup::editors::write_user_file(path, &updated)?;
    Ok(if existing.is_some() {
        FileChange::Updated
    } else {
        FileChange::Created
    })
}

/// Removes the managed block from the file at `path`. When `delete_if_empty`
/// is set and nothing but whitespace remains, the file is deleted.
pub fn uninstall_block(path: &Path, delete_if_empty: bool) -> Result<FileChange> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileChange::Absent),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };
    let Some(rest) = remove_block(&text).with_context(|| path.display().to_string())? else {
        return Ok(FileChange::Absent);
    };
    if delete_if_empty && rest.trim().is_empty() && !crate::commands::setup_claude::is_symlink(path)
    {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to delete {}", path.display()))?;
        return Ok(FileChange::Deleted);
    }
    crate::setup::editors::write_user_file(path, &rest)?;
    Ok(FileChange::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> String {
        routing_block(Flavor::Claude { subagent: true })
    }

    #[test]
    fn block_is_short_versioned_and_marked() {
        for flavor in [
            Flavor::Claude { subagent: true },
            Flavor::Claude { subagent: false },
            Flavor::Agents,
        ] {
            let b = routing_block(flavor);
            let lines: Vec<&str> = b.lines().collect();
            assert!(lines.len() < 15, "{} lines", lines.len());
            assert_eq!(lines.first().copied(), Some(BEGIN));
            assert_eq!(lines.last().copied(), Some(END));
            assert!(b.contains(&format!("v{ROUTING_VERSION}")));
            assert!(b.contains("myc callers") && b.contains("grep/rg"));
        }
        assert!(routing_block(Flavor::Claude { subagent: true }).contains("code-explorer"));
        assert!(!routing_block(Flavor::Agents).contains("code-explorer"));
        assert!(routing_block(Flavor::Agents).contains("myc setup-codex --uninstall"));
    }

    #[test]
    fn upsert_appends_once_and_is_idempotent() {
        let original = "# Project\n\nSome rules.\n";
        let once = upsert_block(original, &block()).unwrap();
        assert!(once.starts_with(original));
        assert!(once.contains("Some rules.\n\n<!-- BEGIN myceliums -->"));
        let twice = upsert_block(&once, &block()).unwrap();
        assert_eq!(once, twice);
        assert_eq!(upsert_block("", &block()).unwrap(), block());
        // A file without a trailing newline still gets a blank separator line.
        assert!(upsert_block("x", &block())
            .unwrap()
            .starts_with("x\n\n<!-- BEGIN"));
    }

    #[test]
    fn upsert_replaces_an_old_block_in_place_and_keeps_surrounding_text() {
        let text = format!("top\n\n{BEGIN}\nold v0 text\n{END}\n\nbottom\n");
        let out = upsert_block(&text, &block()).unwrap();
        assert!(out.starts_with("top\n\n<!-- BEGIN myceliums -->\n"));
        assert!(out.ends_with(&format!("{END}\n\nbottom\n")));
        assert!(!out.contains("old v0 text"));
        // Duplicate blocks collapse into one.
        let dup = format!("{BEGIN}\na\n{END}\nmid\n{BEGIN}\nb\n{END}\n");
        let out = upsert_block(&dup, &block()).unwrap();
        assert_eq!(out.matches(BEGIN).count(), 1);
        assert!(out.contains("mid\n"));
    }

    #[test]
    fn remove_restores_the_original_text() {
        for original in [
            "# Project\n\nSome rules.\n",
            "",
            "only line\n",
            "trailing blank lines stay\n\n\n",
        ] {
            let with = upsert_block(original, &block()).unwrap();
            let back = remove_block(&with).unwrap().unwrap();
            assert_eq!(back, original, "round trip of {original:?}");
        }
        // Text without a final line break gets one; nothing is trimmed.
        let no_eol = upsert_block("x", &block()).unwrap();
        assert_eq!(remove_block(&no_eol).unwrap().unwrap(), "x\n");
        let middle = format!("top\n{BEGIN}\nx\n{END}\nbottom\n");
        assert_eq!(remove_block(&middle).unwrap().unwrap(), "top\nbottom\n");
        assert_eq!(remove_block("no block here\n").unwrap(), None);
    }

    #[test]
    fn crlf_files_keep_crlf() {
        let original = "# P\r\n\r\nrules\r\n";
        let with = upsert_block(original, &block()).unwrap();
        assert!(
            !with.replace("\r\n", "").contains('\n'),
            "mixed line endings"
        );
        assert_eq!(remove_block(&with).unwrap().unwrap(), original);
    }

    #[test]
    fn unbalanced_markers_are_an_error_not_a_rewrite() {
        assert!(upsert_block(&format!("{BEGIN}\nno end\n"), &block()).is_err());
        assert!(remove_block(&format!("x\n{END}\n")).is_err());
        assert!(upsert_block(&format!("{BEGIN}\n{BEGIN}\n{END}\n"), &block()).is_err());
    }

    #[test]
    fn quoted_markers_in_prose_are_not_blocks() {
        let prose = format!("The block sits between `{BEGIN}` and `{END}`.\n");
        assert_eq!(remove_block(&prose).unwrap(), None);
        let out = upsert_block(&prose, &block()).unwrap();
        assert!(out.starts_with(&prose));
        assert_eq!(remove_block(&out).unwrap().unwrap(), prose);
    }

    #[cfg(unix)]
    #[test]
    fn a_claude_md_symlink_survives_install_and_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("AGENTS.md");
        let claude = dir.path().join("CLAUDE.md");
        std::fs::write(&agents, "shared rules\n").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", &claude).unwrap();
        let is_link = |p: &std::path::Path| {
            std::fs::symlink_metadata(p)
                .unwrap()
                .file_type()
                .is_symlink()
        };

        assert_eq!(
            install_block(&claude, &block()).unwrap(),
            FileChange::Updated
        );
        assert!(is_link(&claude));
        assert!(std::fs::read_to_string(&agents).unwrap().contains(BEGIN));
        assert_eq!(uninstall_block(&claude, true).unwrap(), FileChange::Removed);
        assert!(is_link(&claude));
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), "shared rules\n");

        // Emptied through a link: the link and its target stay.
        std::fs::write(&agents, "").unwrap();
        install_block(&claude, &block()).unwrap();
        uninstall_block(&claude, true).unwrap();
        assert!(is_link(&claude));
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), "");
    }

    #[test]
    fn install_and_uninstall_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        assert_eq!(install_block(&path, &block()).unwrap(), FileChange::Created);
        assert_eq!(
            install_block(&path, &block()).unwrap(),
            FileChange::Unchanged
        );
        assert_eq!(uninstall_block(&path, true).unwrap(), FileChange::Deleted);
        assert!(!path.exists());
        assert_eq!(uninstall_block(&path, true).unwrap(), FileChange::Absent);

        std::fs::write(&path, "mine\n").unwrap();
        assert_eq!(install_block(&path, &block()).unwrap(), FileChange::Updated);
        assert_eq!(uninstall_block(&path, true).unwrap(), FileChange::Removed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
        assert_eq!(uninstall_block(&path, true).unwrap(), FileChange::Absent);
    }
}
