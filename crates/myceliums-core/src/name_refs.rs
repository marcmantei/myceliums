//! References by qualified name (#179): `REFERENCES` edges from the places
//! that name a project symbol in a string or a config file to that symbol.
//!
//! Code often reaches a class without calling it: `Class.forName("a.b.C")`,
//! `@ConditionalOnClass(name = "a.b.C")`, Django's
//! `import_string(settings.X)` with `X = "pkg.mod.Name"` in the settings, a
//! Spring `META-INF/spring.factories` or `*.imports` file, a ServiceLoader
//! file in `META-INF/services/`, an XML bean's `class="a.b.C"`, a
//! properties or YAML value. The call graph has no edge for these; this
//! module adds one.
//!
//! **Precision first.** A name links only when it is, exactly and
//! completely, the qualified name of a project symbol:
//!
//! - in code, the whole string literal is the name (`"a.b.C"`, not
//!   `"see a.b.C"`); comments are skipped;
//! - in a config file, a whole dotted token is the name: the longest run of
//!   identifiers and separators, so `logging.level.a.b.C` is not `a.b.C`;
//!   comments are skipped;
//! - the name must include the package (Java, Kotlin: the `package`
//!   declaration) or the module path (Python: the file's path from the
//!   outermost directory with an `__init__.py`); a short name, a partial
//!   path or a package name never links;
//! - Java and Kotlin: a type by its canonical (`a.b.Outer.Inner`) or binary
//!   (`a.b.Outer$Inner`) name, a member only as `a.b.Type#member`; Python:
//!   a class, function or method (`pkg.mod.Class.method`, also the
//!   entry-point form `pkg.mod:func`);
//! - a string in code links only to a symbol of its own language family;
//!   a config file to either;
//! - a name that more than [`MAX_TARGETS`] symbols carry links none.
//!
//! The edge starts at the innermost symbol around the string, else at the
//! file (a module-level Python setting, a config file). Config files that
//! name a symbol are recorded as file nodes; other config files are not
//! indexed. The edge metadata ([`NameRefMetadata`]) holds the line, the name
//! as written and the line's text.

use myceliums_storage::{CodeSymbol, FileNode, Relationship, RelationshipKind, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::parser::SourceLanguage;

/// `extraction_method` of a name-reference edge.
pub const METHOD: &str = "name-ref";

/// A name carried by more symbols than this links none.
pub const MAX_TARGETS: usize = 8;

/// The longest string literal or token considered a name.
const MAX_NAME_LEN: usize = 300;

/// The longest line text kept in the edge metadata.
const MAX_TEXT_LEN: usize = 120;

/// Language families whose qualified names this module knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Java and Kotlin: package-qualified types, `Type#member`.
    Jvm,
    /// Python: module-qualified classes, functions and methods.
    Python,
}

impl Family {
    /// The family of a source language, `None` for the others.
    pub fn of_language(lang: SourceLanguage) -> Option<Self> {
        match lang {
            SourceLanguage::Java | SourceLanguage::Kotlin => Some(Family::Jvm),
            SourceLanguage::Python => Some(Family::Python),
            _ => None,
        }
    }

    /// The family of an indexed file, by its extension.
    fn of_path(path: &str) -> Option<Self> {
        let ext = Path::new(path).extension()?.to_str()?;
        Family::of_language(SourceLanguage::from_extension(ext)?)
    }
}

/// What kind of file a config/resource file is, which decides its comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// `*.properties`, `spring.factories`: `#`/`!` comment lines.
    Properties,
    /// `META-INF/spring/*.imports`, `META-INF/services/*`: one name per
    /// line, `#` comments.
    NameList,
    /// `*.yml`, `*.yaml`: `#` comments.
    Yaml,
    /// `*.xml`: `<!-- -->` comments.
    Xml,
    /// `*.toml`, `*.cfg`, `*.ini`: `#`/`;` comment lines.
    Ini,
}

/// The kind of a config/resource file whose names this module reads, by
/// its path (relative to the repository); `None` for any other file.
pub fn resource_kind(rel: &Path) -> Option<ResourceKind> {
    let name = rel.file_name()?.to_str()?;
    let in_meta_inf = |dir: &str| {
        let mut parents = rel.parent()?.components().rev();
        let last = parents.next()?.as_os_str().to_str()?;
        let before = parents.next()?.as_os_str().to_str()?;
        Some(last == dir && before == "META-INF")
    };
    if name == "spring.factories" {
        return Some(ResourceKind::Properties);
    }
    if in_meta_inf("services") == Some(true) {
        return Some(ResourceKind::NameList);
    }
    let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "imports" if in_meta_inf("spring") == Some(true) => Some(ResourceKind::NameList),
        "properties" => Some(ResourceKind::Properties),
        "yml" | "yaml" => Some(ResourceKind::Yaml),
        "xml" => Some(ResourceKind::Xml),
        "toml" | "cfg" | "ini" => Some(ResourceKind::Ini),
        _ => None,
    }
}

/// One place that may name a symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// 1-based line; 0 for a name the file's own name carries
    /// (`META-INF/services/a.b.Service`).
    pub line: u32,
    /// The name as written.
    pub name: String,
}

/// Where a file's candidates come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// String literals of a source file of this family.
    Code(Family),
    /// Tokens of a config/resource file.
    Config(ResourceKind),
}

/// A file's candidates.
#[derive(Debug, Clone)]
pub struct FileCandidates {
    /// Repository-relative path.
    pub path: String,
    /// What the candidates are.
    pub origin: Origin,
    /// The candidates, in file order.
    pub candidates: Vec<Candidate>,
    /// The text of each candidate's line, trimmed and cut (same order).
    pub texts: Vec<String>,
}

/// Metadata of a name-reference (`REFERENCES`) edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRefMetadata {
    /// Always [`METHOD`].
    pub extraction_method: String,
    /// The file that names the target.
    pub file: String,
    /// The first line that names it there (0: the file's name).
    pub line: u32,
    /// The name as written.
    pub name: String,
    /// That line's text, trimmed and cut.
    pub text: String,
    /// `string` (a literal in code) or `config`.
    pub origin: String,
    /// How many times the source names the target.
    pub count: u32,
}

impl NameRefMetadata {
    /// The metadata of `rel`, when it is a name-reference edge.
    pub fn of(rel: &Relationship) -> Option<Self> {
        if rel.kind != RelationshipKind::References || !rel.metadata.contains(METHOD) {
            return None;
        }
        serde_json::from_str::<Self>(&rel.metadata)
            .ok()
            .filter(|m| m.extraction_method == METHOD)
    }
}

// --- Scanning ---------------------------------------------------------------

/// The string literals of a source file that may be qualified names, with
/// their lines; empty for a language whose names this module does not know.
pub fn code_candidates(source: &str, lang: SourceLanguage) -> Vec<Candidate> {
    let Some(family) = Family::of_language(lang) else {
        return Vec::new();
    };
    let literals = match family {
        Family::Jvm => c_like_literals(source),
        Family::Python => python_literals(source),
    };
    literals
        .into_iter()
        .filter(|(_, text)| is_code_name(text, family))
        .map(|(line, text)| Candidate {
            line,
            name: text.to_string(),
        })
        .collect()
}

/// A source file's candidates ([`code_candidates`]), `None` when it has
/// none.
pub fn file_candidates(rel: &str, lang: SourceLanguage, source: &str) -> Option<FileCandidates> {
    let family = Family::of_language(lang)?;
    let candidates = code_candidates(source, lang);
    if candidates.is_empty() {
        return None;
    }
    let texts = texts_of(source, &candidates);
    Some(FileCandidates {
        path: rel.to_string(),
        origin: Origin::Code(family),
        candidates,
        texts,
    })
}

/// The package a Java/Kotlin file declares, from its parsed imports (the
/// parsers report `package a.b` as an import named `package`).
pub fn declared_package(imports: &[crate::parser::ImportInfo]) -> Option<&str> {
    imports
        .iter()
        .find(|i| i.original_name.as_deref() == Some("package"))
        .map(|i| i.source_module.as_str())
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Every line's text for `candidates` of `source`.
pub fn texts_of(source: &str, candidates: &[Candidate]) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    candidates
        .iter()
        .map(|c| match c.line {
            0 => "(file name)".to_string(),
            n => cut(
                lines.get(n as usize - 1).copied().unwrap_or("").trim(),
                MAX_TEXT_LEN,
            ),
        })
        .collect()
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `text` is a dotted run of identifiers, `seps` the separators
/// allowed between them, at least two identifiers.
fn is_chain(text: &str, seps: &[u8]) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NAME_LEN {
        return false;
    }
    let mut parts = 0;
    let mut at_start = true;
    for &b in bytes {
        if at_start {
            if !is_ident_start(b) {
                return false;
            }
            at_start = false;
            parts += 1;
        } else if seps.contains(&b) {
            at_start = true;
        } else if !is_ident_char(b) {
            return false;
        }
    }
    !at_start && parts >= 2
}

/// Whether a whole string literal of `family` code has a qualified name's
/// form: `a.b.C`, `a.b.Outer$Inner`, `a.b.C#member` (JVM); `a.b.c`,
/// `a.b:c` (Python).
fn is_code_name(text: &str, family: Family) -> bool {
    match family {
        Family::Jvm => match text.split_once('#') {
            Some((ty, member)) => is_chain(ty, b".$") && is_ident(member),
            None => is_chain(text, b".$"),
        },
        Family::Python => match text.split_once(':') {
            Some((module, attr)) => {
                (is_chain(module, b".") || is_ident(module)) && is_dotted_or_ident(attr)
            }
            None => is_chain(text, b"."),
        },
    }
}

fn is_ident(text: &str) -> bool {
    let b = text.as_bytes();
    !b.is_empty() && is_ident_start(b[0]) && b.iter().all(|&c| is_ident_char(c))
}

fn is_dotted_or_ident(text: &str) -> bool {
    is_ident(text) || is_chain(text, b".")
}

/// String literals of Java/Kotlin source: `"…"` and text blocks `"""…"""`;
/// comments and character literals skipped. `(line, content)` with the
/// line the literal starts on.
fn c_like_literals(source: &str) -> Vec<(u32, &str)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    if bytes[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'"' if bytes[i..].starts_with(b"\"\"\"") => {
                let start_line = line;
                let start = i + 3;
                i = start;
                while i < bytes.len() && !bytes[i..].starts_with(b"\"\"\"") {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    if bytes.get(i) == Some(&b'\n') {
                        line += 1;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                if let Some(text) = source.get(start..end) {
                    out.push((start_line, text.trim()));
                }
                i += 3;
            }
            quote @ (b'"' | b'\'') => {
                let start = i + 1;
                i = start;
                let mut escaped = false;
                while i < bytes.len() && bytes[i] != quote && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' {
                        escaped = true;
                        i += 1;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                if quote == b'"' && !escaped {
                    if let Some(text) = source.get(start..end) {
                        out.push((line, text));
                    }
                }
                if i < bytes.len() && bytes[i] == quote {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// String literals of Python source, with prefixes (`r`, `u`; `f` and `b`
/// literals are skipped) and triple quotes; `#` comments skipped.
fn python_literals(source: &str) -> Vec<(u32, &str)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            quote @ (b'"' | b'\'') => {
                // The prefix: the letters right before the quote.
                let mut p = i;
                while p > 0 && bytes[p - 1].is_ascii_alphabetic() && i - p < 3 {
                    p -= 1;
                }
                let prefix = &source[p..i];
                let identifier_before = p > 0 && is_ident_char(bytes[p - 1]);
                let raw = prefix.contains(['r', 'R']);
                let skipped = identifier_before || prefix.contains(['f', 'F', 'b', 'B']);
                let triple = bytes[i..].starts_with(&[quote, quote, quote]);
                let start_line = line;
                let start = i + if triple { 3 } else { 1 };
                i = start;
                loop {
                    if i >= bytes.len() {
                        break;
                    }
                    let b = bytes[i];
                    if b == b'\\' && !raw {
                        if bytes.get(i + 1) == Some(&b'\n') {
                            line += 1;
                        }
                        i += 2;
                        continue;
                    }
                    if triple {
                        if bytes[i..].starts_with(&[quote, quote, quote]) {
                            break;
                        }
                    } else if b == quote || b == b'\n' {
                        break;
                    }
                    if b == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                let closed = end < bytes.len() && bytes[end] == quote;
                if !skipped && closed {
                    if let Some(text) = source.get(start..end) {
                        out.push((start_line, if triple { text.trim() } else { text }));
                    }
                }
                // An unclosed literal ends at its line's newline, which is
                // left for the line count.
                i = match (closed, triple) {
                    (true, true) => end + 3,
                    (true, false) => end + 1,
                    (false, _) => end,
                };
            }
            _ => i += 1,
        }
    }
    out
}

/// The names a config/resource file may carry: every whole dotted token
/// outside comments, and for a ServiceLoader file its own name.
pub fn config_candidates(rel: &Path, source: &str, kind: ResourceKind) -> Vec<Candidate> {
    let mut out = Vec::new();
    if kind == ResourceKind::NameList {
        if let Some(name) = rel.file_name().and_then(|n| n.to_str()) {
            let in_services = rel
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|d| d == "services");
            if in_services && is_chain(name, b".$") {
                out.push(Candidate {
                    line: 0,
                    name: name.to_string(),
                });
            }
        }
    }
    let text = match kind {
        ResourceKind::Xml => blank_xml_comments(source),
        _ => std::borrow::Cow::Borrowed(source),
    };
    for (index, line) in text.lines().enumerate() {
        let body = match kind {
            ResourceKind::Properties | ResourceKind::NameList | ResourceKind::Ini => {
                let t = line.trim_start();
                if t.starts_with('#') || t.starts_with('!') || t.starts_with(';') {
                    continue;
                }
                match kind {
                    // `a.b.C # note` in a name list.
                    ResourceKind::NameList => t.split('#').next().unwrap_or(""),
                    _ => line,
                }
            }
            ResourceKind::Yaml => {
                let t = line.trim_start();
                if t.starts_with('#') {
                    continue;
                }
                line.split(" #").next().unwrap_or("")
            }
            ResourceKind::Xml => line,
        };
        for name in tokens(body) {
            out.push(Candidate {
                line: index as u32 + 1,
                name: name.to_string(),
            });
        }
    }
    out
}

/// `source` with every `<!-- … -->` comment blanked out, newlines kept.
fn blank_xml_comments(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains("<!--") {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find("-->").map_or(after.len(), |e| e + 3);
        out.extend(
            after[..end]
                .chars()
                .map(|c| if c == '\n' { '\n' } else { ' ' }),
        );
        rest = &after[end..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// The whole dotted tokens of a config line: maximal runs of identifiers
/// joined by `.` or `$`, with an optional `:attr` (Python entry points),
/// of at least two identifiers.
fn tokens(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !is_ident_start(b) {
            i += 1;
            continue;
        }
        // Part of a longer word (`1a.b`) or of a path (`x/a.b`).
        let start = i;
        let preceded = start > 0 && matches!(bytes[start - 1], b'0'..=b'9' | b'/' | b'\\');
        let mut end = i;
        let mut colon_used = false;
        loop {
            while end < bytes.len() && is_ident_char(bytes[end]) {
                end += 1;
            }
            match bytes.get(end) {
                Some(b'.' | b'$') if bytes.get(end + 1).is_some_and(|&c| is_ident_start(c)) => {
                    end += 1;
                }
                Some(b':')
                    if !colon_used
                        && line[start..end].contains('.')
                        && bytes.get(end + 1).is_some_and(|&c| is_ident_start(c)) =>
                {
                    colon_used = true;
                    end += 1;
                }
                _ => break,
            }
        }
        let token = &line[start..end];
        // A token followed by more of a name (`a.b.c-d`, `a.b.C/x`) or a
        // path is not a whole name.
        let followed = bytes
            .get(end)
            .is_some_and(|&c| c == b'/' || c == b'\\' || c == b'-' || c == b'.' || c == b'$');
        if !preceded && !followed && is_chain(&token.replace(':', "."), b".$") {
            out.push(token);
        }
        i = end.max(i + 1);
    }
    out
}

/// The lookup key of a name as written: `$` and `:` become `.`; `#member`
/// stays.
fn key_of(name: &str) -> String {
    name.replace(['$', ':'], ".")
}

/// The simple name a key ends with: the member after `#`, else the last
/// segment.
fn last_name(key: &str) -> &str {
    match key.rsplit_once('#') {
        Some((_, member)) => member,
        None => key.rsplit('.').next().unwrap_or(key),
    }
}

// --- Linking ----------------------------------------------------------------

/// How qualified names are rooted: Java/Kotlin packages and Python package
/// directories. Reads files under `root` when it was not told.
#[derive(Default)]
pub struct Layout {
    root: Option<PathBuf>,
    /// File → its `package` (Java, Kotlin), from the parse results.
    packages: HashMap<String, String>,
    /// Python package directories, when given instead of read from disk.
    package_dirs: Option<HashSet<String>>,
    read_packages: HashMap<String, Option<String>>,
    read_dirs: HashMap<String, bool>,
}

impl Layout {
    /// The layout of the repository at `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: Some(root.to_path_buf()),
            ..Self::default()
        }
    }

    /// A layout that reads nothing from disk: Python package directories are
    /// `package_dirs` (repository-relative, `/`-separated).
    pub fn fixed(package_dirs: impl IntoIterator<Item = String>) -> Self {
        Self {
            package_dirs: Some(package_dirs.into_iter().collect()),
            ..Self::default()
        }
    }

    /// Records the `package` a Java/Kotlin file declares.
    pub fn set_package(&mut self, file: &str, package: &str) {
        self.packages.insert(file.to_string(), package.to_string());
    }

    /// The package a Java/Kotlin file declares (`None`: the default one).
    fn jvm_package(&mut self, file: &str) -> Option<String> {
        if let Some(p) = self.packages.get(file) {
            return Some(p.clone());
        }
        let root = self.root.as_ref()?;
        if let Some(known) = self.read_packages.get(file) {
            return known.clone();
        }
        let package = std::fs::read_to_string(root.join(file))
            .ok()
            .and_then(|text| package_declaration(&text));
        self.read_packages.insert(file.to_string(), package.clone());
        package
    }

    fn is_python_package(&mut self, dir: &str) -> bool {
        if let Some(dirs) = &self.package_dirs {
            return dirs.contains(dir);
        }
        let Some(root) = &self.root else {
            return false;
        };
        if let Some(&known) = self.read_dirs.get(dir) {
            return known;
        }
        let is = root.join(dir).join("__init__.py").is_file();
        self.read_dirs.insert(dir.to_string(), is);
        is
    }

    /// The dotted module of a Python file: its path from the outermost
    /// directory of the run of package directories it is in. `None` for an
    /// `__init__.py` outside any package.
    fn python_module(&mut self, file: &str) -> Option<String> {
        let stem = file.strip_suffix(".py")?;
        let parts: Vec<&str> = stem.split('/').collect();
        let (dirs, name) = parts.split_at(parts.len() - 1);
        let mut first = dirs.len();
        while first > 0 && self.is_python_package(&dirs[..first].join("/")) {
            first -= 1;
        }
        let mut segments: Vec<&str> = dirs[first..].to_vec();
        if name[0] != "__init__" {
            segments.push(name[0]);
        }
        if segments.is_empty() || segments.iter().any(|s| !is_ident(s)) {
            return None;
        }
        Some(segments.join("."))
    }
}

/// The `package` a Java/Kotlin source declares, from its text.
pub fn package_declaration(text: &str) -> Option<String> {
    for line in text.lines().take(400) {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("package ") {
            let name: String = rest
                .trim()
                .trim_end_matches(';')
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            // `package a.b;`, or a one-segment `package app;`.
            let valid = is_chain(&name, b".") || is_ident(&name);
            return valid.then_some(name);
        }
        if t.starts_with("import ")
            || t.starts_with("public ")
            || t.starts_with("class ")
            || t.starts_with("interface ")
        {
            return None;
        }
    }
    None
}

fn is_type(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum
    )
}

fn is_source_kind(kind: &SymbolKind) -> bool {
    !matches!(
        kind,
        SymbolKind::Import
            | SymbolKind::Rationale
            | SymbolKind::Document
            | SymbolKind::Section
            | SymbolKind::Email
            | SymbolKind::Conversation
            | SymbolKind::Person
            | SymbolKind::Attachment
    )
}

/// The symbols that enclose `s` in its file (`file` is all of the file's
/// symbols), outermost first.
fn enclosing<'a>(s: &CodeSymbol, file: &[&'a CodeSymbol]) -> Vec<&'a CodeSymbol> {
    let mut outer: Vec<&CodeSymbol> = file
        .iter()
        .copied()
        .filter(|o| {
            o.uid != s.uid
                && is_source_kind(&o.kind)
                && o.start_line <= s.start_line
                && o.end_line >= s.end_line
                && ((o.end_line - o.start_line) > (s.end_line - s.start_line)
                    || (is_type(&o.kind) && !is_type(&s.kind)))
        })
        .collect();
    outer.sort_by_key(|o| std::cmp::Reverse(o.end_line - o.start_line));
    outer
}

/// The qualified name a symbol is named by, if it has one this module
/// links: `pkg.Outer.Inner`, `pkg.Type#member` (JVM), `pkg.mod.Class.method`
/// (Python).
fn qualified_key(
    s: &CodeSymbol,
    family: Family,
    file: &[&CodeSymbol],
    layout: &mut Layout,
) -> Option<String> {
    let outer = enclosing(s, file);
    match family {
        Family::Jvm => {
            // Nested in types only: a local or anonymous class has no name.
            if !outer.iter().all(|o| is_type(&o.kind)) {
                return None;
            }
            let mut segments: Vec<String> = Vec::new();
            if let Some(package) = layout.jvm_package(&s.file_path) {
                segments.push(package);
            }
            segments.extend(outer.iter().map(|o| o.name.clone()));
            if is_type(&s.kind) {
                segments.push(s.name.clone());
                Some(segments.join("."))
            } else if !outer.is_empty()
                && matches!(
                    s.kind,
                    SymbolKind::Method
                        | SymbolKind::Function
                        | SymbolKind::Variable
                        | SymbolKind::Constant
                )
            {
                Some(format!("{}#{}", segments.join("."), s.name))
            } else {
                None
            }
        }
        Family::Python => {
            if !matches!(
                s.kind,
                SymbolKind::Class | SymbolKind::Function | SymbolKind::Method
            ) || !outer.iter().all(|o| o.kind == SymbolKind::Class)
            {
                return None;
            }
            let mut segments = vec![layout.python_module(&s.file_path)?];
            segments.extend(outer.iter().map(|o| o.name.clone()));
            segments.push(s.name.clone());
            Some(segments.join("."))
        }
    }
}

/// What [`link`] found.
#[derive(Debug, Default)]
pub struct Linked {
    /// The `REFERENCES` edges.
    pub relationships: Vec<Relationship>,
    /// A file node for each config file that names a symbol.
    pub config_files: Vec<FileNode>,
    /// Names linked (each occurrence).
    pub linked: usize,
    /// Names that more than [`MAX_TARGETS`] symbols carry.
    pub ambiguous: usize,
}

/// The edges for `files`' candidates: each candidate whose name is exactly
/// the qualified name of one of `targets` (at most [`MAX_TARGETS`] of them)
/// gets an edge from the innermost of `sources` around it in its file, else
/// from its file's node in `file_nodes` (a config file gets a new node).
pub fn link(
    files: &[FileCandidates],
    sources: &[CodeSymbol],
    targets: &[CodeSymbol],
    file_nodes: &[FileNode],
    layout: &mut Layout,
    repo_id: &str,
) -> Linked {
    let mut linked = Linked::default();
    let keys: HashSet<String> = files
        .iter()
        .flat_map(|f| f.candidates.iter().map(|c| key_of(&c.name)))
        .collect();
    if keys.is_empty() {
        return linked;
    }
    let wanted: HashSet<&str> = keys.iter().map(|k| last_name(k)).collect();

    // Every symbol of the files that hold a wanted name, for what encloses
    // each.
    let target_files: HashSet<&str> = targets
        .iter()
        .filter(|s| wanted.contains(s.name.as_str()))
        .map(|s| s.file_path.as_str())
        .collect();
    let mut by_file: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    for s in targets {
        if target_files.contains(s.file_path.as_str()) {
            by_file.entry(s.file_path.as_str()).or_default().push(s);
        }
    }
    let mut table: HashMap<String, Vec<(&CodeSymbol, Family)>> = HashMap::new();
    for (file, symbols) in &by_file {
        let Some(family) = Family::of_path(file) else {
            continue;
        };
        for s in symbols {
            if !wanted.contains(s.name.as_str()) {
                continue;
            }
            if let Some(key) = qualified_key(s, family, symbols, layout) {
                if keys.contains(&key) {
                    table.entry(key).or_default().push((s, family));
                }
            }
        }
    }
    if table.is_empty() {
        return linked;
    }

    let mut sources_by_file: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    for s in sources {
        if is_source_kind(&s.kind) {
            sources_by_file
                .entry(s.file_path.as_str())
                .or_default()
                .push(s);
        }
    }
    let file_uids: HashMap<&str, &str> = file_nodes
        .iter()
        .map(|f| (f.path.as_str(), f.uid.as_str()))
        .collect();

    // (source, target) → metadata, in first-seen order.
    let mut edges: Vec<(String, String, NameRefMetadata)> = Vec::new();
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    for file in files {
        let mut config_uid: Option<String> = None;
        for (index, c) in file.candidates.iter().enumerate() {
            let Some(found) = table.get(&key_of(&c.name)) else {
                continue;
            };
            let found: Vec<&CodeSymbol> = found
                .iter()
                .filter(|(_, family)| match file.origin {
                    Origin::Code(own) => *family == own,
                    Origin::Config(_) => true,
                })
                .map(|(s, _)| *s)
                .collect();
            if found.is_empty() {
                continue;
            }
            if found.len() > MAX_TARGETS {
                linked.ambiguous += 1;
                continue;
            }
            let source_uid = match file.origin {
                Origin::Code(_) => {
                    let innermost = sources_by_file.get(file.path.as_str()).and_then(|syms| {
                        syms.iter()
                            .filter(|s| s.start_line <= c.line && c.line <= s.end_line)
                            .min_by_key(|s| s.end_line - s.start_line)
                    });
                    match innermost {
                        Some(s) => s.uid.clone(),
                        None => match file_uids.get(file.path.as_str()) {
                            Some(uid) => uid.to_string(),
                            None => continue,
                        },
                    }
                }
                Origin::Config(kind) => config_uid
                    .get_or_insert_with(|| {
                        let uid = Uuid::new_v4().to_string();
                        linked.config_files.push(FileNode {
                            uid: uid.clone(),
                            path: file.path.clone(),
                            language: config_language(kind).to_string(),
                            repo_id: repo_id.to_string(),
                            num_symbols: 0,
                        });
                        uid
                    })
                    .clone(),
            };
            linked.linked += 1;
            for target in found {
                if target.uid == source_uid {
                    continue;
                }
                let pair = (source_uid.clone(), target.uid.clone());
                if let Some(&at) = seen.get(&pair) {
                    edges[at].2.count += 1;
                    continue;
                }
                seen.insert(pair, edges.len());
                edges.push((
                    source_uid.clone(),
                    target.uid.clone(),
                    NameRefMetadata {
                        extraction_method: METHOD.to_string(),
                        file: file.path.clone(),
                        line: c.line,
                        name: c.name.clone(),
                        text: file.texts.get(index).cloned().unwrap_or_default(),
                        origin: match file.origin {
                            Origin::Code(_) => "string",
                            Origin::Config(_) => "config",
                        }
                        .to_string(),
                        count: 1,
                    },
                ));
            }
        }
    }
    linked.relationships = edges
        .into_iter()
        .map(|(source_uid, target_uid, meta)| Relationship {
            uid: Uuid::new_v4().to_string(),
            source_uid,
            target_uid,
            kind: RelationshipKind::References,
            repo_id: repo_id.to_string(),
            metadata: serde_json::to_string(&meta).unwrap_or_default(),
        })
        .collect();
    linked
}

/// The `language` of a config file's node.
fn config_language(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Properties => "properties",
        ResourceKind::NameList => "name-list",
        ResourceKind::Yaml => "yaml",
        ResourceKind::Xml => "xml",
        ResourceKind::Ini => "ini",
    }
}

/// The config/resource files under `root` whose names this module reads
/// ([`resource_kind`]), by the analysis' walk rules (skipped directories,
/// `include`/`exclude`, size), each with its candidates. Files that name
/// nothing name-shaped are left out.
pub fn scan_resources(root: &Path, scope: &crate::index_scope::IndexScope) -> Vec<FileCandidates> {
    use rayon::prelude::*;
    let mut paths: Vec<(PathBuf, String, ResourceKind)> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // META-INF is entered although it is not hidden; hidden and
            // build directories are skipped as by the analysis.
            !crate::index_scope::IndexScope::skips_name(&e.file_name().to_string_lossy())
        })
        .flatten()
    {
        if entry.path_is_symlink() || !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let Some(kind) = resource_kind(rel) else {
            continue;
        };
        if !scope.admits(rel) {
            continue;
        }
        match entry.metadata() {
            Ok(meta) if scope.within_size(meta.len()) => {}
            _ => continue,
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        paths.push((entry.path().to_path_buf(), rel_str, kind));
    }
    paths
        .par_iter()
        .filter_map(|(path, rel, kind)| read_resource(path, rel, *kind))
        .collect()
}

/// The name-reference pass of a full analysis: the config files under
/// `root` ([`scan_resources`]) and `code`'s string literals, linked to
/// `symbols`. Skipped (nothing read) when no Java, Kotlin or Python file
/// was indexed: no other language's names are known.
pub fn link_repository(
    root: &Path,
    scope: &crate::index_scope::IndexScope,
    mut code: Vec<FileCandidates>,
    symbols: &[CodeSymbol],
    file_nodes: &[FileNode],
    layout: &mut Layout,
    repo_id: &str,
) -> Linked {
    if !file_nodes
        .iter()
        .any(|f| Family::of_path(&f.path).is_some())
    {
        return Linked::default();
    }
    code.extend(scan_resources(root, scope));
    link(&code, symbols, symbols, file_nodes, layout, repo_id)
}

/// One config file's candidates, `None` when it has none or cannot be read.
pub fn read_resource(path: &Path, rel: &str, kind: ResourceKind) -> Option<FileCandidates> {
    let bytes = std::fs::read(path).ok()?;
    let source = std::str::from_utf8(&bytes).ok()?;
    let candidates = config_candidates(Path::new(rel), source, kind);
    if candidates.is_empty() {
        return None;
    }
    let texts = texts_of(source, &candidates);
    Some(FileCandidates {
        path: rel.to_string(),
        origin: Origin::Config(kind),
        candidates,
        texts,
    })
}

// --- Reading ----------------------------------------------------------------

/// One name-reference edge, resolved for display.
#[derive(Debug, Clone)]
pub struct NameRefHit<'a> {
    /// The edge's metadata.
    pub meta: NameRefMetadata,
    /// The symbol that names the target; `None` when a file does (a config
    /// file, module-level code).
    pub source: Option<&'a CodeSymbol>,
    /// The symbol named.
    pub target: &'a CodeSymbol,
}

/// The name-reference edges into `targets` (`incoming`) or out of them,
/// ordered by file and line. For a type, its members' edges count too.
pub fn hits<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    subjects: &[&CodeSymbol],
    incoming: bool,
) -> Vec<NameRefHit<'a>> {
    let mut wanted: HashSet<&str> = subjects.iter().map(|s| s.uid.as_str()).collect();
    // Members of a type subject (MEMBER_OF member → type).
    let types: HashSet<&str> = subjects
        .iter()
        .filter(|s| is_type(&s.kind))
        .map(|s| s.uid.as_str())
        .collect();
    if !types.is_empty() {
        for r in relationships {
            if r.kind == RelationshipKind::MemberOf && types.contains(r.target_uid.as_str()) {
                wanted.insert(r.source_uid.as_str());
            }
        }
    }
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut out: Vec<NameRefHit<'a>> = relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::References)
        .filter(|r| {
            let end = if incoming {
                &r.target_uid
            } else {
                &r.source_uid
            };
            wanted.contains(end.as_str())
        })
        .filter_map(|r| {
            let meta = NameRefMetadata::of(r)?;
            let target = *by_uid.get(r.target_uid.as_str())?;
            Some(NameRefHit {
                source: by_uid.get(r.source_uid.as_str()).copied(),
                target,
                meta,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (a.meta.file.as_str(), a.meta.line, &a.target.qualified_name).cmp(&(
            b.meta.file.as_str(),
            b.meta.line,
            &b.target.qualified_name,
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(file: &str, name: &str, kind: SymbolKind, lines: (u32, u32)) -> CodeSymbol {
        CodeSymbol {
            uid: format!("{file}:{name}:{}", lines.0),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: lines.0,
            end_line: lines.1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    fn code(path: &str, source: &str, lang: SourceLanguage) -> FileCandidates {
        let candidates = code_candidates(source, lang);
        let texts = texts_of(source, &candidates);
        FileCandidates {
            path: path.to_string(),
            origin: Origin::Code(Family::of_language(lang).unwrap()),
            candidates,
            texts,
        }
    }

    fn config(path: &str, source: &str) -> FileCandidates {
        let kind = resource_kind(Path::new(path)).unwrap();
        let candidates = config_candidates(Path::new(path), source, kind);
        let texts = texts_of(source, &candidates);
        FileCandidates {
            path: path.to_string(),
            origin: Origin::Config(kind),
            candidates,
            texts,
        }
    }

    fn names(c: &[Candidate]) -> Vec<(u32, &str)> {
        c.iter().map(|c| (c.line, c.name.as_str())).collect()
    }

    #[test]
    fn java_literals_are_whole_names_outside_comments() {
        let src = r#"package a;
// Class.forName("a.b.Commented")
/* "a.b.Block" */
class X {
  @ConditionalOnClass(name = "a.b.Present")
  void m() {
    Class.forName("a.b.Outer$Inner");
    load("a.b.Type#member");
    log("see a.b.Prose");
    char c = '"'; String s = "a.b.AfterChar";
    String t = """
        a.b.Block2
        """;
    String e = "a.b.\"Esc\"";
    String one = "default";
    String path = "a/b/C.class";
  }
}
"#;
        let found = code_candidates(src, SourceLanguage::Java);
        assert_eq!(
            names(&found),
            vec![
                (5, "a.b.Present"),
                (7, "a.b.Outer$Inner"),
                (8, "a.b.Type#member"),
                (10, "a.b.AfterChar"),
                (11, "a.b.Block2"),
            ]
        );
    }

    #[test]
    fn python_literals_skip_comments_f_and_byte_strings() {
        let src = "X = 'pkg.mod.A'  # 'pkg.mod.Comment'\n\
                   Y = f\"pkg.mod.{x}\"\n\
                   Z = b'pkg.mod.Bytes'\n\
                   W = r\"pkg.mod:func\"\n\
                   \"\"\"Docstring about pkg.mod.A.\"\"\"\n\
                   V = [\n    \"pkg.mod.B\",\n]\n";
        let found = code_candidates(src, SourceLanguage::Python);
        assert_eq!(
            names(&found),
            vec![(1, "pkg.mod.A"), (4, "pkg.mod:func"), (7, "pkg.mod.B")]
        );
    }

    #[test]
    fn config_tokens_are_whole_and_outside_comments() {
        let factories = "# a.b.Commented\n\
            org.springframework.boot.autoconfigure.EnableAutoConfiguration=\\\n\
            a.b.FirstAutoConfiguration,\\\n\
            a.b.Second\n";
        let found = config_candidates(
            Path::new("src/main/resources/META-INF/spring.factories"),
            factories,
            ResourceKind::Properties,
        );
        assert_eq!(
            names(&found),
            vec![
                (
                    2,
                    "org.springframework.boot.autoconfigure.EnableAutoConfiguration"
                ),
                (3, "a.b.FirstAutoConfiguration"),
                (4, "a.b.Second"),
            ]
        );
        let xml = "<beans>\n<!-- <bean class=\"a.b.Old\"/>\n-->\n\
            <bean class=\"a.b.New\" p:x=\"logging.level.a.b.New\"/>\n\
            <import resource=\"classpath:a/b/ctx.xml\"/>\n</beans>\n";
        let found = config_candidates(Path::new("ctx.xml"), xml, ResourceKind::Xml);
        assert_eq!(
            names(&found),
            vec![(4, "a.b.New"), (4, "logging.level.a.b.New")]
        );
        let services = "# comment a.b.No\na.b.Impl # note\n";
        let found = config_candidates(
            Path::new("res/META-INF/services/a.b.Service"),
            services,
            ResourceKind::NameList,
        );
        assert_eq!(names(&found), vec![(0, "a.b.Service"), (2, "a.b.Impl")]);
        let yaml = "spring:\n  main:\n    banner: a.b.Banner # a.b.Not\n# a.b.No\n";
        let found = config_candidates(Path::new("application.yml"), yaml, ResourceKind::Yaml);
        assert_eq!(names(&found), vec![(3, "a.b.Banner")]);
        let toml = "[project.scripts]\nmyc = \"pkg.cli:main\"\n";
        let found = config_candidates(Path::new("pyproject.toml"), toml, ResourceKind::Ini);
        // A candidate is only a name's form: `project.scripts` names no
        // symbol and links nothing.
        assert_eq!(
            names(&found),
            vec![(1, "project.scripts"), (2, "pkg.cli:main")]
        );
    }

    #[test]
    fn resource_kinds_follow_the_conventions() {
        let kind = |p: &str| resource_kind(Path::new(p));
        assert_eq!(
            kind("m/src/main/resources/META-INF/spring.factories"),
            Some(ResourceKind::Properties)
        );
        assert_eq!(
            kind("m/src/main/resources/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports"),
            Some(ResourceKind::NameList)
        );
        assert_eq!(
            kind("m/src/main/resources/META-INF/services/a.b.Service"),
            Some(ResourceKind::NameList)
        );
        assert_eq!(kind("x/other.imports"), None);
        assert_eq!(kind("app/application.yaml"), Some(ResourceKind::Yaml));
        assert_eq!(kind("src/Main.java"), None);
    }

    /// The fixture of the linking tests: a Java package `a.b` and a Python
    /// package `pkg`.
    fn project() -> Vec<CodeSymbol> {
        vec![
            sym("src/a/b/Plugin.java", "Plugin", SymbolKind::Class, (3, 20)),
            sym("src/a/b/Plugin.java", "Inner", SymbolKind::Class, (5, 8)),
            sym("src/a/b/Plugin.java", "start", SymbolKind::Method, (10, 12)),
            sym("src/a/b/Plugin.java", "run", SymbolKind::Method, (13, 19)),
            // A local class in a method has no qualified name.
            sym("src/a/b/Plugin.java", "Local", SymbolKind::Class, (14, 16)),
            sym("src/a/b/Loader.java", "Loader", SymbolKind::Class, (3, 30)),
            sym("src/a/b/Loader.java", "load", SymbolKind::Method, (5, 12)),
            sym(
                "src/a/b/Loader.java",
                "NAME",
                SymbolKind::Constant,
                (14, 14),
            ),
            sym("pkg/views.py", "Reporter", SymbolKind::Class, (1, 10)),
            sym("pkg/views.py", "render", SymbolKind::Method, (3, 9)),
            sym(
                "pkg/views.py",
                "get_reporter",
                SymbolKind::Function,
                (12, 14),
            ),
            sym(
                "pkg/settings.py",
                "configure",
                SymbolKind::Function,
                (20, 25),
            ),
            sym("scripts/tool.py", "main", SymbolKind::Function, (1, 3)),
        ]
    }

    fn layout() -> Layout {
        let mut layout = Layout::fixed(["pkg".to_string()]);
        layout.set_package("src/a/b/Plugin.java", "a.b");
        layout.set_package("src/a/b/Loader.java", "a.b");
        layout
    }

    fn file_node(path: &str) -> FileNode {
        FileNode {
            uid: format!("file:{path}"),
            path: path.to_string(),
            language: String::new(),
            repo_id: "r".to_string(),
            num_symbols: 0,
        }
    }

    /// `(source uid, target qualified-name-ish uid, line)` of each edge.
    fn edges(linked: &Linked) -> Vec<(String, String, u32)> {
        let mut out: Vec<_> = linked
            .relationships
            .iter()
            .map(|r| {
                let meta = NameRefMetadata::of(r).unwrap();
                (r.source_uid.clone(), r.target_uid.clone(), meta.line)
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn exact_qualified_names_link_from_the_innermost_symbol() {
        let symbols = project();
        let java = code(
            "src/a/b/Loader.java",
            "package a.b;\n\
             \n\
             class Loader {\n\
             \n\
               void load() {\n\
                 Class.forName(\"a.b.Plugin\");\n\
                 Class.forName(\"a.b.Plugin$Inner\");\n\
                 Class.forName(\"a.b.Plugin.Inner\");\n\
                 invoke(\"a.b.Plugin#start\");\n\
                 Class.forName(\"a.b.Plugin\");\n\
               }\n\
             \n\
             \n\
               static final String NAME = \"a.b.Loader\";\n\
             }\n",
            SourceLanguage::Java,
        );
        let python = code(
            "pkg/settings.py",
            "REPORTER = \"pkg.views.Reporter\"\n\
             ENTRY = \"pkg.views:get_reporter\"\n\
             RENDER = \"pkg.views.Reporter.render\"\n",
            SourceLanguage::Python,
        );
        let linked = link(
            &[java, python],
            &symbols,
            &symbols,
            &[file_node("pkg/settings.py")],
            &mut layout(),
            "r",
        );
        assert_eq!(
            edges(&linked),
            vec![
                (
                    "file:pkg/settings.py".to_string(),
                    "pkg/views.py:Reporter:1".to_string(),
                    1
                ),
                (
                    "file:pkg/settings.py".to_string(),
                    "pkg/views.py:get_reporter:12".to_string(),
                    2
                ),
                (
                    "file:pkg/settings.py".to_string(),
                    "pkg/views.py:render:3".to_string(),
                    3
                ),
                // A constant naming its own class: from the constant.
                (
                    "src/a/b/Loader.java:NAME:14".to_string(),
                    "src/a/b/Loader.java:Loader:3".to_string(),
                    14
                ),
                (
                    "src/a/b/Loader.java:load:5".to_string(),
                    "src/a/b/Plugin.java:Inner:5".to_string(),
                    7
                ),
                (
                    "src/a/b/Loader.java:load:5".to_string(),
                    "src/a/b/Plugin.java:Plugin:3".to_string(),
                    6
                ),
                (
                    "src/a/b/Loader.java:load:5".to_string(),
                    "src/a/b/Plugin.java:start:10".to_string(),
                    9
                ),
            ]
        );
        let plugin_twice = linked
            .relationships
            .iter()
            .filter_map(NameRefMetadata::of)
            .find(|m| m.name == "a.b.Plugin")
            .unwrap();
        assert_eq!(plugin_twice.count, 2);
        assert_eq!(plugin_twice.text, "Class.forName(\"a.b.Plugin\");");
        assert_eq!(linked.config_files.len(), 0);
    }

    /// Precision: partial names, short names, package names, common words,
    /// names of the other language family and names with a different
    /// package never link.
    #[test]
    fn partial_and_foreign_names_do_not_link() {
        let symbols = project();
        let java = code(
            "src/a/b/Loader.java",
            "package a.b;\nclass Loader {\n void load() {\n\
             f(\"b.Plugin\"); f(\"Plugin\"); f(\"a.b\"); f(\"a.b.plugin\");\n\
             f(\"a.b.Plugin.start\"); f(\"a.b.Plugin#nope\"); f(\"x.a.b.Plugin\");\n\
             f(\"a.b.Plugin.Local\"); f(\"a.b.Plugin#Local\"); f(\"pkg.views.Reporter\");\n\
             f(\"default\"); f(\"true\"); f(\"a.b.Plugin \");\n }\n}\n",
            SourceLanguage::Java,
        );
        let python = code(
            "pkg/settings.py",
            "A = 'views.Reporter'\nB = 'Reporter'\nC = 'pkg.views'\nD = 'a.b.Plugin'\n\
             E = 'pkg.Reporter'\nF = 'tool.main.x'\n",
            SourceLanguage::Python,
        );
        let props = config(
            "src/main/resources/app.properties",
            "logging.level.a.b.Plugin=DEBUG\na.b.Plugin.enabled=true\npkg=a.b\nname=Plugin\n",
        );
        let linked = link(
            &[java, python, props],
            &symbols,
            &symbols,
            &[file_node("pkg/settings.py")],
            &mut layout(),
            "r",
        );
        assert_eq!(edges(&linked), vec![]);
        assert!(linked.config_files.is_empty());
    }

    #[test]
    fn config_files_get_a_node_and_link_either_family() {
        let symbols = project();
        let factories = config(
            "src/main/resources/META-INF/spring.factories",
            "a.b.Loader=\\\n  a.b.Plugin\n",
        );
        let toml = config("pyproject.toml", "[x]\nrun = \"pkg.views:get_reporter\"\n");
        let tool = config("setup.cfg", "[x]\nrun = tool.main\n");
        let linked = link(
            &[factories, toml, tool],
            &symbols,
            &symbols,
            &[],
            &mut layout(),
            "r",
        );
        let mut paths: Vec<&str> = linked
            .config_files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "pyproject.toml",
                "setup.cfg",
                "src/main/resources/META-INF/spring.factories"
            ]
        );
        let mut named: Vec<(String, u32)> = linked
            .relationships
            .iter()
            .filter_map(NameRefMetadata::of)
            .map(|m| (m.name, m.line))
            .collect();
        named.sort();
        assert_eq!(
            named,
            [
                ("a.b.Loader".to_string(), 1),
                ("a.b.Plugin".to_string(), 2),
                ("pkg.views:get_reporter".to_string(), 2),
                // `scripts/` has no `__init__.py`: `tool.py` is module `tool`.
                ("tool.main".to_string(), 2),
            ]
        );
    }

    #[test]
    fn a_name_of_too_many_symbols_links_none() {
        let mut symbols = project();
        for i in 0..=MAX_TARGETS as u32 {
            symbols.push(sym(
                "src/a/b/Many.java",
                "Many",
                SymbolKind::Class,
                (100 * i + 1, 100 * i + 50),
            ));
        }
        let mut layout = layout();
        layout.set_package("src/a/b/Many.java", "a.b");
        let props = config("x.properties", "k=a.b.Many\n");
        let linked = link(&[props], &symbols, &symbols, &[], &mut layout, "r");
        assert!(linked.relationships.is_empty());
        assert_eq!(linked.ambiguous, 1);
    }

    #[test]
    fn python_modules_start_at_the_outermost_package() {
        let mut layout = Layout::fixed(["src/pkg".to_string(), "src/pkg/sub".to_string()]);
        assert_eq!(
            layout.python_module("src/pkg/sub/mod.py").as_deref(),
            Some("pkg.sub.mod")
        );
        assert_eq!(
            layout.python_module("src/pkg/__init__.py").as_deref(),
            Some("pkg")
        );
        assert_eq!(layout.python_module("tool.py").as_deref(), Some("tool"));
        assert_eq!(layout.python_module("__init__.py"), None);
    }

    #[test]
    fn package_declarations_are_read_from_the_header() {
        assert_eq!(
            package_declaration("/* x */\npackage org.acme.app;\nimport a.B;\n").as_deref(),
            Some("org.acme.app")
        );
        assert_eq!(
            package_declaration("package org.acme\n\nclass K\n").as_deref(),
            Some("org.acme")
        );
        assert_eq!(package_declaration("import a.B;\nclass C {}\n"), None);
    }
}

/// The pass inside a real analysis: a Spring auto-configuration project, a
/// Django-like project, and incremental re-indexing.
#[cfg(test)]
mod analysis_tests {
    use super::*;
    use crate::analyzer::Analyzer;
    use myceliums_storage::Store;
    use tempfile::TempDir;

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    /// `(where, target qualified name, line)` of every name-reference edge:
    /// `where` is `file:qualified_name` for a symbol, `file` for a file.
    async fn name_edges(store: &Store) -> Vec<(String, String, u32)> {
        let symbols = store.get_symbols().await.unwrap();
        let files = store.get_files().await.unwrap();
        let by_uid: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
        let file_by_uid: HashMap<&str, &str> = files
            .iter()
            .map(|f| (f.uid.as_str(), f.path.as_str()))
            .collect();
        let mut out = Vec::new();
        for r in store.get_relationships().await.unwrap() {
            let Some(meta) = NameRefMetadata::of(&r) else {
                continue;
            };
            let from = match by_uid.get(r.source_uid.as_str()) {
                Some(s) => format!("{}:{}", s.file_path, s.qualified_name),
                None => file_by_uid
                    .get(r.source_uid.as_str())
                    .expect("the source is a symbol or an indexed file")
                    .to_string(),
            };
            assert_eq!(from.split(':').next().unwrap(), meta.file);
            out.push((
                from,
                by_uid[r.target_uid.as_str()].qualified_name.clone(),
                meta.line,
            ));
        }
        out.sort();
        out
    }

    fn e(from: &str, to: &str, line: u32) -> (String, String, u32) {
        (from.to_string(), to.to_string(), line)
    }

    const AUTO_CONFIG: &str = r#"package org.acme.autoconfigure;

@AutoConfiguration
@ConditionalOnClass(name = "org.acme.client.AcmeClient")
public class AcmeAutoConfiguration {
    // Class.forName("org.acme.client.AcmeClient") in a comment
    public Object client() throws Exception {
        return Class.forName("org.acme.client.AcmeClient$Builder").getDeclaredConstructor().newInstance();
    }

    public boolean present() {
        return isPresent("org.acme.client") || isPresent("AcmeClient") || isPresent("acme.client.AcmeClient");
    }
}
"#;

    /// #179: a Spring auto-configuration project. Every place that names a
    /// class by its fully qualified name links to it, from the innermost
    /// symbol (code) or the file (config); a package name, a short name, a
    /// partial name, a comment and a build directory do not.
    #[tokio::test]
    async fn spring_auto_configuration_names_link_to_their_classes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("acme");
        write(
            &root,
            &[
                (
                    "src/main/java/org/acme/autoconfigure/AcmeAutoConfiguration.java",
                    AUTO_CONFIG,
                ),
                (
                    "src/main/java/org/acme/autoconfigure/AcmeEnvironment.java",
                    "package org.acme.autoconfigure;\n\npublic class AcmeEnvironment {}\n",
                ),
                (
                    "src/main/java/org/acme/client/AcmeClient.java",
                    "package org.acme.client;\n\npublic class AcmeClient {\n    public static class Builder {\n        public AcmeClient build() { return new AcmeClient(); }\n    }\n}\n",
                ),
                (
                    "src/main/java/org/acme/spi/Codec.java",
                    "package org.acme.spi;\n\npublic interface Codec {}\n",
                ),
                (
                    "src/main/java/org/acme/spi/JsonCodec.java",
                    "package org.acme.spi;\n\npublic class JsonCodec implements Codec {}\n",
                ),
                (
                    "src/main/resources/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports",
                    "# auto-configurations\norg.acme.autoconfigure.AcmeAutoConfiguration\n",
                ),
                (
                    "src/main/resources/META-INF/spring.factories",
                    "org.springframework.boot.env.EnvironmentPostProcessor=\\\n  org.acme.autoconfigure.AcmeEnvironment\n",
                ),
                (
                    "src/main/resources/META-INF/services/org.acme.spi.Codec",
                    "org.acme.spi.JsonCodec\n",
                ),
                (
                    "src/main/resources/beans.xml",
                    "<beans>\n  <!-- <bean class=\"org.acme.spi.JsonCodec\"/> -->\n  <bean id=\"client\" class=\"org.acme.client.AcmeClient\"/>\n</beans>\n",
                ),
                (
                    "src/main/resources/application.yml",
                    "acme:\n  codec: org.acme.spi.JsonCodec\n",
                ),
                (
                    "src/main/resources/application.properties",
                    "logging.level.org.acme.client=DEBUG\nacme.client-class=org.acme.client.AcmeClient\nacme.name=AcmeClient\n",
                ),
                (
                    "build/resources/main/META-INF/spring.factories",
                    "x=org.acme.client.AcmeClient\n",
                ),
            ],
        );
        let store = Store::open(&dir.path().join("db"), "acme").await.unwrap();
        let analyzer = Analyzer::new(store, root).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();
        let config = "src/main/resources";
        let auto = "src/main/java/org/acme/autoconfigure/AcmeAutoConfiguration.java";
        assert_eq!(
            name_edges(analyzer.store()).await,
            vec![
                e(&format!("{auto}:AcmeAutoConfiguration"), "AcmeClient", 4),
                e(
                    &format!("{auto}:AcmeAutoConfiguration.client"),
                    "AcmeClient.Builder",
                    8
                ),
                e(&format!("{config}/META-INF/services/org.acme.spi.Codec"), "Codec", 0),
                e(&format!("{config}/META-INF/services/org.acme.spi.Codec"), "JsonCodec", 1),
                e(&format!("{config}/META-INF/spring.factories"), "AcmeEnvironment", 2),
                e(
                    &format!("{config}/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports"),
                    "AcmeAutoConfiguration",
                    2
                ),
                e(&format!("{config}/application.properties"), "AcmeClient", 2),
                e(&format!("{config}/application.yml"), "JsonCodec", 2),
                e(&format!("{config}/beans.xml"), "AcmeClient", 3),
            ]
        );
        // The config files that name a symbol are file nodes; the others
        // (and the build output) are not indexed.
        let files = analyzer.store().get_files().await.unwrap();
        assert!(files
            .iter()
            .any(|f| f.path == format!("{config}/beans.xml") && f.language == "xml"));
        assert!(!files.iter().any(|f| f.path.starts_with("build/")));
    }

    /// #179, the agent eval's `django-flow-runserver-import-string`: Django
    /// reaches classes through `import_string(settings.X)` with the dotted
    /// path in the settings module, and through literal paths. Empty
    /// `__init__.py` files (not indexed) still make the packages.
    #[tokio::test]
    async fn django_settings_and_import_string_paths_link() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("django-project");
        write(
            &root,
            &[
                ("django/__init__.py", "def setup():\n    from django.utils.log import configure_logging\n    configure_logging(\"logging.config.dictConfig\")\n"),
                ("django/utils/__init__.py", ""),
                ("django/utils/module_loading.py", "def import_string(dotted_path):\n    module_path, class_name = dotted_path.rsplit(\".\", 1)\n    return getattr(__import__(module_path), class_name)\n"),
                ("django/utils/log.py", "def configure_logging(logging_config):\n    pass\n"),
                ("django/conf/__init__.py", ""),
                (
                    "django/conf/global_settings.py",
                    "WSGI_APPLICATION = None\n\
                     DEFAULT_EXCEPTION_REPORTER = \"django.views.debug.ExceptionReporter\"\n\
                     DEFAULT_EXCEPTION_REPORTER_FILTER = \"django.views.debug.SafeExceptionReporterFilter\"\n\
                     LOGGING_CONFIG = \"logging.config.dictConfig\"\n\
                     # MIDDLEWARE = [\"django.middleware.security.SecurityMiddleware\"]\n",
                ),
                ("django/views/__init__.py", ""),
                (
                    "django/views/debug.py",
                    "from django.conf import settings\n\
                     from django.utils.module_loading import import_string\n\
                     \n\
                     class SafeExceptionReporterFilter:\n    pass\n\
                     \n\
                     class ExceptionReporter:\n    def get_traceback_text(self):\n        return \"\"\n\
                     \n\
                     def get_exception_reporter_class(request):\n    \
                         return import_string(settings.DEFAULT_EXCEPTION_REPORTER)\n",
                ),
                ("django/core/__init__.py", ""),
                ("django/core/handlers/__init__.py", ""),
                ("django/core/handlers/wsgi.py", "class WSGIHandler:\n    def __init__(self):\n        pass\n"),
                ("django/core/servers/__init__.py", ""),
                (
                    "django/core/servers/basehttp.py",
                    "from django.utils.module_loading import import_string\n\
                     \n\
                     def get_internal_wsgi_application():\n    \
                         return import_string(\"django.core.handlers.wsgi.WSGIHandler\")()\n",
                ),
                ("django/middleware/__init__.py", ""),
                ("django/middleware/security.py", "class SecurityMiddleware:\n    def __call__(self, request):\n        return request\n"),
                ("mysite/__init__.py", ""),
                (
                    "mysite/settings.py",
                    "MIDDLEWARE = [\n    \"django.middleware.security.SecurityMiddleware\",\n    \"django.middleware.SecurityMiddleware\",\n]\nROOT_URLCONF = \"mysite.urls\"\n",
                ),
                ("pyproject.toml", "[project.scripts]\nreport = \"django.views.debug:get_exception_reporter_class\"\n"),
            ],
        );
        let store = Store::open(&dir.path().join("db"), "django").await.unwrap();
        let analyzer = Analyzer::new(store, root.clone()).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();
        let expected = vec![
            e("django/conf/global_settings.py", "ExceptionReporter", 2),
            e(
                "django/conf/global_settings.py",
                "SafeExceptionReporterFilter",
                3,
            ),
            e(
                "django/core/servers/basehttp.py:get_internal_wsgi_application",
                "WSGIHandler",
                4,
            ),
            e("mysite/settings.py", "SecurityMiddleware", 2),
            e("pyproject.toml", "get_exception_reporter_class", 2),
        ];
        assert_eq!(name_edges(analyzer.store()).await, expected);

        // Incremental: a changed settings module and a changed config file
        // are linked again against the stored symbols; nothing doubles.
        write(
            &root,
            &[
                (
                    "django/conf/global_settings.py",
                    "DEFAULT_EXCEPTION_REPORTER = \"django.views.debug.ExceptionReporter\"\n",
                ),
                ("pyproject.toml", "[project.scripts]\n"),
            ],
        );
        analyzer
            .analyze_files(&[
                root.join("django/conf/global_settings.py"),
                root.join("pyproject.toml"),
            ])
            .await
            .unwrap();
        assert_eq!(
            name_edges(analyzer.store()).await,
            vec![
                e("django/conf/global_settings.py", "ExceptionReporter", 1),
                e(
                    "django/core/servers/basehttp.py:get_internal_wsgi_application",
                    "WSGIHandler",
                    4,
                ),
                e("mysite/settings.py", "SecurityMiddleware", 2),
            ]
        );
        assert!(!analyzer
            .store()
            .get_files()
            .await
            .unwrap()
            .iter()
            .any(|f| f.path == "pyproject.toml"));
    }

    /// An incremental run links a changed Java file's strings to classes of
    /// unchanged files, reading their package from disk.
    #[tokio::test]
    async fn incremental_java_strings_link_to_stored_classes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("acme");
        write(
            &root,
            &[
                (
                    "src/org/acme/client/AcmeClient.java",
                    "package org.acme.client;\n\npublic class AcmeClient {}\n",
                ),
                (
                    "src/org/acme/Loader.java",
                    "package org.acme;\n\npublic class Loader {\n    void load() {}\n}\n",
                ),
            ],
        );
        let store = Store::open(&dir.path().join("db"), "acme").await.unwrap();
        let analyzer = Analyzer::new(store, root.clone()).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();
        assert!(name_edges(analyzer.store()).await.is_empty());
        write(
            &root,
            &[(
                "src/org/acme/Loader.java",
                "package org.acme;\n\npublic class Loader {\n    void load() throws Exception {\n        Class.forName(\"org.acme.client.AcmeClient\");\n    }\n}\n",
            )],
        );
        analyzer
            .analyze_files(&[root.join("src/org/acme/Loader.java")])
            .await
            .unwrap();
        assert_eq!(
            name_edges(analyzer.store()).await,
            vec![e("src/org/acme/Loader.java:Loader.load", "AcmeClient", 5)]
        );
    }
}
