//! Which files an analysis indexes, and which text in them belongs to
//! myceliums itself (#151).
//!
//! [`IndexScope`] is the one predicate behind the analyzer's file walk and
//! the freshness checks (the MCP `Index:` line and the `analyze` tool's
//! re-index decision, the non-git cache check): a change to a file the
//! analyzer would not index cannot make the index stale, so it is not
//! counted. The rules are the walk's: hidden files and directories and the
//! usual build/dependency directories are skipped, the extension must map to
//! a [`SourceLanguage`], `.myceliums.toml`'s `include`/`exclude` globs and
//! `skip_patterns` apply, and the file must be within `max_file_size_kb`.
//!
//! The managed block (`<!-- BEGIN myceliums -->` … `<!-- END myceliums -->`)
//! that `myc setup-claude` / `myc setup-codex` write into `CLAUDE.md` or
//! `AGENTS.md` is myceliums' own text, not project knowledge: the analyzer
//! indexes such files with the block blanked out ([`blank_managed_blocks`]),
//! so writing or updating the block changes nothing the index holds.

use crate::config::{AnalysisSection, ProjectConfig, CONFIG_FILENAME};
use crate::error::Result;
use crate::parser::SourceLanguage;
use anyhow::Context as _;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::borrow::Cow;
use std::path::{Component, Path};

/// Default maximum file size in bytes (512 KB) applied when no config is present.
pub const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 512 * 1024;

/// Directory and file names the walk never enters, besides hidden ones
/// (a leading `.`).
const SKIPPED_NAMES: &[&str] = &[
    "node_modules",
    "__pycache__",
    "target",
    "dist",
    "build",
    "venv",
];

/// First line of the block myceliums manages in a memory file (CLAUDE.md,
/// AGENTS.md).
pub const MANAGED_BLOCK_BEGIN: &str = "<!-- BEGIN myceliums -->";
/// Last line of that block.
pub const MANAGED_BLOCK_END: &str = "<!-- END myceliums -->";

/// Key of the `index_meta` record listing the config files the resolver
/// read only because an `extends` names them (`packages/tsconfig/base.json`,
/// #169), as a JSON array of repository-relative paths. Editing one of them
/// can change the index like editing a `tsconfig.json` can.
pub const RESOLUTION_CONFIGS_META_KEY: &str = "resolution_config_files";

/// The files an analysis indexes, decided by path (and size).
#[derive(Debug, Clone)]
pub struct IndexScope {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    skip_patterns: Vec<String>,
    max_size: Option<u64>,
    /// Config files the resolver read under names that are not config
    /// names (see [`RESOLUTION_CONFIGS_META_KEY`]), forward-slashed.
    resolution_configs: std::collections::BTreeSet<String>,
}

impl Default for IndexScope {
    /// The scope without a `.myceliums.toml`.
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            // Without a config the analyzer still applies the default
            // `[analysis]` skip rules.
            skip_patterns: AnalysisSection::default().skip_patterns,
            max_size: Some(DEFAULT_MAX_FILE_SIZE_BYTES),
            resolution_configs: Default::default(),
        }
    }
}

impl IndexScope {
    /// The scope of an analysis run with `config` (`None`: no
    /// `.myceliums.toml`). Fails on an invalid glob, as the analysis does.
    pub fn new(config: Option<&ProjectConfig>) -> Result<Self> {
        let Some(cfg) = config else {
            return Ok(Self::default());
        };
        let analysis: &AnalysisSection = &cfg.analysis;
        Ok(Self {
            include: build_globset(&analysis.include)?,
            exclude: build_globset(&analysis.exclude)?,
            skip_patterns: analysis.skip_patterns.clone(),
            max_size: match analysis.max_file_size_kb {
                0 => None,
                kb => Some(kb * 1024),
            },
            resolution_configs: Default::default(),
        })
    }

    /// The same scope, also counting `paths` (repository-relative) as
    /// resolution config files: the `extends` bases an index recorded under
    /// [`RESOLUTION_CONFIGS_META_KEY`].
    pub fn with_resolution_configs(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.resolution_configs
            .extend(paths.into_iter().map(|p| p.replace('\\', "/")));
        self
    }

    /// Whether `rel` is a file the resolver reads to resolve bare JS/TS
    /// imports (#169) — `package.json`, `pnpm-workspace.yaml`, `lerna.json`,
    /// `tsconfig*.json`, `jsconfig*.json`, or a recorded `extends` base —
    /// outside the directories the walk skips. A change to one can change
    /// every edge through an import, so it calls for a full analysis.
    pub fn is_resolution_config(&self, rel: &Path) -> bool {
        let walked = rel.components().all(|component| match component {
            Component::Normal(name) => !Self::skips_name(&name.to_string_lossy()),
            Component::CurDir => true,
            _ => false,
        });
        if !walked {
            return false;
        }
        let named = rel.file_name().is_some_and(|name| {
            crate::resolver::is_resolution_config_name(&name.to_string_lossy())
        });
        named
            || self
                .resolution_configs
                .contains(&rel.to_string_lossy().replace('\\', "/"))
    }

    /// The scope of the project rooted at `root`: its `.myceliums.toml` when
    /// there is one.
    pub fn for_project(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG_FILENAME);
        if !path.exists() {
            return Ok(Self::default());
        }
        let config = ProjectConfig::load(&path)?;
        Self::new(Some(&config))
    }

    /// Whether the walk skips an entry with this file name (hidden, or a
    /// build/dependency directory). Applies to directories and files alike.
    pub fn skips_name(name: &str) -> bool {
        name.starts_with('.') || SKIPPED_NAMES.contains(&name)
    }

    /// The language the analyzer would index `rel` (relative to the analyzed
    /// directory) as, judged by its path only; `None` when it would not
    /// index it.
    pub fn language(&self, rel: &Path) -> Option<SourceLanguage> {
        for component in rel.components() {
            match component {
                Component::Normal(name) if !Self::skips_name(&name.to_string_lossy()) => {}
                Component::Normal(_) | Component::ParentDir => return None,
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        let language = self.discovers(rel)?;
        // Discovered, then skipped as minified or bundled before parsing —
        // only on the text path, as in the analyzer.
        if reads_as_text(language)
            && crate::file_guard::matches_skip_pattern(rel, &self.skip_patterns)
        {
            return None;
        }
        Some(language)
    }

    /// The language the file walk picks `rel` up as, for a path whose
    /// directories it already entered: a known extension, and the
    /// `include`/`exclude` globs.
    pub(crate) fn discovers(&self, rel: &Path) -> Option<SourceLanguage> {
        let ext = rel.extension()?.to_str()?;
        let language = SourceLanguage::from_extension(ext)?;
        if self.include.as_ref().is_some_and(|inc| !inc.is_match(rel)) {
            return None;
        }
        if self.exclude.as_ref().is_some_and(|exc| exc.is_match(rel)) {
            return None;
        }
        Some(language)
    }

    /// Whether `.myceliums.toml`'s `include`/`exclude` globs let `rel` in,
    /// whatever its extension: the config files the name-reference pass
    /// reads (#179) follow them too.
    pub fn admits(&self, rel: &Path) -> bool {
        !self.include.as_ref().is_some_and(|inc| !inc.is_match(rel))
            && !self.exclude.as_ref().is_some_and(|exc| exc.is_match(rel))
    }

    /// Whether a file of `len` bytes is within `max_file_size_kb`.
    pub fn within_size(&self, len: u64) -> bool {
        self.max_size.is_none_or(|max| len <= max)
    }

    /// Whether a change to `rel` (relative to `root`, the analyzed
    /// directory) can change the index: the analyzer would index the file —
    /// or indexed it before it was deleted — or it is `.myceliums.toml`,
    /// which decides what is indexed, or a resolution config
    /// ([`is_resolution_config`](Self::is_resolution_config)), which decides
    /// what imports bind.
    pub fn affects_index(&self, root: &Path, rel: &Path) -> bool {
        if rel == Path::new(CONFIG_FILENAME) || self.is_resolution_config(rel) {
            return true;
        }
        if self.language(rel).is_none() {
            return false;
        }
        match std::fs::metadata(root.join(rel)) {
            Ok(meta) => self.within_size(meta.len()),
            // Deleted: the index may still hold it.
            Err(_) => true,
        }
    }
}

/// Whether the analysis reads files of `lang` as text through the
/// `[analysis]` skip rules (size, line length, `skip_patterns`, UTF-8).
/// E-mail, mailbox, PDF and notebook files take their own paths and are not
/// subject to them. The analyzer routes files by this, and [`IndexScope`]
/// applies `skip_patterns` by it.
pub fn reads_as_text(lang: SourceLanguage) -> bool {
    #[cfg(feature = "pdf")]
    if lang == SourceLanguage::Pdf {
        return false;
    }
    !matches!(
        lang,
        SourceLanguage::Email | SourceLanguage::Mbox | SourceLanguage::Jupyter
    )
}

/// Whether the analysis blanks managed blocks in files of `lang` before
/// indexing them (Markdown and MDX — where `myc setup-claude` and `myc
/// setup-codex` write them). Every other file is indexed as it is, markers
/// and all, so a change between its markers is a change.
pub fn blanks_managed_blocks(lang: SourceLanguage) -> bool {
    matches!(lang, SourceLanguage::Markdown | SourceLanguage::Mdx)
}

/// The text the analysis indexes for a file of `lang` whose content is
/// `source`: managed blocks blanked where [`blanks_managed_blocks`] says so.
pub fn indexed_text(lang: SourceLanguage, source: &str) -> Cow<'_, str> {
    if blanks_managed_blocks(lang) {
        blank_managed_blocks(source)
    } else {
        Cow::Borrowed(source)
    }
}

/// File names myceliums writes a managed block into.
pub const MEMORY_FILE_NAMES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// `index_meta` key of the fingerprints of the text the analysis indexed for
/// files that hold, or may get, a managed block: a JSON object from path
/// (relative to the analyzed directory) to [`text_fingerprint`].
pub const INDEXED_TEXT_META_KEY: &str = "indexed_text_fingerprints";

/// Whether the analysis records the fingerprint of the text it indexes for
/// `rel`: a file whose managed blocks it blanks that is named like a memory
/// file or holds a block. Freshness compares against it, so a block added to
/// an uncommitted CLAUDE.md is no change either.
pub fn records_fingerprint(rel: &Path, lang: SourceLanguage, source: &str) -> bool {
    blanks_managed_blocks(lang)
        && (rel
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| MEMORY_FILE_NAMES.contains(&n))
            || source.contains(MANAGED_BLOCK_BEGIN))
}

/// Hex SHA-256 of indexed text, trailing whitespace ignored (installing a
/// block adds a blank line before it).
pub fn text_fingerprint(indexed: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(indexed.trim_end().as_bytes()))
}

/// `text` with every line of each managed block (markers included) emptied,
/// line breaks kept, so the lines around it keep their numbers. An
/// unterminated block is left as it is.
pub fn blank_managed_blocks(text: &str) -> Cow<'_, str> {
    if !text.contains(MANAGED_BLOCK_BEGIN) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut pending = String::new();
    let mut open = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if !open && trimmed == MANAGED_BLOCK_BEGIN {
            open = true;
        }
        if open {
            pending.push_str(line);
            if trimmed == MANAGED_BLOCK_END {
                open = false;
                out.extend(line_breaks(&pending));
                pending.clear();
            }
        } else {
            out.push_str(line);
        }
    }
    // A BEGIN without an END is not a block.
    out.push_str(&pending);
    Cow::Owned(out)
}

/// The line breaks of `text`, in order (`\n` or `\r\n`).
fn line_breaks(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n').filter_map(|line| {
        if line.ends_with("\r\n") {
            Some("\r\n")
        } else if line.ends_with('\n') {
            Some("\n")
        } else {
            None
        }
    })
}

/// Whether `current` and `previous` differ only in managed blocks (and
/// trailing whitespace): then the analyzer indexes the same text for both.
pub fn same_outside_managed_blocks(current: &str, previous: &str) -> bool {
    blank_managed_blocks(current).trim_end() == blank_managed_blocks(previous).trim_end()
}

/// Build a `GlobSet` from glob patterns; `None` when there are none.
fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder.add(Glob::new(pat).with_context(|| format!("Invalid glob pattern: {}", pat))?);
    }
    Ok(Some(
        builder
            .build()
            .with_context(|| "Failed to build glob set")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_and_build_paths_and_unknown_extensions_are_out_of_scope() {
        let scope = IndexScope::default();
        for path in [
            ".mcp.json",
            ".claude/settings.json",
            ".claude/agents/code-explorer.md",
            "node_modules/x/index.js",
            "a/target/gen.rs",
            "src/.hidden.ts",
            "Cargo.lock",
            "image.png",
        ] {
            assert_eq!(scope.language(Path::new(path)), None, "{path}");
        }
        for path in ["src/app.ts", "CLAUDE.md", "docs/guide.md", "lib/x.py"] {
            assert!(scope.language(Path::new(path)).is_some(), "{path}");
        }
    }

    #[test]
    fn config_globs_and_skip_patterns_apply() {
        let mut config = ProjectConfig::default();
        config.analysis.include = vec!["src/**".into()];
        config.analysis.exclude = vec!["src/gen/**".into()];
        let scope = IndexScope::new(Some(&config)).unwrap();
        assert!(scope.language(Path::new("src/a.ts")).is_some());
        assert!(scope.language(Path::new("README.md")).is_none());
        assert!(scope.language(Path::new("src/gen/a.ts")).is_none());
        assert!(scope.language(Path::new("src/vendor.min.js")).is_none());
    }

    #[test]
    fn the_config_file_and_deleted_files_affect_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let scope = IndexScope::default();
        assert!(scope.affects_index(dir.path(), Path::new(CONFIG_FILENAME)));
        assert!(scope.affects_index(dir.path(), Path::new("gone.ts")));
        assert!(!scope.affects_index(dir.path(), Path::new(".mcp.json")));
        std::fs::write(dir.path().join("big.ts"), vec![b'a'; 600 * 1024]).unwrap();
        assert!(!scope.affects_index(dir.path(), Path::new("big.ts")));
    }

    /// The files that decide what a bare JS/TS import binds count (#169):
    /// by name, and the `extends` bases an index recorded — but not inside
    /// the directories the walk skips.
    #[test]
    fn resolution_configs_affect_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let scope = IndexScope::default();
        for path in [
            "package.json",
            "packages/core/package.json",
            "pnpm-workspace.yaml",
            "lerna.json",
            "tsconfig.json",
            "apps/web/tsconfig.app.json",
            "tsconfig.base.json",
            "jsconfig.json",
        ] {
            assert!(scope.affects_index(dir.path(), Path::new(path)), "{path}");
        }
        for path in [
            "node_modules/x/package.json",
            ".changeset/package.json",
            "dist/package.json",
            "packages/tsconfig/base.json",
            "data.json",
        ] {
            assert!(!scope.affects_index(dir.path(), Path::new(path)), "{path}");
        }
        let recorded = IndexScope::default()
            .with_resolution_configs(["packages/tsconfig/base.json".to_string()]);
        assert!(recorded.affects_index(dir.path(), Path::new("packages/tsconfig/base.json")));
        assert!(!recorded.affects_index(dir.path(), Path::new("data.json")));
    }

    #[test]
    fn managed_blocks_are_blanked_line_for_line() {
        let text = "# Notes\n\n<!-- BEGIN myceliums -->\nrouting\n<!-- END myceliums -->\nafter\n";
        let blanked = blank_managed_blocks(text);
        assert_eq!(blanked, "# Notes\n\n\n\n\nafter\n");
        assert_eq!(blanked.lines().count(), text.lines().count());

        let crlf = "a\r\n<!-- BEGIN myceliums -->\r\nx\r\n<!-- END myceliums -->\r\nb\r\n";
        assert_eq!(blank_managed_blocks(crlf), "a\r\n\r\n\r\n\r\nb\r\n");

        // Unterminated: not a block.
        let open = "a\n<!-- BEGIN myceliums -->\nb\n";
        assert_eq!(blank_managed_blocks(open), open);
        // Quoted in prose: not a marker line.
        let quoted = "use `<!-- BEGIN myceliums -->` markers\n";
        assert_eq!(blank_managed_blocks(quoted), quoted);
    }

    #[test]
    fn only_markdown_blanks_blocks_and_skip_patterns_follow_the_text_path() {
        let text = "a\n<!-- BEGIN myceliums -->\nx\n<!-- END myceliums -->\n";
        assert_eq!(indexed_text(SourceLanguage::Markdown, text), "a\n\n\n\n");
        for lang in [
            SourceLanguage::PlainText,
            SourceLanguage::TypeScript,
            SourceLanguage::Vue,
        ] {
            assert_eq!(indexed_text(lang, text), text, "{lang:?}");
        }
        // The analyzer indexes a notebook or mailbox whatever its name.
        let mut config = ProjectConfig::default();
        config.analysis.skip_patterns = vec!["min.js".into(), "ipynb".into(), "eml".into()];
        let scope = IndexScope::new(Some(&config)).unwrap();
        assert!(scope.language(Path::new("analysis.ipynb")).is_some());
        assert!(scope.language(Path::new("mail/hello.eml")).is_some());
        assert!(scope.language(Path::new("src/vendor.min.js")).is_none());
    }

    #[test]
    fn memory_files_and_files_with_a_block_record_a_fingerprint() {
        let md = SourceLanguage::Markdown;
        assert!(records_fingerprint(Path::new("CLAUDE.md"), md, "notes"));
        assert!(records_fingerprint(Path::new("sub/AGENTS.md"), md, ""));
        assert!(records_fingerprint(
            Path::new("docs/x.md"),
            md,
            "<!-- BEGIN myceliums -->"
        ));
        assert!(!records_fingerprint(Path::new("docs/x.md"), md, "notes"));
        assert!(!records_fingerprint(
            Path::new("x.txt"),
            SourceLanguage::PlainText,
            "<!-- BEGIN myceliums -->"
        ));
        assert_eq!(text_fingerprint("a\n\n"), text_fingerprint("a"));
        assert_ne!(text_fingerprint("a"), text_fingerprint("b"));
    }

    #[test]
    fn an_appended_block_is_no_change_but_an_edit_outside_it_is() {
        let before = "# Project\n\nNotes.\n";
        let block = "<!-- BEGIN myceliums -->\nrouting\n<!-- END myceliums -->\n";
        let after = format!("{before}\n{block}");
        assert!(same_outside_managed_blocks(&after, before));
        // A new file holding only the block.
        assert!(same_outside_managed_blocks(block, ""));
        let edited = format!("# Project\n\nOther notes.\n\n{block}");
        assert!(!same_outside_managed_blocks(&edited, before));
    }
}
