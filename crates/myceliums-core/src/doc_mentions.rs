//! Doc-to-symbol mentions (#180): which docs name a code symbol.
//!
//! A document names code in a few ways an author means as code: Markdown
//! and AsciiDoc code spans (`` `Foo#bar()` ``), Sphinx/MyST roles
//! (``:class:`pkg.Foo` ``), links into the Javadoc (`{spring-framework-api}/core/Foo.html#bar()`,
//! `javadoc:pkg.Foo#bar()[]`), and in code, the doc comment links
//! (`{@link Foo#bar}`, `@see`, `<see cref>`, KDoc/rustdoc `[Foo]`) that
//! [`crate::parser`] records as [`DocRef`]s. Each becomes a `MENTIONS` edge
//! from the innermost `Section` (or the `Document`) around it, or from the
//! documented symbol, to the code symbol it names.
//!
//! Precision comes first: an agent will edit the docs this lists.
//!
//! - A qualified name (`Foo#bar`, `pkg.Foo`, `Foo::bar`) matches symbols
//!   whose container path — directories, file stem, enclosing types — ends
//!   with it; leading package segments that match nothing are dropped, the
//!   owner never is.
//! - A bare name matches only when one symbol (or one overload set) has it,
//!   preferring product code over tests, or when exactly one candidate's
//!   owner is named in the same section (in a doc comment: the documented
//!   type, then the same file, then the same directory).
//! - A bare word in a code span must look like code: camelCase, PascalCase,
//!   snake_case, ALL_CAPS, a call `name()` or an annotation `@Name`.
//!
//! Anything else is recorded as ambiguous or unresolved
//! ([`DocMentionStats`]) and not linked. Prose is never matched word by
//! word.

use crate::content::adoc_block_delimiter;
use crate::mentions::{MentionMatch, MentionMetadata};
use myceliums_storage::{
    CodeSymbol, Relationship, RelationshipKind, Store, SymbolKind, SymbolMetadata,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;
use uuid::Uuid;

/// `extraction_method` of the edges this module makes.
pub const METHOD: &str = "doc-ref";

/// Locations kept per ambiguous name in [`DocMentionStats`].
const AMBIGUOUS_LOCATIONS: usize = 3;

/// How a mention was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Syntax {
    /// An inline code span in Markdown or AsciiDoc.
    CodeSpan,
    /// A Sphinx or MyST role (``:class:`Foo` ``, ``{py:func}`foo` ``).
    Role,
    /// A link into API docs (Javadoc URL, `javadoc:` macro).
    ApiLink,
    /// A link in a doc comment ([`DocRef`]).
    DocComment,
}

impl Syntax {
    /// The name stored in the edge metadata.
    pub fn name(self) -> &'static str {
        match self {
            Syntax::CodeSpan => "code-span",
            Syntax::Role => "role",
            Syntax::ApiLink => "api-link",
            Syntax::DocComment => "doc-comment",
        }
    }

    /// Whether the syntax itself says the text is a code reference; a code
    /// span may hold any code (a literal, a property key, an XML tag).
    fn explicit(self) -> bool {
        !matches!(self, Syntax::CodeSpan)
    }
}

/// A bare name the analysis did not link because several symbols carry it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbiguousName {
    /// Mentions of the name left unlinked.
    pub count: usize,
    /// The first few of them, as `file:line`.
    pub at: Vec<String>,
}

/// What became of the mentions one analysis found (#180). Persisted in the
/// index (`index_meta`, key [`DocMentionStats::META_KEY`]) so the tools can
/// say which mentions of a name were left unlinked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocMentionStats {
    /// Mentions linked to a symbol.
    pub linked: usize,
    /// Mentions of a name several symbols carry, none preferred.
    pub ambiguous: usize,
    /// Code-shaped mentions of nothing in the project (a JDK type, a
    /// library, a literal).
    pub unresolved: usize,
    /// Ambiguous mentions by the name written.
    #[serde(default)]
    pub ambiguous_by_name: BTreeMap<String, AmbiguousName>,
}

impl DocMentionStats {
    /// Key under which the record is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "doc_mentions";

    /// The record of the last full analysis, `None` for an index built
    /// before #180.
    pub async fn load(store: &Store) -> myceliums_storage::Result<Option<Self>> {
        Ok(store
            .get_index_meta(Self::META_KEY)
            .await?
            .and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Store the record.
    pub async fn save(&self, store: &Store) -> myceliums_storage::Result<()> {
        let json = serde_json::to_string(self).unwrap_or_default();
        store.set_index_meta(Self::META_KEY, &json).await
    }

    /// The unlinked mentions of `name`.
    pub fn ambiguous_named(&self, name: &str) -> Option<&AmbiguousName> {
        self.ambiguous_by_name.get(name)
    }
}

/// Edges and statistics of one extraction.
#[derive(Debug, Default)]
pub struct Extracted {
    /// `MENTIONS` edges, one per (source, target), every match in its metadata.
    pub relationships: Vec<Relationship>,
    /// What became of the mentions.
    pub stats: DocMentionStats,
}

/// A mention as found, before resolution.
struct Found<'a> {
    /// The section, document or documented symbol the edge starts at.
    source: &'a CodeSymbol,
    line: u32,
    text: String,
    context: String,
    syntax: Syntax,
}

/// Link the mentions written in `sources` — documents with their sections,
/// and code symbols with doc-comment references — to the code symbols of
/// `targets` (the whole project; `sources` may be a subset of it).
pub fn extract(sources: &[CodeSymbol], targets: &[CodeSymbol], repo_id: &str) -> Extracted {
    let found = find_mentions(sources);
    let index = TargetIndex::new(targets);
    // Type-like names written in each scope (a section, a doc comment), the
    // owners a bare member name may belong to.
    let mut scope_names: HashMap<&str, HashSet<String>> = HashMap::new();
    for f in &found {
        let names = scope_names.entry(f.source.uid.as_str()).or_default();
        if let Some(r) = Ref::parse(&f.text, f.syntax) {
            for seg in &r.segments {
                if seg.starts_with(|c: char| c.is_ascii_uppercase()) {
                    names.insert(seg.clone());
                }
            }
        }
        if f.syntax == Syntax::DocComment {
            names.extend(owner_chain(f.source));
        }
    }

    let mut stats = DocMentionStats::default();
    let mut edges: HashMap<(&str, &str), Vec<MentionMatch>> = HashMap::new();
    let mut order: Vec<(&str, &str)> = Vec::new();
    for f in &found {
        let Some(r) = Ref::parse(&f.text, f.syntax) else {
            continue;
        };
        let r = if r.this_type {
            // Javadoc `{@link #member}`: a member of the documented type.
            match own_type(f.source) {
                Some(owner) => r.with_owner(owner),
                None => continue,
            }
        } else {
            r
        };
        let owners = scope_names.get(f.source.uid.as_str());
        match index.resolve(&r, f, owners) {
            Outcome::NotCode => {}
            Outcome::Unresolved => stats.unresolved += 1,
            Outcome::Ambiguous => {
                stats.ambiguous += 1;
                let entry = stats
                    .ambiguous_by_name
                    .entry(r.segments.last().cloned().unwrap_or_default())
                    .or_default();
                entry.count += 1;
                if entry.at.len() < AMBIGUOUS_LOCATIONS {
                    entry.at.push(format!("{}:{}", f.source.file_path, f.line));
                }
            }
            Outcome::Linked(hits) => {
                let hits: Vec<&CodeSymbol> =
                    hits.into_iter().filter(|t| t.uid != f.source.uid).collect();
                if hits.is_empty() {
                    continue;
                }
                stats.linked += 1;
                for target in hits {
                    let key = (f.source.uid.as_str(), target.uid.as_str());
                    let matches = edges.entry(key).or_insert_with(|| {
                        order.push(key);
                        Vec::new()
                    });
                    matches.push(MentionMatch {
                        line: f.line,
                        context: f.context.clone(),
                        method: METHOD.to_string(),
                        text: Some(f.text.clone()),
                        syntax: Some(f.syntax.name().to_string()),
                    });
                }
            }
        }
    }
    let relationships = order
        .into_iter()
        .map(|key| {
            let matches = edges.remove(&key).unwrap_or_default();
            let metadata = MentionMetadata {
                count: matches.len(),
                matches,
                extraction_method: METHOD.to_string(),
            };
            Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: key.0.to_string(),
                target_uid: key.1.to_string(),
                kind: RelationshipKind::Mentions,
                repo_id: repo_id.to_string(),
                metadata: serde_json::to_string(&metadata).unwrap_or_default(),
            }
        })
        .collect();
    Extracted {
        relationships,
        stats,
    }
}

// ── Reading the edges ─────────────────────────────────────────────────

/// A doc that mentions a symbol: the section, document or documented symbol
/// the edge starts at, the symbol it names, and the line of its first
/// mention there.
#[derive(Debug, Clone, Copy)]
pub struct DocHit<'a> {
    /// A `Section` or `Document`, or a code symbol whose doc comment it is.
    pub source: &'a CodeSymbol,
    /// The symbol mentioned.
    pub target: &'a CodeSymbol,
    /// Line of the first mention.
    pub line: u32,
}

impl DocHit<'_> {
    /// The mention is in a doc comment, not a document.
    pub fn in_comment(&self) -> bool {
        !matches!(self.source.kind, SymbolKind::Section | SymbolKind::Document)
    }
}

/// The symbols whose docs a change to `roots` concerns: each root and, for
/// a type, its members.
pub fn mention_targets<'a>(
    symbols: &'a [CodeSymbol],
    roots: &[&'a CodeSymbol],
) -> Vec<&'a CodeSymbol> {
    let mut out: Vec<&CodeSymbol> = roots.to_vec();
    for root in roots.iter().filter(|r| is_type(&r.kind)) {
        let prefix = format!("{}.", root.qualified_name);
        out.extend(
            symbols
                .iter()
                .filter(|s| s.file_path == root.file_path && s.qualified_name.starts_with(&prefix)),
        );
    }
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.uid.as_str()));
    out
}

/// The docs that mention any of `targets` (doc `MENTIONS` edges, #180):
/// one hit per (doc, symbol), documents first, then doc comments, each by
/// file and line.
pub fn docs_mentioning<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    targets: &[&'a CodeSymbol],
) -> Vec<DocHit<'a>> {
    let wanted: HashMap<&str, &CodeSymbol> = targets.iter().map(|t| (t.uid.as_str(), *t)).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let edges: Vec<&Relationship> = relationships
        .iter()
        .filter(|r| {
            r.kind == RelationshipKind::Mentions
                && wanted.contains_key(r.target_uid.as_str())
                && r.metadata.contains(METHOD)
        })
        .collect();
    if edges.is_empty() {
        return Vec::new();
    }
    let sources: HashSet<&str> = edges.iter().map(|r| r.source_uid.as_str()).collect();
    let by_uid: HashMap<&str, &CodeSymbol> = symbols
        .iter()
        .filter(|s| sources.contains(s.uid.as_str()))
        .map(|s| (s.uid.as_str(), s))
        .collect();
    let mut hits: Vec<DocHit<'a>> = edges
        .into_iter()
        .filter_map(|r| {
            let meta: MentionMetadata = serde_json::from_str(&r.metadata).ok()?;
            if meta.extraction_method != METHOD {
                return None;
            }
            let line = meta.matches.iter().map(|m| m.line).min()?;
            Some(DocHit {
                source: by_uid.get(r.source_uid.as_str())?,
                target: wanted.get(r.target_uid.as_str())?,
                line,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        (a.in_comment(), a.source.file_path.as_str(), a.line).cmp(&(
            b.in_comment(),
            b.source.file_path.as_str(),
            b.line,
        ))
    });
    hits
}

// ── Finding mentions ──────────────────────────────────────────────────

/// The documents' markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Markup {
    Markdown,
    AsciiDoc,
}

fn markup_of(path: &str) -> Option<Markup> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => Some(Markup::Markdown),
        "adoc" | "asciidoc" => Some(Markup::AsciiDoc),
        _ => None,
    }
}

fn find_mentions(sources: &[CodeSymbol]) -> Vec<Found<'_>> {
    let mut sections: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
    for s in sources.iter().filter(|s| s.kind == SymbolKind::Section) {
        sections.entry(s.file_path.as_str()).or_default().push(s);
    }
    let mut found = Vec::new();
    for symbol in sources {
        match symbol.kind {
            SymbolKind::Document => {
                let Some(markup) = markup_of(&symbol.file_path) else {
                    continue;
                };
                let in_file = sections
                    .get(symbol.file_path.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for (line, text, context, syntax) in scan_document(&symbol.content, markup) {
                    // The innermost section around the line: the one that
                    // starts last.
                    let source = in_file
                        .iter()
                        .filter(|s| s.start_line <= line && line <= s.end_line)
                        .max_by_key(|s| s.start_line)
                        .copied()
                        .unwrap_or(symbol);
                    found.push(Found {
                        source,
                        line,
                        text,
                        context,
                        syntax,
                    });
                }
            }
            SymbolKind::Section | SymbolKind::Email => {}
            _ => {
                let Some(json) = symbol.metadata.as_deref() else {
                    continue;
                };
                if !json.contains("\"doc_refs\"") {
                    continue;
                }
                let Ok(metadata) = serde_json::from_str::<SymbolMetadata>(json) else {
                    continue;
                };
                for r in metadata.doc_refs {
                    found.push(Found {
                        source: symbol,
                        line: r.line,
                        context: r.target.clone(),
                        text: r.target,
                        syntax: Syntax::DocComment,
                    });
                }
            }
        }
    }
    found
}

static ROLE: OnceLock<Regex> = OnceLock::new();
static API_LINK: OnceLock<Regex> = OnceLock::new();
static KDOC_LINK: OnceLock<Regex> = OnceLock::new();
static JAVADOC_MACRO: OnceLock<Regex> = OnceLock::new();

/// `(line, text, context, syntax)` of every mention in a document, outside
/// code blocks, comments and attribute lines.
fn scan_document(content: &str, markup: Markup) -> Vec<(u32, String, String, Syntax)> {
    let role = ROLE.get_or_init(|| {
        Regex::new(r"(?:(?::[A-Za-z][\w-]*)+:|\{[A-Za-z][\w:-]*\})`([^`\n]+)`")
            .expect("valid regex")
    });
    let api = API_LINK.get_or_init(|| {
        Regex::new(r"(?:\{[\w-]*api[\w-]*\}\+*/?|/javadoc-api/)([A-Za-z_][\w/.$]*?)\.html(?:#([^\[\s+]*))?")
            .expect("valid regex")
    });
    let kdoc = KDOC_LINK.get_or_init(|| {
        Regex::new(r"\{[\w-]*kdoc[\w-]*\}\+*/?[\w.-]+/([A-Za-z_][\w.]*)/([\w/.-]*?)\.html")
            .expect("valid regex")
    });
    let javadoc = JAVADOC_MACRO
        .get_or_init(|| Regex::new(r"javadoc:([A-Za-z_][^\[\s]*)\[").expect("valid regex"));
    let mut out = Vec::new();
    let mut in_block: Option<&str> = None;
    for (idx, line) in content.lines().enumerate() {
        let n = idx as u32 + 1;
        let trimmed = line.trim();
        match markup {
            Markup::Markdown => {
                if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                    in_block = if in_block.is_some() {
                        None
                    } else {
                        Some("```")
                    };
                    continue;
                }
            }
            Markup::AsciiDoc => {
                if let Some(d) = adoc_block_delimiter(line) {
                    match in_block {
                        None => in_block = Some(d),
                        Some(open) if open == d => in_block = None,
                        Some(_) => {}
                    }
                    continue;
                }
                if in_block.is_none()
                    && (trimmed.starts_with("//")
                        || trimmed.starts_with("include::")
                        || is_attribute_entry(trimmed))
                {
                    continue;
                }
            }
        }
        if in_block.is_some() {
            continue;
        }
        let context = || crate::text::prefix_chars(trimmed, 160).to_string();
        let mut masked = line.to_string();
        if markup == Markup::Markdown {
            for c in role.captures_iter(line) {
                let whole = c.get(0).expect("match");
                out.push((n, role_target(&c[1]), context(), Syntax::Role));
                masked.replace_range(whole.range(), &" ".repeat(whole.len()));
            }
        } else {
            for c in api.captures_iter(line) {
                let mut text = c[1].replace('/', ".");
                if let Some(anchor) = c.get(2).map(|a| a.as_str()) {
                    let member = anchor.split('(').next().unwrap_or("");
                    if is_identifier(member) {
                        text.push('#');
                        text.push_str(member);
                    }
                }
                out.push((n, text, context(), Syntax::ApiLink));
            }
            for c in kdoc.captures_iter(line) {
                if let Some(text) = kdoc_target(&c[1], &c[2]) {
                    out.push((n, text, context(), Syntax::ApiLink));
                }
            }
            for c in javadoc.captures_iter(line) {
                out.push((n, c[1].to_string(), context(), Syntax::ApiLink));
            }
        }
        for span in code_spans(&masked, markup) {
            out.push((n, span, context(), Syntax::CodeSpan));
        }
    }
    out
}

/// The symbol a Dokka (KDoc) page names: `package` and the page path under
/// it, `-co-web-filter/index` (a class, `CoWebFilter`), `-foo/bar-baz` (a
/// member `Foo.barBaz`), `co-router` (a top-level function `coRouter`);
/// `None` for the package's own index page.
fn kdoc_target(package: &str, page: &str) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    for part in page.split('/').filter(|p| !p.is_empty() && *p != "index") {
        let (class, words) = match part.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, part),
        };
        let mut name = String::new();
        for (i, word) in words.split('-').filter(|w| !w.is_empty()).enumerate() {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                if class || i > 0 {
                    name.extend(first.to_uppercase());
                } else {
                    name.push(first);
                }
                name.push_str(chars.as_str());
            }
        }
        if !is_identifier(&name) {
            return None;
        }
        names.push(name);
    }
    if names.is_empty() {
        return None;
    }
    Some(format!("{package}.{}", names.join(".")))
}

/// An AsciiDoc attribute entry (`:name: value`) or block attribute line
/// (`[source,java]`): no prose.
fn is_attribute_entry(line: &str) -> bool {
    if line.starts_with('[') && line.ends_with(']') {
        return true;
    }
    let Some(rest) = line.strip_prefix(':') else {
        return false;
    };
    let Some((name, _)) = rest.split_once(':') else {
        return false;
    };
    let name = name.trim_end_matches('!').trim_start_matches('!');
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// The target of a role's text: `Title <pkg.Foo>` names `pkg.Foo`; `~` and
/// `!` prefixes only change the rendering.
fn role_target(text: &str) -> String {
    let text = text.trim();
    let target = match (text.rfind('<'), text.ends_with('>')) {
        (Some(open), true) => &text[open + 1..text.len() - 1],
        _ => text,
    };
    target.trim_start_matches(['~', '!']).to_string()
}

/// The inline code spans of a line: a run of backticks up to the next run of
/// the same length. In AsciiDoc a single-backtick span is constrained (no
/// word character outside it), `` `+x+` `` is a passthrough of `x`, and
/// `` "`x`" `` / `` '`x`' `` are curved quotes, not code.
fn code_spans(line: &str, markup: Markup) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] == b'`' {
            i += 1;
        }
        let run = i - start;
        // The closing run of the same length.
        let mut j = i;
        let mut close = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let s = j;
                while j < bytes.len() && bytes[j] == b'`' {
                    j += 1;
                }
                if j - s == run {
                    close = Some(s);
                    break;
                }
            } else {
                j += 1;
            }
        }
        let Some(close) = close else {
            continue;
        };
        let end = close + run;
        let inner = &line[i..close];
        i = end;
        if markup == Markup::AsciiDoc {
            let before = line[..start].chars().next_back();
            let after = line[end..].chars().next();
            if matches!(before, Some('"' | '\'')) || matches!(after, Some('"' | '\'')) {
                continue;
            }
            if run == 1
                && (before.is_some_and(|c| c.is_alphanumeric())
                    || after.is_some_and(|c| c.is_alphanumeric()))
            {
                continue;
            }
        }
        let inner = inner.trim();
        let inner = if markup == Markup::AsciiDoc {
            inner
                .strip_prefix('+')
                .and_then(|s| s.strip_suffix('+'))
                .unwrap_or(inner)
        } else {
            inner
        };
        if !inner.is_empty() {
            spans.push(inner.to_string());
        }
    }
    spans
}

// ── Parsing a reference ───────────────────────────────────────────────

/// A mention parsed into name segments: `pkg.Foo#bar(String)` is
/// `[pkg, Foo, bar]` with `call`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ref {
    segments: Vec<String>,
    /// Written as a call (`bar()`).
    call: bool,
    /// Written as an annotation (`@Bean`).
    annotation: bool,
    /// Javadoc `#member`: a member of the documented type.
    this_type: bool,
    syntax: Syntax,
}

impl Ref {
    fn parse(text: &str, syntax: Syntax) -> Option<Ref> {
        let mut t = text.trim();
        // XML-doc cref kinds: `T:`, `M:`, `P:`, `F:`, `E:`.
        if syntax == Syntax::DocComment {
            if let Some(rest) = t
                .strip_prefix("T:")
                .or_else(|| t.strip_prefix("M:"))
                .or_else(|| t.strip_prefix("P:"))
                .or_else(|| t.strip_prefix("F:"))
                .or_else(|| t.strip_prefix("E:"))
            {
                t = rest;
            }
        }
        let annotation = t.starts_with('@');
        t = t.trim_start_matches('@');
        let this_type = syntax == Syntax::DocComment && t.starts_with('#');
        t = t.trim_start_matches('#');
        let mut call = false;
        if let Some(open) = t.find('(') {
            if !t.ends_with(')') {
                // `{@link Foo#bar(String, int)}` is cut at the space.
                if syntax != Syntax::DocComment {
                    return None;
                }
            }
            call = true;
            t = &t[..open];
        }
        // Generic arguments and array brackets: `List<String>`, `Foo[]`.
        if let Some(open) = t.find('<') {
            if !t.ends_with('>') {
                return None;
            }
            t = &t[..open];
        }
        t = t.trim_end_matches("[]");
        if t.is_empty() {
            return None;
        }
        let mut segments: Vec<String> = Vec::new();
        for seg in t.split(['.', '#']).flat_map(|s| s.split("::")) {
            if !is_identifier(seg) {
                return None;
            }
            segments.push(seg.to_string());
        }
        // Rust path roots name no item.
        while segments.len() > 1 && matches!(segments[0].as_str(), "crate" | "self" | "super") {
            segments.remove(0);
        }
        if segments.is_empty() || segments.len() > 12 {
            return None;
        }
        Some(Ref {
            segments,
            call,
            annotation,
            this_type,
            syntax,
        })
    }

    fn with_owner(mut self, owner: &str) -> Ref {
        self.segments.insert(0, owner.to_string());
        self.this_type = false;
        self
    }

    fn name(&self) -> &str {
        self.segments.last().map(String::as_str).unwrap_or("")
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Whether a bare word reads as code rather than as an English word:
/// camelCase, PascalCase, snake_case, ALL_CAPS or with digits. A single
/// capitalised word (`Resource`) counts: in a code span that is a type.
fn looks_like_code(name: &str) -> bool {
    let has_upper = name.chars().any(|c| c.is_uppercase());
    let has_lower = name.chars().any(|c| c.is_lowercase());
    name.contains('_')
        || (has_upper && has_lower)
        || (has_upper && name.len() >= 3)
        || name.chars().any(|c| c.is_ascii_digit())
}

/// Annotation names the JDK, Jakarta EE, JSR 305 or JSpecify declare and
/// projects declare again (Spring's deprecated `org.springframework.lang.Nullable`):
/// a bare `@Name` of these is ambiguous, never linked.
const SHARED_ANNOTATIONS: &[&str] = &[
    "Nullable",
    "NonNull",
    "Nonnull",
    "NotNull",
    "NullMarked",
    "NullUnmarked",
    "Inject",
    "Named",
    "Singleton",
    "Qualifier",
    "Scope",
    "Valid",
    "PostConstruct",
    "PreDestroy",
    "Resource",
    "Generated",
    "Override",
    "Deprecated",
    "FunctionalInterface",
    "SuppressWarnings",
    "SafeVarargs",
    "Documented",
    "Retention",
    "Target",
    "Inherited",
];

/// Words a bare code span holds that name no project symbol even when one
/// is declared with that name: literals, and the standard library types
/// docs name all the time that a project (or a library it vendors) may
/// shadow. Written qualified or as a link they still resolve.
const NOT_NAMES: &[&str] = &[
    "true",
    "false",
    "null",
    "None",
    "True",
    "False",
    "nil",
    "this",
    "self",
    "super",
    "void",
    "String",
    "Object",
    "Integer",
    "Boolean",
    "Long",
    "List",
    "Map",
    "Set",
    "Optional",
    "Collection",
    "Iterable",
    "Iterator",
    "Stream",
    "Function",
    "BiFunction",
    "Predicate",
    "Supplier",
    "Consumer",
    "BiConsumer",
    "Runnable",
    "Callable",
    "Executor",
    "Future",
    "CompletableFuture",
    "Throwable",
    "Exception",
    "RuntimeException",
    "Error",
    "Class",
    "Thread",
    "Comparator",
    "Comparable",
    "Closeable",
    "AutoCloseable",
    "Record",
    "Enum",
    "Number",
    "Type",
    "Duration",
    "Instant",
    "Path",
    "File",
    "URI",
    "URL",
];

// ── Resolving ─────────────────────────────────────────────────────────

enum Outcome<'a> {
    /// Not a code name: a literal, a property key, markup.
    NotCode,
    /// A code name of nothing in the project.
    Unresolved,
    /// Several symbols, none preferred.
    Ambiguous,
    /// The symbol, or the overloads of one method.
    Linked(Vec<&'a CodeSymbol>),
}

/// The kinds a mention can name.
fn is_target(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
            | SymbolKind::Constant
            | SymbolKind::Enum
    )
}

fn is_type(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum | SymbolKind::TypeAlias
    )
}

/// Whether a type is declared as an annotation (Java `@interface`, Kotlin
/// `annotation class`).
fn is_annotation_type(symbol: &CodeSymbol) -> bool {
    let declaration = if symbol.signature.is_empty() {
        crate::text::prefix_chars(&symbol.content, 400)
    } else {
        symbol.signature.as_str()
    };
    declaration.contains("@interface") || declaration.contains("annotation class")
}

/// File stems that name the directory's module, not a type.
fn is_module_file(stem: &str) -> bool {
    matches!(
        stem,
        "mod" | "lib" | "main" | "index" | "__init__" | "package-info"
    )
}

/// Languages where a file is no namespace: a name is qualified by its
/// package (the directories) and its types, not by the file it is in
/// (Kotlin top-level functions, Go, C#).
fn file_is_namespace(file: &str) -> bool {
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    !matches!(
        ext,
        "java" | "kt" | "kts" | "scala" | "go" | "cs" | "swift" | "php"
    )
}

/// A symbol's container path: directories, the file stem (where a file is a
/// namespace and the first enclosing type does not repeat it), and the
/// qualified name's segments.
fn path_of(symbol: &CodeSymbol) -> Vec<&str> {
    let mut path: Vec<&str> = Vec::new();
    let (dirs, file) = symbol
        .file_path
        .rsplit_once('/')
        .unwrap_or(("", symbol.file_path.as_str()));
    path.extend(dirs.split('/').filter(|d| !d.is_empty()));
    let stem = file.split('.').next().unwrap_or(file);
    let qualified: Vec<&str> = symbol
        .qualified_name
        .split(['.', ':'])
        .filter(|s| !s.is_empty())
        .collect();
    if file_is_namespace(file) && !is_module_file(stem) && qualified.first() != Some(&stem) {
        path.push(stem);
    }
    path.extend(qualified);
    path
}

/// The name of the type that declares `symbol`, if it has one.
fn parent_name(symbol: &CodeSymbol) -> Option<&str> {
    let mut parts = symbol
        .qualified_name
        .rsplit(['.', ':'])
        .filter(|s| !s.is_empty());
    parts.next()?;
    parts.next()
}

/// The type a doc comment's `#member` belongs to: the documented type
/// itself, or the type declaring the documented member.
fn own_type(symbol: &CodeSymbol) -> Option<&str> {
    if is_type(&symbol.kind) {
        Some(symbol.name.as_str())
    } else {
        parent_name(symbol)
    }
}

/// The documented symbol's enclosing type names: owners its doc comment may
/// name members of without writing them.
fn owner_chain(symbol: &CodeSymbol) -> Vec<String> {
    let mut names: Vec<String> = symbol
        .qualified_name
        .split(['.', ':'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if !is_type(&symbol.kind) {
        names.pop();
    }
    names
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

struct TargetIndex<'a> {
    by_name: HashMap<&'a str, Vec<&'a CodeSymbol>>,
}

impl<'a> TargetIndex<'a> {
    fn new(targets: &'a [CodeSymbol]) -> Self {
        let mut by_name: HashMap<&str, Vec<&CodeSymbol>> = HashMap::new();
        for t in targets.iter().filter(|t| is_target(&t.kind)) {
            by_name.entry(t.name.as_str()).or_default().push(t);
        }
        Self { by_name }
    }

    fn resolve(&self, r: &Ref, found: &Found<'_>, owners: Option<&HashSet<String>>) -> Outcome<'a> {
        let name = r.name();
        if r.segments.len() == 1 {
            let reads_as_code = r.syntax.explicit()
                || r.call
                || r.annotation
                || (looks_like_code(name) && name.len() >= 3);
            if !reads_as_code || NOT_NAMES.contains(&name) {
                return Outcome::NotCode;
            }
        }
        let candidates = self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[]);
        let mut matched: Vec<&'a CodeSymbol> = if r.segments.len() == 1 {
            candidates.to_vec()
        } else {
            self.qualified(candidates, &r.segments)
        };
        if matched.is_empty() && r.segments.len() > 1 {
            // `Propagation.NEVER`: an enum constant or a field is no symbol;
            // the type that declares it is.
            if name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            {
                let owner = &r.segments[r.segments.len() - 2];
                let types: Vec<&CodeSymbol> = self
                    .by_name
                    .get(owner.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .filter(|t| is_type(&t.kind))
                    .collect();
                matched = self.qualified(&types, &r.segments[..r.segments.len() - 1]);
            }
        }
        if matched.is_empty() {
            return Outcome::Unresolved;
        }
        // `@Name` is an annotation: of the types, only annotation types
        // qualify (`@Resource` is Jakarta's, not a `Resource` interface);
        // a function stays (a Python decorator).
        if r.annotation {
            matched.retain(|s| !is_type(&s.kind) || is_annotation_type(s));
            if matched.is_empty() {
                return Outcome::Unresolved;
            }
            // `@Nullable`, `@Inject`, ...: the JDK, Jakarta and JSpecify
            // declare them too, and a bare name cannot say whose is meant.
            if r.segments.len() == 1 && SHARED_ANNOTATIONS.contains(&name) {
                return Outcome::Ambiguous;
            }
        }
        // A constructor carries its type's name: `Foo` names the type `Foo`,
        // not also each `Foo(...)` declared in it.
        let types: Vec<(&str, &str)> = matched
            .iter()
            .filter(|s| is_type(&s.kind))
            .map(|s| (s.file_path.as_str(), s.name.as_str()))
            .collect();
        if !types.is_empty() {
            matched.retain(|s| {
                is_type(&s.kind)
                    || !types.contains(&(s.file_path.as_str(), s.name.as_str()))
                    || parent_name(s) != Some(s.name.as_str())
            });
        }
        if let Some(one) = one_group(&matched) {
            return Outcome::Linked(one);
        }
        // Product code over tests.
        let product: Vec<&CodeSymbol> = matched.iter().copied().filter(|s| !s.is_test()).collect();
        if !product.is_empty() {
            if let Some(one) = one_group(&product) {
                return Outcome::Linked(one);
            }
            matched = product;
        }
        // An owner named nearby.
        if let Some(owners) = owners {
            let owned: Vec<&CodeSymbol> = matched
                .iter()
                .copied()
                .filter(|s| parent_name(s).is_some_and(|p| p != name && owners.contains(p)))
                .collect();
            if let Some(one) = one_group(&owned) {
                return Outcome::Linked(one);
            }
        }
        // A doc comment's link resolves like the compiler would look: the
        // same file, then the same package.
        if found.syntax == Syntax::DocComment {
            let file = found.source.file_path.as_str();
            let same_file: Vec<&CodeSymbol> = matched
                .iter()
                .copied()
                .filter(|s| s.file_path == file)
                .collect();
            if let Some(one) = one_group(&same_file) {
                return Outcome::Linked(one);
            }
            let dir = directory(file);
            let same_dir: Vec<&CodeSymbol> = matched
                .iter()
                .copied()
                .filter(|s| directory(&s.file_path) == dir)
                .collect();
            if let Some(one) = one_group(&same_dir) {
                return Outcome::Linked(one);
            }
        }
        Outcome::Ambiguous
    }

    /// The candidates whose container path ends with `segments`; when none
    /// does, with fewer leading (package) segments, down to owner and name.
    fn qualified(&self, candidates: &[&'a CodeSymbol], segments: &[String]) -> Vec<&'a CodeSymbol> {
        let paths: Vec<Vec<&str>> = candidates.iter().map(|c| path_of(c)).collect();
        for skip in 0..segments.len().saturating_sub(1).max(1) {
            let want = &segments[skip..];
            let hits: Vec<&CodeSymbol> = candidates
                .iter()
                .zip(&paths)
                .filter(|(_, path)| {
                    path.len() >= want.len()
                        && path[path.len() - want.len()..]
                            .iter()
                            .zip(want)
                            .all(|(a, b)| *a == b.as_str())
                })
                .map(|(c, _)| *c)
                .collect();
            if !hits.is_empty() {
                return hits;
            }
        }
        Vec::new()
    }
}

/// `symbols` when they are one symbol or the overloads of one method (one
/// file, one qualified name); `None` otherwise.
fn one_group<'a>(symbols: &[&'a CodeSymbol]) -> Option<Vec<&'a CodeSymbol>> {
    let first = symbols.first()?;
    symbols
        .iter()
        .all(|s| s.file_path == first.file_path && s.qualified_name == first.qualified_name)
        .then(|| symbols.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(file: &str, qualified: &str, kind: SymbolKind, lines: (u32, u32)) -> CodeSymbol {
        let name = qualified.rsplit('.').next().unwrap().to_string();
        CodeSymbol {
            uid: format!("{file}::{qualified}:{}", lines.0),
            name,
            qualified_name: qualified.to_string(),
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

    fn doc(file: &str, content: &str) -> CodeSymbol {
        let mut d = sym(file, "document", SymbolKind::Document, (1, 1000));
        d.content = content.to_string();
        d
    }

    fn section(file: &str, name: &str, lines: (u32, u32)) -> CodeSymbol {
        sym(file, name, SymbolKind::Section, lines)
    }

    /// `(source name, target qualified name, line)` of every edge.
    fn links(sources: &[CodeSymbol], targets: &[CodeSymbol]) -> Vec<(String, String, u32)> {
        let all: Vec<CodeSymbol> = sources.iter().chain(targets).cloned().collect();
        let by_uid: HashMap<&str, &CodeSymbol> = all.iter().map(|s| (s.uid.as_str(), s)).collect();
        let mut out: Vec<(String, String, u32)> = extract(sources, &all, "r")
            .relationships
            .iter()
            .flat_map(|r| {
                let meta: MentionMetadata = serde_json::from_str(&r.metadata).unwrap();
                let source = by_uid[r.source_uid.as_str()].name.clone();
                let target = by_uid[r.target_uid.as_str()].qualified_name.clone();
                meta.matches
                    .into_iter()
                    .map(move |m| (source.clone(), target.clone(), m.line))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn docs_mentioning_lists_documents_before_doc_comments() {
        let mut documented = sym("src/Other.java", "Other.run", SymbolKind::Method, (20, 30));
        documented.metadata = Some(
            serde_json::to_string(&SymbolMetadata {
                doc_refs: vec![myceliums_storage::DocRef {
                    target: "Store#open".into(),
                    line: 18,
                }],
                ..Default::default()
            })
            .unwrap(),
        );
        let store = sym("src/Store.java", "Store", SymbolKind::Class, (1, 50));
        let open = sym("src/Store.java", "Store.open", SymbolKind::Method, (10, 20));
        let sources = vec![
            documented,
            doc("docs/a.md", "## Store\n\nUse `Store`.\n"),
            section("docs/a.md", "Store", (1, 3)),
        ];
        let all: Vec<CodeSymbol> = [sources.clone(), vec![store.clone(), open.clone()]].concat();
        let rels = extract(&sources, &all, "r").relationships;
        let store = all
            .iter()
            .find(|s| s.qualified_name == "Store" && s.kind == SymbolKind::Class)
            .unwrap();
        let targets = mention_targets(&all, &[store]);
        assert_eq!(targets.len(), 2, "the class and its method");
        let hits: Vec<(String, String, u32, bool)> = docs_mentioning(&all, &rels, &targets)
            .iter()
            .map(|h| {
                (
                    h.source.name.clone(),
                    h.target.qualified_name.clone(),
                    h.line,
                    h.in_comment(),
                )
            })
            .collect();
        assert_eq!(
            hits,
            vec![
                ("Store".to_string(), "Store".to_string(), 3, false),
                ("run".to_string(), "Store.open".to_string(), 18, true),
            ]
        );
    }

    #[test]
    fn kdoc_links_name_classes_members_and_top_level_functions() {
        assert_eq!(
            kdoc_target("org.springframework.web.server", "-co-web-filter/index").as_deref(),
            Some("org.springframework.web.server.CoWebFilter")
        );
        assert_eq!(
            kdoc_target("org.acme", "-store/open-all").as_deref(),
            Some("org.acme.Store.openAll")
        );
        assert_eq!(
            kdoc_target("org.acme.server", "co-router").as_deref(),
            Some("org.acme.server.coRouter")
        );
        assert_eq!(kdoc_target("org.acme", "index"), None);
        let targets = vec![
            sym(
                "web/src/main/kotlin/org/acme/server/CoWebFilter.kt",
                "CoWebFilter",
                SymbolKind::Class,
                (1, 9),
            ),
            // A top-level function: the file is no namespace in Kotlin.
            sym(
                "web/src/main/kotlin/org/acme/server/RouterFunctionDsl.kt",
                "coRouter",
                SymbolKind::Function,
                (3, 9),
            ),
        ];
        let content = "== Kotlin\n\nSee {spring-framework-api-kdoc}/web/org.acme.server/-co-web-filter/index.html[filter],\n{spring-framework-api-kdoc}/web/org.acme.server/co-router.html[coRouter DSL]\nand {spring-framework-api-kdoc}/[the index].\n";
        let sources = vec![
            doc("docs/kotlin.adoc", content),
            section("docs/kotlin.adoc", "Kotlin", (1, 5)),
        ];
        assert_eq!(
            links(&sources, &targets),
            vec![
                ("Kotlin".to_string(), "CoWebFilter".to_string(), 3),
                ("Kotlin".to_string(), "coRouter".to_string(), 4),
            ]
        );
    }

    #[test]
    fn hard_names_stay_ambiguous_or_link_by_their_owner() {
        let mut nullable = sym(
            "core/src/main/java/org/acme/lang/Nullable.java",
            "Nullable",
            SymbolKind::Interface,
            (1, 9),
        );
        nullable.signature = "public @interface Nullable".to_string();
        let targets = vec![
            nullable,
            // `get()` and `of()`: many owners.
            sym(
                "src/main/java/org/acme/Store.java",
                "Store.get",
                SymbolKind::Method,
                (3, 4),
            ),
            sym(
                "src/main/java/org/acme/Cache.java",
                "Cache.get",
                SymbolKind::Method,
                (3, 4),
            ),
            sym(
                "src/main/java/org/acme/Store.java",
                "Store.of",
                SymbolKind::Method,
                (5, 6),
            ),
            sym(
                "src/main/java/org/acme/Cache.java",
                "Cache.of",
                SymbolKind::Method,
                (5, 6),
            ),
            // Nested builders of two types.
            sym(
                "src/main/java/org/acme/WebClient.java",
                "WebClient",
                SymbolKind::Interface,
                (1, 90),
            ),
            sym(
                "src/main/java/org/acme/WebClient.java",
                "WebClient.Builder",
                SymbolKind::Interface,
                (10, 40),
            ),
            sym(
                "src/main/java/org/acme/RestClient.java",
                "RestClient.Builder",
                SymbolKind::Interface,
                (10, 40),
            ),
            // Overloads within one type, and the same name in another type.
            sym(
                "src/main/java/org/acme/Store.java",
                "Store.open",
                SymbolKind::Method,
                (7, 8),
            ),
            sym(
                "src/main/java/org/acme/Store.java",
                "Store.open",
                SymbolKind::Method,
                (9, 10),
            ),
            sym(
                "src/main/java/org/acme/Cache.java",
                "Cache.open",
                SymbolKind::Method,
                (7, 8),
            ),
        ];
        let content = "== One\n\nMark it `@Nullable`; call `get()` and `of()`; a `Builder` too.\n\n== Two\n\nA `WebClient` has a `Builder`; `Store#open(String)` opens.\n\n== Three\n\nBoth `Store` and `Cache` have `open()`.\n";
        let sources = vec![
            doc("docs/hard.adoc", content),
            section("docs/hard.adoc", "One", (1, 4)),
            section("docs/hard.adoc", "Two", (5, 8)),
            section("docs/hard.adoc", "Three", (9, 11)),
        ];
        let all: Vec<CodeSymbol> = [sources.clone(), targets.clone()].concat();
        let found = links(&sources, &targets);
        // `Store#open(String)` links both overloads of Store.open, nothing else.
        assert_eq!(
            found,
            vec![
                ("Two".to_string(), "Store.open".to_string(), 7),
                ("Two".to_string(), "WebClient".to_string(), 7),
                ("Two".to_string(), "WebClient.Builder".to_string(), 7),
            ]
        );
        let extracted = extract(&sources, &all, "r");
        let open_edges = extracted
            .relationships
            .iter()
            .filter(|r| r.target_uid.contains("Store.open"))
            .count();
        assert_eq!(open_edges, 2, "both overloads");
        let ambiguous: Vec<&str> = extracted
            .stats
            .ambiguous_by_name
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(ambiguous, vec!["Builder", "Nullable", "get", "of", "open"]);
    }

    #[test]
    fn code_spans_follow_markdown_and_asciidoc_rules() {
        assert_eq!(
            code_spans("Use `Foo` and ``a ` b`` or `unclosed", Markup::Markdown),
            vec!["Foo", "a ` b"]
        );
        assert_eq!(
            code_spans(r#"the "`why`" and `+Bar+`, not x`y`z"#, Markup::AsciiDoc),
            vec!["Bar"]
        );
    }

    #[test]
    fn references_parse_into_segments() {
        let r = Ref::parse("org.example.Foo#bar(String, int)", Syntax::CodeSpan).unwrap();
        assert_eq!(r.segments, ["org", "example", "Foo", "bar"]);
        assert!(r.call);
        let r = Ref::parse("@Transactional", Syntax::CodeSpan).unwrap();
        assert!(r.annotation);
        assert_eq!(
            Ref::parse("crate::store::Store::open", Syntax::DocComment)
                .unwrap()
                .segments,
            ["store", "Store", "open"]
        );
        assert_eq!(
            Ref::parse("T:Acme.Orders.OrderService", Syntax::DocComment)
                .unwrap()
                .segments,
            ["Acme", "Orders", "OrderService"]
        );
        assert!(Ref::parse("#bar", Syntax::DocComment).unwrap().this_type);
        for not_code in [
            "urlDecode=false",
            "<aop:aspect>",
            "?.",
            "spring aot",
            "\"Forwarded\"",
            "a-b",
        ] {
            assert_eq!(Ref::parse(not_code, Syntax::CodeSpan), None, "{not_code}");
        }
    }

    #[test]
    fn a_markdown_section_links_qualified_and_unique_names() {
        let targets = vec![
            sym("src/store.rs", "Store", SymbolKind::Class, (1, 50)),
            sym("src/store.rs", "Store.open", SymbolKind::Method, (10, 20)),
            sym("src/cache.rs", "Cache.open", SymbolKind::Method, (5, 9)),
            sym("src/util.rs", "import_string", SymbolKind::Function, (1, 3)),
        ];
        let content = "# Guide\n\n## Storage\n\nCall `Store::open()` or `import_string`.\n\n## Caching\n\nA bare `open()` is ambiguous here.\n\n```rust\nlet s = `Store`;\n```\n\nThe word Store in prose is not a link.\n";
        let sources = vec![
            doc("docs/guide.md", content),
            section("docs/guide.md", "Guide", (1, 15)),
            section("docs/guide.md", "Storage", (3, 6)),
            section("docs/guide.md", "Caching", (7, 15)),
        ];
        assert_eq!(
            links(&sources, &targets),
            vec![
                ("Storage".to_string(), "Store.open".to_string(), 5),
                ("Storage".to_string(), "import_string".to_string(), 5),
            ]
        );
        let stats = extract(&sources, &[sources.clone(), targets].concat(), "r").stats;
        assert_eq!(stats.ambiguous, 1);
        assert_eq!(stats.ambiguous_by_name["open"].at, vec!["docs/guide.md:9"]);
    }

    #[test]
    fn a_bare_member_links_when_its_owner_is_named_in_the_section() {
        let targets = vec![
            sym("src/store.rs", "Store", SymbolKind::Class, (1, 50)),
            // A constructor: `Store` still names one thing, the type.
            sym("src/store.rs", "Store.Store", SymbolKind::Method, (3, 5)),
            sym("src/store.rs", "Store.open", SymbolKind::Method, (10, 20)),
            sym("src/cache.rs", "Cache.open", SymbolKind::Method, (5, 9)),
        ];
        let content =
            "## Store\n\nA `Store` is opened with `open()`.\n\n## Other\n\nThen `open()` again.\n";
        let sources = vec![
            doc("docs/a.md", content),
            section("docs/a.md", "Store", (1, 4)),
            section("docs/a.md", "Other", (5, 7)),
        ];
        assert_eq!(
            links(&sources, &targets),
            vec![
                ("Store".to_string(), "Store".to_string(), 3),
                ("Store".to_string(), "Store.open".to_string(), 3),
            ]
        );
    }

    #[test]
    fn asciidoc_code_spans_api_links_and_javadoc_macros_link() {
        let mut targets = vec![
            sym(
                "spring-core/src/main/java/org/springframework/core/MethodParameter.java",
                "MethodParameter",
                SymbolKind::Class,
                (1, 900),
            ),
            sym(
                "spring-core/src/main/java/org/springframework/core/MethodParameter.java",
                "MethodParameter.isOptional",
                SymbolKind::Method,
                (100, 110),
            ),
            sym(
                "spring-tx/src/main/java/org/springframework/transaction/annotation/Transactional.java",
                "Transactional",
                SymbolKind::Interface,
                (1, 90),
            ),
            sym(
                "spring-tx/src/main/java/org/springframework/transaction/annotation/Propagation.java",
                "Propagation",
                SymbolKind::Enum,
                (1, 90),
            ),
            sym("other/src/main/java/org/acme/core/MethodParameter.java", "MethodParameter", SymbolKind::Class, (1, 9)),
            // Not the annotation `@Resource`, nor the JDK `Predicate`.
            sym("spring-core/src/main/java/org/springframework/core/io/Resource.java", "Resource", SymbolKind::Interface, (1, 9)),
            sym("spring-core/src/main/java/org/springframework/cglib/core/Predicate.java", "Predicate", SymbolKind::Interface, (1, 9)),
        ];
        targets[2].signature = "public @interface Transactional".to_string();
        let content = "= Title\n\n== Transactions\n\nAnnotate with `@Transactional` and `Propagation.NEVER`, not `@Resource` or `Predicate<Throwable>`.\nSee {spring-framework-api}/core/MethodParameter.html#isOptional()[`isOptional`]\nand javadoc:org.springframework.core.MethodParameter[].\n\n[source,java]\n----\n`Transactional` in a listing\n----\n// `Transactional` in a comment\nNot \"`Transactional`\" quoted.\n";
        let sources = vec![
            doc("docs/tx.adoc", content),
            section("docs/tx.adoc", "Transactions", (3, 15)),
        ];
        assert_eq!(
            links(&sources, &targets),
            vec![
                ("Transactions".to_string(), "MethodParameter".to_string(), 7),
                (
                    "Transactions".to_string(),
                    "MethodParameter.isOptional".to_string(),
                    6
                ),
                ("Transactions".to_string(), "Propagation".to_string(), 5),
                ("Transactions".to_string(), "Transactional".to_string(), 5),
            ]
        );
    }

    #[test]
    fn doc_comment_links_resolve_from_the_documented_symbol() {
        let mut documented = sym(
            "src/main/java/org/acme/Service.java",
            "Service.run",
            SymbolKind::Method,
            (20, 30),
        );
        documented.metadata = Some(
            serde_json::to_string(&SymbolMetadata {
                doc_refs: vec![
                    myceliums_storage::DocRef {
                        target: "#stop".into(),
                        line: 15,
                    },
                    myceliums_storage::DocRef {
                        target: "Helper#assist(String,".into(),
                        line: 16,
                    },
                    myceliums_storage::DocRef {
                        target: "Widget".into(),
                        line: 17,
                    },
                    myceliums_storage::DocRef {
                        target: "java.util.List".into(),
                        line: 18,
                    },
                ],
                ..Default::default()
            })
            .unwrap(),
        );
        let targets = vec![
            sym(
                "src/main/java/org/acme/Service.java",
                "Service.stop",
                SymbolKind::Method,
                (32, 40),
            ),
            sym(
                "src/main/java/org/other/Service.java",
                "Service.stop",
                SymbolKind::Method,
                (32, 40),
            ),
            sym(
                "src/main/java/org/acme/Helper.java",
                "Helper.assist",
                SymbolKind::Method,
                (3, 9),
            ),
            sym(
                "src/main/java/org/acme/Widget.java",
                "Widget",
                SymbolKind::Class,
                (1, 9),
            ),
            sym(
                "src/main/java/org/other/Widget.java",
                "Widget",
                SymbolKind::Class,
                (1, 9),
            ),
        ];
        let sources = vec![documented];
        let found = links(&sources, &targets);
        let expected = vec![
            ("run".to_string(), "Helper.assist".to_string(), 16),
            ("run".to_string(), "Service.stop".to_string(), 15),
            ("run".to_string(), "Widget".to_string(), 17),
        ];
        assert_eq!(found, expected);
        // The same-package `Widget` and the same-file `stop`, not the others.
        let all: Vec<CodeSymbol> = [sources.clone(), targets.clone()].concat();
        let rels = extract(&sources, &all, "r").relationships;
        assert!(
            rels.iter().all(|r| !r.target_uid.contains("org/other")),
            "{rels:?}"
        );
    }
}
