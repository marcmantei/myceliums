//! The routing block: a short, versioned instruction block that `myc
//! setup-claude` keeps in a project's `.claude/rules/myceliums.md` (at user
//! scope in `~/.claude/CLAUDE.md`) and `myc setup-codex` in its AGENTS.md,
//! between `<!-- BEGIN myceliums -->` and `<!-- END myceliums -->` (#126,
//! layer 1). Where a block may go without hiding the project's own
//! instructions: [`super::instructions`] (#200).
//!
//! The block tells the agent which questions go to the code graph and which
//! stay with grep. It is loaded on every turn, so it is kept under 15 lines.
//! Everything between the markers belongs to myceliums: installing replaces
//! it and uninstalling removes it, as long as it reads exactly as some version
//! wrote it ([`obstacle`]). Text outside the markers is never touched.

use std::path::Path;

use anyhow::{bail, Context as _, Result};

/// First line of the managed block. Shared with the analyzer, which leaves
/// the block out of the index (#151).
pub const BEGIN: &str = myceliums_core::index_scope::MANAGED_BLOCK_BEGIN;
/// Last line of the managed block.
pub const END: &str = myceliums_core::index_scope::MANAGED_BLOCK_END;
/// Version of the block's wording; bump it when the text changes.
pub const ROUTING_VERSION: u32 = 4;

/// Which agent the block is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Claude Code; mentions the `code-explorer` subagent when it is
    /// installed.
    Claude {
        /// Whether `.claude/agents/code-explorer.md` is installed alongside.
        subagent: bool,
    },
    /// AGENTS.md for Codex and other agents that read it.
    Agents,
}

/// The block, markers included, with `\n` line endings.
pub fn routing_block(flavor: Flavor) -> String {
    render(ROUTING_VERSION, flavor)
}

/// The block as version `version` of it read. Earlier versions are kept so
/// that a block someone edited can be told from one we wrote
/// ([`is_unedited`]).
fn render(version: u32, flavor: Flavor) -> String {
    let setup = match flavor {
        Flavor::Claude { .. } => "myc setup-claude",
        Flavor::Agents => "myc setup-codex",
    };
    let also = match flavor {
        Flavor::Claude { subagent: true } => {
            "The myceliums MCP tools and the `code-explorer` subagent answer the same questions."
        }
        _ if version < 2 => "The myceliums MCP tools (`get_callers`, `get_callees`, `context_search`) answer the same questions.",
        _ => "The myceliums MCP tools (`find_callers`, `find_callees`, `impact_of_change`, `search_code`) answer the same questions.",
    };
    let heuristic = if version < 3 {
        ""
    } else {
        "Graph answers are heuristic: when completeness matters, or a result says `Incomplete` or lists calls without an edge, read the source before answering.\n"
    };
    let edits = if version < 4 {
        format!("edits inside these markers are overwritten; `{setup} --uninstall` removes them")
    } else {
        format!("it updates this block and `{setup} --uninstall` removes it, unless the block was edited; put your own text outside the markers")
    };
    format!(
        "{BEGIN}\n\
         <!-- v{version}, written by `{setup}`: {edits} -->\n\
         ## Code navigation: myceliums\n\
         Structural questions go to the myceliums code graph first, not grep/rg:\n\
         - callers / callees of X: `myc callers X`, `myc callees X` (`--depth 3` for chains)\n\
         - what breaks if X or a file changes: `myc impact X`, `myc impact path/to/file`\n\
         - where or how something is implemented: `myc context \"<question>\"`\n\
         {also}\n\
         {heuristic}\
         grep/rg stay right for string literals, config values, log messages and non-code files.\n\
         {END}\n"
    )
}

/// Whether `block` (markers included) is, byte for byte, a text that some
/// version of `myc` wrote, in a file with the line breaks of `block`.
fn is_shipped(block: &str) -> bool {
    (1..=ROUTING_VERSION).any(|version| {
        [
            Flavor::Claude { subagent: true },
            Flavor::Claude { subagent: false },
            Flavor::Agents,
        ]
        .into_iter()
        .any(|flavor| {
            let text = render(version, flavor);
            text == block || text.replace('\n', "\r\n") == block
        })
    })
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
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += line.len();
        let n = i + 1;
        match line.trim() {
            BEGIN if open.is_some() => {
                bail!("line {n} has a second `{BEGIN}` before `{END}`")
            }
            BEGIN => open = Some(start),
            END => match open.take() {
                Some(begin) => ranges.push((begin, offset)),
                None => bail!("line {n} has `{END}` without a `{BEGIN}`"),
            },
            _ => {}
        }
    }
    if let Some(start) = open {
        let n = text[..start].matches('\n').count() + 1;
        bail!("line {n} has `{BEGIN}` without a matching `{END}`");
    }
    Ok(ranges)
}

/// `text` with the managed block set to `block`.
///
/// An existing block is replaced in place;
/// otherwise the block is appended, after a blank line when the text ends
/// with a line break. CRLF files get CRLF.
pub fn upsert_block(text: &str, block: &str) -> Result<String> {
    let block = if text.contains("\r\n") {
        block.replace('\n', "\r\n")
    } else {
        block.to_string()
    };
    let ranges = block_ranges(text)?;
    if ranges.is_empty() {
        // A blank line separates the block from the text before it. Text
        // without a final line break gets that break alone, so that
        // `remove_block` can tell the two and restore either.
        let mut out = text.to_string();
        if !out.is_empty() {
            out.push_str(newline_of(text));
        }
        out.push_str(&block);
        return Ok(out);
    }
    let [(start, end)] = ranges[..] else {
        bail!(
            "found {} myceliums blocks; leave one, then re-run",
            ranges.len()
        );
    };
    Ok(format!("{}{block}{}", &text[..start], &text[end..]))
}

/// Which managed blocks to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Every block.
    Any,
    /// Every block but one `myc setup-codex` wrote.
    Claude,
}

/// A managed block in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Its first line, from 1: where to look, never what to delete.
    pub line: usize,
    /// Whether it reads, byte for byte, as some version of `myc` wrote it.
    pub shipped: bool,
    /// [`Owner::Any`]: written by `myc setup-codex`.
    pub owner: Owner,
}

/// What to do by hand about `blocks`. The step names the markers, not line
/// numbers, so that it is safe to follow twice, in any order and under every
/// name of the file. Text that is not as `myc` wrote it may hold the user's
/// own, so that step names no range at all.
pub fn manual_step(blocks: &[Block]) -> String {
    if blocks.iter().all(|block| block.shipped) {
        format!(
            "delete everything from the line `{BEGIN}` to the next line `{END}`, both included; if there are several such blocks, each"
        )
    } else {
        format!(
            "the text between `{BEGIN}` and `{END}` is not as myc wrote it and may hold your own: {COMPARE_STEP}"
        )
    }
}

/// The step for a block that cannot be told from the user's text.
pub const COMPARE_STEP: &str = "compare it with a block that myc writes (`myc setup-claude` in an empty git project) and remove by hand what is not yours";

/// The managed blocks in `text`; an error when the markers do not match.
pub fn blocks(text: &str) -> Result<Vec<Block>> {
    let line = |offset: usize| text[..offset].matches('\n').count() + 1;
    Ok(block_ranges(text)?
        .into_iter()
        .map(|(start, end)| {
            let block = &text[start..end];
            Block {
                line: line(start),
                shipped: is_shipped(block),
                owner: if Owner::Claude.owns(block) {
                    Owner::Claude
                } else {
                    Owner::Any
                },
            }
        })
        .collect())
}

impl Owner {
    fn owns(self, block: &str) -> bool {
        match self {
            Owner::Any => true,
            Owner::Claude => !block.contains("written by `myc setup-codex`"),
        }
    }
}

/// `text` without the managed blocks of `owner`, or `None` when it holds
/// none. The line break that [`upsert_block`] put before an appended block
/// goes too.
pub fn remove_block(text: &str, owner: Owner) -> Result<Option<String>> {
    let mut ranges = block_ranges(text)?;
    ranges.retain(|(start, end)| owner.owns(&text[*start..*end]));
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
        // added, and nothing else of the user's text.
        let nl = newline_of(text);
        if out.ends_with(nl) {
            out.truncate(out.len() - nl.len());
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

/// Why the file at `path` (or what it links to) is not one to write to: it has
/// several hard links, which a write would separate, is read-only, or holds
/// markers that do not match, several blocks, or a block that is not as we
/// wrote it. `None` for a file that does not exist.
pub fn obstacle(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if hard_links(&meta) > 1 {
        return Some(format!("it has {} hard links", hard_links(&meta)));
    }
    // On Windows the read-only attribute of a directory says nothing.
    let dir = path.parent().filter(|_| cfg!(unix));
    let dir = dir.and_then(|dir| std::fs::metadata(dir).ok());
    if meta.permissions().readonly() || dir.is_some_and(|dir| dir.permissions().readonly()) {
        return Some("it is read-only".to_string());
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return Some(format!("it cannot be read ({e})")),
    };
    let ranges = match block_ranges(&text) {
        Ok(ranges) => ranges,
        Err(e) => return Some(format!("{e:#}")),
    };
    if ranges.len() > 1 {
        return Some(format!("it holds {} myceliums blocks", ranges.len()));
    }
    let [(start, end)] = ranges[..] else {
        return None;
    };
    (!is_shipped(&text[start..end])).then(|| {
        "the text between the markers was edited or reformatted, or is of a newer myc".to_string()
    })
}

#[cfg(unix)]
fn hard_links(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.nlink()
}

/// Not known on this platform.
#[cfg(not(unix))]
fn hard_links(_meta: &std::fs::Metadata) -> u64 {
    1
}

/// [`obstacle`] as an error, so that nothing is ever written past one.
fn refuse(path: &Path) -> Result<()> {
    match obstacle(path) {
        Some(reason) => bail!("{} was not changed: {reason}", path.display()),
        None => Ok(()),
    }
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
    refuse(path)?;
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

/// Removes the managed blocks of `owner` from the file at `path`. When
/// `delete_if_empty` is set and nothing but whitespace remains, the file is
/// deleted.
pub fn uninstall_block(path: &Path, owner: Owner, delete_if_empty: bool) -> Result<FileChange> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileChange::Absent),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };
    let Some(rest) = remove_block(&text, owner).with_context(|| path.display().to_string())? else {
        return Ok(FileChange::Absent);
    };
    refuse(path)?;
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
        // A file without a trailing newline gets just that.
        assert!(upsert_block("x", &block())
            .unwrap()
            .starts_with("x\n<!-- BEGIN"));
    }

    #[test]
    fn upsert_replaces_an_old_block_in_place_and_keeps_surrounding_text() {
        let text = format!("top\n\n{BEGIN}\nold v0 text\n{END}\n\nbottom\n");
        let out = upsert_block(&text, &block()).unwrap();
        assert!(out.starts_with("top\n\n<!-- BEGIN myceliums -->\n"));
        assert!(out.ends_with(&format!("{END}\n\nbottom\n")));
        assert!(!out.contains("old v0 text"));
        // Two blocks are the user's to sort out.
        let dup = format!("{BEGIN}\na\n{END}\nmid\n{BEGIN}\nb\n{END}\n");
        assert!(upsert_block(&dup, &block()).is_err());
    }

    #[test]
    fn remove_restores_the_original_text() {
        for original in [
            "# Project\n\nSome rules.\n",
            "",
            "only line\n",
            "no final line break",
            "\n",
            "crlf\r\n",
            "crlf\r\nno final line break",
            "trailing blank lines stay\n\n\n",
        ] {
            let with = upsert_block(original, &block()).unwrap();
            let back = remove_block(&with, Owner::Any).unwrap().unwrap();
            assert_eq!(back, original, "round trip of {original:?}");
        }
        // A block of an older version: those added a final line break to text
        // without one, and a blank line.
        let old = format!("x\n\n{}", block());
        assert_eq!(remove_block(&old, Owner::Any).unwrap().unwrap(), "x\n");
        let middle = format!("top\n{BEGIN}\nx\n{END}\nbottom\n");
        assert_eq!(
            remove_block(&middle, Owner::Any).unwrap().unwrap(),
            "top\nbottom\n"
        );
        assert_eq!(remove_block("no block here\n", Owner::Any).unwrap(), None);
    }

    #[test]
    fn crlf_files_keep_crlf() {
        let original = "# P\r\n\r\nrules\r\n";
        let with = upsert_block(original, &block()).unwrap();
        assert!(
            !with.replace("\r\n", "").contains('\n'),
            "mixed line endings"
        );
        assert_eq!(remove_block(&with, Owner::Any).unwrap().unwrap(), original);
    }

    #[test]
    fn unbalanced_markers_are_an_error_not_a_rewrite() {
        assert!(upsert_block(&format!("{BEGIN}\nno end\n"), &block()).is_err());
        assert!(remove_block(&format!("x\n{END}\n"), Owner::Any).is_err());
        assert!(upsert_block(&format!("{BEGIN}\n{BEGIN}\n{END}\n"), &block()).is_err());
    }

    #[test]
    fn quoted_markers_in_prose_are_not_blocks() {
        let prose = format!("The block sits between `{BEGIN}` and `{END}`.\n");
        assert_eq!(remove_block(&prose, Owner::Any).unwrap(), None);
        let out = upsert_block(&prose, &block()).unwrap();
        assert!(out.starts_with(&prose));
        assert_eq!(remove_block(&out, Owner::Any).unwrap().unwrap(), prose);
    }

    #[test]
    fn every_version_of_the_block_is_known_and_edits_are_not() {
        for version in 1..=ROUTING_VERSION {
            for flavor in [
                Flavor::Claude { subagent: true },
                Flavor::Claude { subagent: false },
                Flavor::Agents,
            ] {
                let text = render(version, flavor);
                assert!(is_shipped(&text) && is_shipped(&text.replace('\n', "\r\n")));
                assert!(!is_shipped(text.trim_end()));
                assert!(!is_shipped(&text.replacen('\n', "\r\n", 1)));
                assert!(!is_shipped(&text.replace("grep/rg stay", "grep stays")));
                assert!(!is_shipped(&text.replace("- what", "* what")));
            }
        }
        // What was written in fact, word for word.
        let v1 = render(1, Flavor::Agents);
        assert!(v1.contains("`context_search`") && !v1.contains("heuristic"));
        let v2 = render(2, Flavor::Agents);
        assert!(v2.contains("`search_code`") && !v2.contains("heuristic"));
        // As `myc setup-claude` at f472dae wrote it, copied.
        let v3 = include_str!("../../tests/data/routing_block_v3.md");
        assert_eq!(render(3, Flavor::Claude { subagent: true }), v3);
    }

    #[test]
    fn blocks_are_found_with_the_line_to_look_at() {
        let claude = routing_block(Flavor::Claude { subagent: true });
        let codex = routing_block(Flavor::Agents);
        let text = format!("a\n\n{claude}b\n{codex}");
        let n = claude.lines().count();
        let found = blocks(&text).unwrap();
        assert_eq!((found[0].line, found[0].shipped), (3, true));
        assert_eq!(found[0].owner, Owner::Claude);
        assert_eq!((found[1].line, found[1].shipped), (n + 4, true));
        assert_eq!(found[1].owner, Owner::Any);
        assert!(blocks(&format!("{BEGIN}\n")).is_err());
    }

    /// Setup says no and leaves the file as it is.
    #[test]
    fn a_file_with_an_obstacle_is_refused() {
        let edited = block().replace(END, "- mine\n<!-- END myceliums -->");
        let cases = [
            (format!("rules\n\n{edited}"), "edited"),
            (format!("{}{}", block(), block()), "2 myceliums blocks"),
            (format!("{BEGIN}\nno end\n"), "without a matching"),
            (render(1, Flavor::Agents), "read-only"),
        ];
        for (text, reason) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("AGENTS.md");
            std::fs::write(&path, &text).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(reason == "read-only");
            std::fs::set_permissions(&path, permissions).unwrap();
            assert!(obstacle(&path).unwrap().contains(reason), "{reason}");
            assert!(install_block(&path, &block()).is_err(), "{reason}");
            assert!(
                uninstall_block(&path, Owner::Any, true).is_err(),
                "{reason}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        assert_eq!(obstacle(Path::new("/no/such/file.md")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_linked_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("AGENTS.md"), dir.path().join("CLAUDE.md"));
        std::fs::write(&a, "rules\n").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        for path in [&a, &b] {
            assert_eq!(obstacle(path).unwrap(), "it has 2 hard links");
            assert!(install_block(path, &block()).is_err());
        }
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "rules\n");
    }

    #[test]
    fn a_block_of_setup_codex_is_not_claudes() {
        let codex = upsert_block("rules\n", &routing_block(Flavor::Agents)).unwrap();
        assert_eq!(remove_block(&codex, Owner::Claude).unwrap(), None);
        assert_eq!(
            remove_block(&codex, Owner::Any).unwrap().unwrap(),
            "rules\n"
        );
        for subagent in [true, false] {
            let claude =
                upsert_block("rules\n", &routing_block(Flavor::Claude { subagent })).unwrap();
            assert_eq!(
                remove_block(&claude, Owner::Claude).unwrap().unwrap(),
                "rules\n"
            );
        }
        // A block without the line that says who wrote it is Claude's.
        let bare = format!("rules\n\n{BEGIN}\nold\n{END}\n");
        assert_eq!(
            remove_block(&bare, Owner::Claude).unwrap().unwrap(),
            "rules\n"
        );
        // Both in one file: only Claude's goes.
        let both = format!("{BEGIN}\nold\n{END}\n{codex}");
        assert_eq!(remove_block(&both, Owner::Claude).unwrap().unwrap(), codex);
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
        assert_eq!(
            uninstall_block(&claude, Owner::Any, true).unwrap(),
            FileChange::Removed
        );
        assert!(is_link(&claude));
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), "shared rules\n");

        // Emptied through a link: the link and its target stay.
        std::fs::write(&agents, "").unwrap();
        install_block(&claude, &block()).unwrap();
        uninstall_block(&claude, Owner::Any, true).unwrap();
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
        assert_eq!(
            uninstall_block(&path, Owner::Any, true).unwrap(),
            FileChange::Deleted
        );
        assert!(!path.exists());
        assert_eq!(
            uninstall_block(&path, Owner::Any, true).unwrap(),
            FileChange::Absent
        );

        std::fs::write(&path, "mine\n").unwrap();
        assert_eq!(install_block(&path, &block()).unwrap(), FileChange::Updated);
        assert_eq!(
            uninstall_block(&path, Owner::Any, true).unwrap(),
            FileChange::Removed
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
        assert_eq!(
            uninstall_block(&path, Owner::Any, true).unwrap(),
            FileChange::Absent
        );
    }
}
