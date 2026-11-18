//! Agent-facing graph navigation (#126).
//!
//! Pure functions over a repository's symbol and relationship tables that
//! answer the structural questions coding agents ask: which symbol a name
//! refers to (with candidates when it is ambiguous), who calls it, what it
//! calls, what depends on a symbol or file, and a token-budgeted context pack
//! for a free-text task. They hold no store handle and do no I/O except
//! [`index_freshness`], so the CLI (`myc callers`, `myc callees`,
//! `myc impact <target>`, `myc context`), the MCP server and the HTTP API can
//! share one implementation.
//!
//! Output sizes are bounded by a token budget. Tokens are estimated as one
//! token per four characters of the compact text rendering
//! ([`estimate_tokens`]); this is an approximation, not a tokenizer.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, RepoInfo, SymbolKind};
use serde::Serialize;

use crate::search::search_symbols;

/// Approximate token count of `text`: one token per four characters.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Whether `kind` is a code definition an agent can navigate to.
///
/// Excludes imports (a `use` line is not a definition), document sections,
/// rationale comments and the email/person nodes of the knowledge graph.
pub fn is_code_definition(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
            | SymbolKind::Enum
            | SymbolKind::Module
            | SymbolKind::Constant
            | SymbolKind::Variable
    )
}

/// Definitions a search hit may contribute to a context pack: code
/// definitions minus variables and constants, which the parsers also record
/// for locals (`const result = parse(x)` inside a test) and which would crowd
/// out the functions and types around them. A variable the task names
/// explicitly is still included.
fn is_searchable_definition(kind: &SymbolKind) -> bool {
    is_code_definition(kind) && !matches!(kind, SymbolKind::Variable | SymbolKind::Constant)
}

/// A signature on one line: whitespace runs (including line breaks in
/// multi-line declarations) collapse to one space, then it is cut to
/// `max_chars`.
fn one_line_signature(signature: &str, max_chars: usize) -> String {
    let joined = signature.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::text::truncate_ellipsis(&joined, max_chars)
}

// ── Symbol resolution ────────────────────────────────────────────────

/// Whether `kind` is something call and dependency questions are about:
/// functions, methods, types and modules.
///
/// Variables and constants are code definitions too, but parsers also record
/// locals (`const result = parse(x)`), so they only count when nothing
/// navigable carries the name. [`resolve_definition`] applies this rule; the
/// `myc` commands and the grep hook both go through it, so a hook hint and
/// the command it names agree on what a name refers to.
pub fn is_navigable(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
            | SymbolKind::Enum
            | SymbolKind::Module
    )
}

/// Whether `kind` can have callers: functions and methods.
pub fn is_callable(kind: &SymbolKind) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
}

/// A stored definition [`resolve_definition`] can work on: a full
/// [`CodeSymbol`] or the location-only
/// [`SymbolLocation`](myceliums_storage::SymbolLocation).
pub trait Definition {
    /// Short name.
    fn name(&self) -> &str;
    /// Fully qualified name.
    fn qualified_name(&self) -> &str;
    /// Symbol kind.
    fn kind(&self) -> &SymbolKind;
    /// Repository-relative file path.
    fn file_path(&self) -> &str;
    /// 1-based start line.
    fn start_line(&self) -> u32;
}

impl Definition for CodeSymbol {
    fn name(&self) -> &str {
        &self.name
    }
    fn qualified_name(&self) -> &str {
        &self.qualified_name
    }
    fn kind(&self) -> &SymbolKind {
        &self.kind
    }
    fn file_path(&self) -> &str {
        &self.file_path
    }
    fn start_line(&self) -> u32 {
        self.start_line
    }
}

impl Definition for myceliums_storage::SymbolLocation {
    fn name(&self) -> &str {
        &self.name
    }
    fn qualified_name(&self) -> &str {
        &self.qualified_name
    }
    fn kind(&self) -> &SymbolKind {
        &self.kind
    }
    fn file_path(&self) -> &str {
        &self.file_path
    }
    fn start_line(&self) -> u32 {
        self.start_line
    }
}

/// What a name resolved to.
#[derive(Debug)]
pub enum Resolution<'a, T> {
    /// Exactly one definition matches.
    Unique(&'a T),
    /// Several definitions match; sorted by file and line.
    Ambiguous(Vec<&'a T>),
    /// Nothing matches.
    NotFound,
}

/// [`Resolution`] over full symbols, as the `myc` commands use it.
pub type SymbolMatch<'a> = Resolution<'a, CodeSymbol>;

/// Normalises a qualified name for suffix matching: `::`, `#`, `/` and `\`
/// become `.`, and Go receiver decoration (`(*T).m`) is dropped.
fn normalize_qualified(name: &str) -> String {
    name.replace("::", ".")
        .replace(['#', '/', '\\'], ".")
        .replace(['(', ')', '*'], "")
}

/// Whether `qualified_name` ends with the qualified `query`
/// (`fzf.Item.AsString` ends with `Item.AsString`; separators `.`, `::`,
/// `#` and `/` are interchangeable).
pub fn qualified_suffix_matches(qualified_name: &str, query: &str) -> bool {
    let wanted = normalize_qualified(query);
    let qn = normalize_qualified(qualified_name);
    qn == wanted || qn.ends_with(&format!(".{wanted}"))
}

/// The short name a possibly qualified query refers to (`Item.AsString` →
/// `AsString`).
pub fn short_name(query: &str) -> String {
    let wanted = normalize_qualified(query.trim());
    wanted.rsplit('.').next().unwrap_or(&wanted).to_string()
}

/// Navigable hits when there are any, otherwise all hits.
fn prefer_navigable<T: Definition>(hits: Vec<&T>) -> Vec<&T> {
    if hits.iter().any(|h| is_navigable(h.kind())) {
        hits.into_iter()
            .filter(|h| is_navigable(h.kind()))
            .collect()
    } else {
        hits
    }
}

/// Resolves a user- or agent-supplied name to code definitions.
///
/// Tried in order, the first non-empty step wins:
/// 1. exact qualified name (`auth::login`);
/// 2. exact short name (`login`);
/// 3. qualified suffix (`Item.AsString` matches `fzf.Item.AsString`, and
///    `Item::as_string` matches `src/item.rs::Item::as_string`).
///
/// Navigable definitions ([`is_navigable`]) win over variables and
/// constants: steps 1 and 2 are first tried for navigable hits only, then
/// for any. More than one remaining hit is
/// [`Resolution::Ambiguous`]: show the candidates with file:line rather than
/// guess.
pub fn resolve_definition<'a, T: Definition>(items: &'a [T], query: &str) -> Resolution<'a, T> {
    let query = query.trim();
    if query.is_empty() {
        return Resolution::NotFound;
    }
    let defs = || items.iter().filter(|s| is_code_definition(s.kind()));

    let by_qualified: Vec<&T> = defs().filter(|s| s.qualified_name() == query).collect();
    let by_name: Vec<&T> = defs().filter(|s| s.name() == query).collect();
    // A navigable exact match wins over a variable whose qualified name
    // happens to equal the query; otherwise the steps apply in order.
    for step in [&by_qualified, &by_name] {
        if step.iter().any(|s| is_navigable(s.kind())) {
            return to_match(prefer_navigable(step.clone()));
        }
    }
    for step in [by_qualified, by_name] {
        if !step.is_empty() {
            return to_match(step);
        }
    }
    let short = short_name(query);
    if normalize_qualified(query).contains('.') && !short.is_empty() {
        let by_suffix: Vec<&T> = defs()
            .filter(|s| s.name() == short && qualified_suffix_matches(s.qualified_name(), query))
            .collect();
        if !by_suffix.is_empty() {
            return to_match(prefer_navigable(by_suffix));
        }
    }
    Resolution::NotFound
}

/// [`resolve_definition`] over full symbols.
pub fn resolve_symbol<'a>(symbols: &'a [CodeSymbol], query: &str) -> SymbolMatch<'a> {
    resolve_definition(symbols, query)
}

fn to_match<T: Definition>(mut hits: Vec<&T>) -> Resolution<'_, T> {
    if hits.len() == 1 {
        return Resolution::Unique(hits[0]);
    }
    hits.sort_by(|a, b| {
        (a.file_path(), a.start_line(), a.qualified_name()).cmp(&(
            b.file_path(),
            b.start_line(),
            b.qualified_name(),
        ))
    });
    Resolution::Ambiguous(hits)
}

/// Up to `limit` definition names close to `query`, for "did you mean".
///
/// Case-insensitive exact matches first, then names containing the query
/// (case-insensitive), shortest first. Deduplicated by name.
pub fn similar_names(symbols: &[CodeSymbol], query: &str, limit: usize) -> Vec<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let mut exact = BTreeSet::new();
    let mut containing = BTreeSet::new();
    for s in symbols.iter().filter(|s| is_code_definition(&s.kind)) {
        let lower = s.name.to_lowercase();
        if lower == q {
            exact.insert(s.name.clone());
        } else if lower.contains(&q) {
            containing.insert((s.name.len(), s.name.clone()));
        }
    }
    exact
        .into_iter()
        .chain(containing.into_iter().map(|(_, n)| n))
        .take(limit)
        .collect()
}

// ── References ───────────────────────────────────────────────────────

/// A pointer to a symbol: what an agent needs to open it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolRef {
    /// Short name.
    pub name: String,
    /// Fully qualified name.
    pub qualified_name: String,
    /// Symbol kind (`Function`, `Method`, ...).
    pub kind: String,
    /// Repository-relative file path.
    pub file: String,
    /// 1-based start line.
    pub line: u32,
}

impl SymbolRef {
    /// Builds a reference from a stored symbol.
    pub fn of(symbol: &CodeSymbol) -> Self {
        Self {
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file: symbol.file_path.clone(),
            line: symbol.start_line,
        }
    }

    /// `file:line`.
    pub fn location(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// A symbol reached by walking graph edges from a start symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphHit {
    /// The symbol reached.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// Number of edges from the start symbol (1 = direct).
    pub depth: u32,
    /// For depth > 1: the name of the symbol one step closer to the start,
    /// through which this one was reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// Which way to follow `Calls` edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDirection {
    /// Follow edges backwards: who calls the start symbol.
    Callers,
    /// Follow edges forwards: what the start symbol calls.
    Callees,
}

/// Breadth-first walk over `kinds` edges from `start_uids`, up to `max_depth`.
///
/// `backwards` follows edges from target to source (callers, dependents).
/// Each symbol is reported once, at its shortest distance. Start symbols are
/// not reported. Hits are ordered by depth, then file and line.
fn walk(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    start_uids: &[&str],
    kinds: &[RelationshipKind],
    backwards: bool,
    max_depth: u32,
) -> Vec<GraphHit> {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
    for rel in relationships.iter().filter(|r| kinds.contains(&r.kind)) {
        let (from, to) = if backwards {
            (rel.target_uid.as_str(), rel.source_uid.as_str())
        } else {
            (rel.source_uid.as_str(), rel.target_uid.as_str())
        };
        next.entry(from).or_default().push(to);
    }

    let mut seen: HashSet<&str> = start_uids.iter().copied().collect();
    let mut queue: VecDeque<(&str, u32)> = start_uids.iter().map(|u| (*u, 0)).collect();
    let mut hits = Vec::new();
    while let Some((uid, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let Some(neighbours) = next.get(uid) else {
            continue;
        };
        for &n in neighbours {
            if !seen.insert(n) {
                continue;
            }
            // Edges into nodes that are not symbols (files, unresolved
            // targets) are not navigable and are not walked through either.
            let Some(sym) = by_uid.get(n) else {
                continue;
            };
            let via = (depth > 0)
                .then(|| by_uid.get(uid).map(|s| s.name.clone()))
                .flatten();
            hits.push(GraphHit {
                symbol: SymbolRef::of(sym),
                depth: depth + 1,
                via,
            });
            queue.push_back((n, depth + 1));
        }
    }
    hits.sort_by(|a, b| {
        (a.depth, a.symbol.file.as_str(), a.symbol.line).cmp(&(
            b.depth,
            b.symbol.file.as_str(),
            b.symbol.line,
        ))
    });
    hits
}

/// Callers or callees of `start` over `Calls` edges, up to `max_depth` hops.
pub fn walk_calls(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    start: &CodeSymbol,
    direction: CallDirection,
    max_depth: u32,
) -> Vec<GraphHit> {
    walk(
        symbols,
        relationships,
        &[start.uid.as_str()],
        &[RelationshipKind::Calls],
        direction == CallDirection::Callers,
        max_depth,
    )
}

// ── Blast radius ─────────────────────────────────────────────────────

/// What depends on a symbol or a file: the inputs of a change-impact answer.
#[derive(Debug, Clone, Serialize)]
pub struct BlastRadius {
    /// The changed symbols the walk started from.
    pub roots: Vec<SymbolRef>,
    /// Symbols that call or import a root, transitively up to the depth.
    pub dependents: Vec<GraphHit>,
    /// Every file holding a root or a dependent, sorted.
    pub files: Vec<String>,
}

/// Dependents of `roots` over incoming `Calls` and `Imports` edges (the same
/// edge kinds [`crate::impact::detect_impact`] treats as dependencies).
pub fn blast_radius(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    roots: &[&CodeSymbol],
    max_depth: u32,
) -> BlastRadius {
    let root_uids: Vec<&str> = roots.iter().map(|s| s.uid.as_str()).collect();
    let dependents = walk(
        symbols,
        relationships,
        &root_uids,
        &[RelationshipKind::Calls, RelationshipKind::Imports],
        true,
        max_depth,
    );
    let files: BTreeSet<String> = roots
        .iter()
        .map(|s| s.file_path.clone())
        .chain(dependents.iter().map(|h| h.symbol.file.clone()))
        .collect();
    let mut roots: Vec<SymbolRef> = roots.iter().map(|s| SymbolRef::of(s)).collect();
    roots.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    BlastRadius {
        roots,
        dependents,
        files: files.into_iter().collect(),
    }
}

/// Code definitions in the indexed file `path`.
///
/// `path` may be repository-relative (`src/a.rs`, `./src/a.rs`) or end with
/// the indexed path; an exact match wins over a suffix match.
pub fn symbols_in_file<'a>(symbols: &'a [CodeSymbol], path: &str) -> Vec<&'a CodeSymbol> {
    let wanted = path.trim().trim_start_matches("./").replace('\\', "/");
    if wanted.is_empty() {
        return vec![];
    }
    let defs = || symbols.iter().filter(|s| is_code_definition(&s.kind));
    let exact: Vec<&CodeSymbol> = defs().filter(|s| s.file_path == wanted).collect();
    if !exact.is_empty() {
        return exact;
    }
    let suffix = format!("/{wanted}");
    let by_suffix: Vec<&CodeSymbol> = defs()
        .filter(|s| {
            s.file_path.ends_with(&suffix) || wanted.ends_with(&format!("/{}", s.file_path))
        })
        .collect();
    // A suffix that names files in two directories is not one file.
    let distinct: BTreeSet<&str> = by_suffix.iter().map(|s| s.file_path.as_str()).collect();
    if distinct.len() == 1 {
        by_suffix
    } else {
        vec![]
    }
}

// ── Budgeting ────────────────────────────────────────────────────────

/// Keeps the longest prefix of `items` whose rendered lines fit in
/// `budget_tokens`, always at least one item when `items` is non-empty.
/// Returns the kept items and how many were dropped.
pub fn fit_to_budget<T>(
    items: Vec<T>,
    budget_tokens: usize,
    render: impl Fn(&T) -> String,
) -> (Vec<T>, usize) {
    let total = items.len();
    let mut used = 0usize;
    let mut kept = Vec::new();
    for item in items {
        let cost = estimate_tokens(&render(&item)) + 1;
        if !kept.is_empty() && used + cost > budget_tokens {
            break;
        }
        used += cost;
        kept.push(item);
    }
    let dropped = total - kept.len();
    (kept, dropped)
}

// ── Context pack ─────────────────────────────────────────────────────

/// Limits for [`build_context_pack`].
#[derive(Debug, Clone, Copy)]
pub struct ContextOptions {
    /// Upper bound for the pack's estimated size in tokens.
    pub budget_tokens: usize,
    /// Most symbols to consider before the budget is applied.
    pub max_symbols: usize,
    /// Most callers and most callees listed per symbol.
    pub max_neighbours: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            budget_tokens: 2000,
            max_symbols: 12,
            max_neighbours: 5,
        }
    }
}

/// One symbol in a context pack, with its immediate call neighbourhood.
#[derive(Debug, Clone, Serialize)]
pub struct ContextEntry {
    /// The symbol.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// Last line of the symbol's definition.
    pub end_line: u32,
    /// Declaration, on one line and shortened to 160 characters.
    pub signature: String,
    /// Why the symbol is in the pack: `"named"` when the task text names it,
    /// `"search"` when lexical search ranked it.
    pub reason: &'static str,
    /// Direct callers, at most `max_neighbours`.
    pub callers: Vec<SymbolRef>,
    /// Number of direct callers in the index.
    pub callers_total: usize,
    /// Direct callees, at most `max_neighbours`.
    pub callees: Vec<SymbolRef>,
    /// Number of direct callees in the index.
    pub callees_total: usize,
}

/// A token-budgeted bundle of the symbols most relevant to a task: what a
/// harness injects into a prompt before the model call (#126, layer 5).
#[derive(Debug, Clone, Serialize)]
pub struct ContextPack {
    /// The task text the pack was built for.
    pub query: String,
    /// The budget the pack was built against.
    pub budget_tokens: usize,
    /// Estimated size of [`ContextPack::render_text`] in tokens.
    pub estimated_tokens: usize,
    /// Whether relevant symbols were left out to stay within the budget.
    pub truncated: bool,
    /// Selected symbols, most relevant first.
    pub symbols: Vec<ContextEntry>,
    /// Files holding the selected symbols, in order of first appearance.
    pub files: Vec<String>,
}

/// Identifier-shaped words in free text, in order of first appearance:
/// tokens with `_`, an inner capital, or a `.`/`::` qualifier.
fn named_identifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$'))) {
        let word = raw.trim_matches(|c: char| matches!(c, '.' | ':'));
        if word.len() < 3 {
            continue;
        }
        let short = word.rsplit(['.', ':']).next().unwrap_or(word);
        let identifier_shaped = word.contains('_')
            || word.contains('.')
            || word.contains("::")
            || short.chars().skip(1).any(|c| c.is_uppercase())
                && short.chars().any(|c| c.is_lowercase());
        if identifier_shaped && seen.insert(word.to_string()) {
            out.push(word.to_string());
        }
    }
    out
}

/// Direct call neighbours of `uid`, ordered by file and line.
fn neighbours<'a>(
    edges: &HashMap<&str, Vec<&'a str>>,
    by_uid: &HashMap<&str, &'a CodeSymbol>,
    uid: &str,
) -> Vec<&'a CodeSymbol> {
    let mut seen = HashSet::new();
    let mut out: Vec<&CodeSymbol> = edges
        .get(uid)
        .into_iter()
        .flatten()
        .filter(|u| seen.insert(**u))
        .filter_map(|u| by_uid.get(u).copied())
        .collect();
    out.sort_by(|a, b| {
        (a.file_path.as_str(), a.start_line).cmp(&(b.file_path.as_str(), b.start_line))
    });
    out
}

impl ContextEntry {
    fn render(&self) -> String {
        let mut out = format!(
            "{} ({}) {}\n",
            self.symbol.qualified_name,
            self.symbol.kind,
            self.symbol.location()
        );
        if !self.signature.is_empty() {
            out.push_str(&format!("  {}\n", self.signature));
        }
        for (label, refs, total) in [
            ("called by", &self.callers, self.callers_total),
            ("calls", &self.callees, self.callees_total),
        ] {
            if total == 0 {
                continue;
            }
            let shown: Vec<String> = refs
                .iter()
                .map(|r| format!("{} {}", r.name, r.location()))
                .collect();
            let more = total.saturating_sub(refs.len());
            let tail = if more > 0 {
                format!(", +{more} more")
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  {label} ({total}): {}{tail}\n",
                shown.join(", ")
            ));
        }
        out
    }
}

impl ContextPack {
    /// Compact text rendering, the form whose size the budget bounds.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        for entry in &self.symbols {
            out.push_str(&entry.render());
        }
        if self.symbols.is_empty() {
            out.push_str("No matching symbols in the index.\n");
        } else {
            out.push_str(&format!("files: {}\n", self.files.join(", ")));
        }
        if self.truncated {
            out.push_str("(more matches omitted to stay within the token budget)\n");
        }
        out
    }
}

/// Builds a context pack for the free-text `query`.
///
/// Symbols named in the query (identifier-shaped words that resolve to a
/// definition) come first, then lexical (BM25) search hits; each comes with
/// its signature and direct callers and callees. Entries are added in that
/// order while the text rendering fits `budget_tokens`; the first entry that
/// does not fit ends the pack and sets `truncated`. The most relevant entry is
/// kept even when it alone exceeds the budget. Lexical search needs no
/// embedding model, so a pack is deterministic and fast to build.
pub fn build_context_pack(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    query: &str,
    options: ContextOptions,
) -> ContextPack {
    let ranked: Vec<&str> = search_symbols(symbols, query)
        .iter()
        .filter(|hit| is_searchable_definition(&hit.symbol.kind))
        .take(options.max_symbols)
        .filter_map(|hit| {
            symbols
                .iter()
                .find(|s| s.uid == hit.symbol.uid)
                .map(|s| s.uid.as_str())
        })
        .collect();
    build_context_pack_ranked(symbols, relationships, query, &ranked, options)
}

/// [`build_context_pack`] with the search ranking supplied by the caller
/// (`ranked_uids`, best first), e.g. from hybrid BM25 + vector search.
/// Symbols named in the query still come first.
pub fn build_context_pack_ranked(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    query: &str,
    ranked_uids: &[&str],
    options: ContextOptions,
) -> ContextPack {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for rel in relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
    {
        callers_of
            .entry(rel.target_uid.as_str())
            .or_default()
            .push(rel.source_uid.as_str());
        callees_of
            .entry(rel.source_uid.as_str())
            .or_default()
            .push(rel.target_uid.as_str());
    }

    // Candidates: named symbols first, then search hits; unique by uid.
    let mut candidates: Vec<(&CodeSymbol, &'static str)> = Vec::new();
    let mut taken: HashSet<&str> = HashSet::new();
    for word in named_identifiers(query) {
        let hits: Vec<&CodeSymbol> = match resolve_symbol(symbols, &word) {
            SymbolMatch::Unique(s) => vec![s],
            SymbolMatch::Ambiguous(all) => all.into_iter().take(3).collect(),
            SymbolMatch::NotFound => vec![],
        };
        for s in hits {
            if taken.insert(s.uid.as_str()) {
                candidates.push((s, "named"));
            }
        }
    }
    for uid in ranked_uids {
        if candidates.len() >= options.max_symbols {
            break;
        }
        if let Some(s) = by_uid.get(uid) {
            if is_searchable_definition(&s.kind) && taken.insert(s.uid.as_str()) {
                candidates.push((s, "search"));
            }
        }
    }
    candidates.truncate(options.max_symbols.max(1));

    let mut entries = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for (sym, reason) in candidates {
        let callers = neighbours(&callers_of, &by_uid, &sym.uid);
        let callees = neighbours(&callees_of, &by_uid, &sym.uid);
        let entry = ContextEntry {
            symbol: SymbolRef::of(sym),
            end_line: sym.end_line,
            signature: one_line_signature(&sym.signature, 160),
            reason,
            callers_total: callers.len(),
            callers: callers
                .iter()
                .take(options.max_neighbours)
                .map(|s| SymbolRef::of(s))
                .collect(),
            callees_total: callees.len(),
            callees: callees
                .iter()
                .take(options.max_neighbours)
                .map(|s| SymbolRef::of(s))
                .collect(),
        };
        let cost = estimate_tokens(&entry.render());
        // The most relevant entry is always kept, even over budget: an empty
        // pack would read as "nothing matched".
        if !entries.is_empty() && used + cost > options.budget_tokens {
            truncated = true;
            break;
        }
        used += cost;
        if !files.contains(&entry.symbol.file) {
            files.push(entry.symbol.file.clone());
        }
        entries.push(entry);
    }

    let mut pack = ContextPack {
        query: query.to_string(),
        budget_tokens: options.budget_tokens,
        estimated_tokens: 0,
        truncated,
        symbols: entries,
        files,
    };
    pack.estimated_tokens = estimate_tokens(&pack.render_text());
    pack
}

// ── Freshness ────────────────────────────────────────────────────────

/// How the index relates to the repository's current commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FreshnessState {
    /// The index was built at the commit HEAD points to. Uncommitted edits
    /// are not checked.
    Current,
    /// HEAD has moved since the index was built.
    Behind,
    /// No commit was recorded or git is unavailable.
    Unknown,
}

/// Index freshness: the analyzed commit versus the repository's HEAD.
#[derive(Debug, Clone, Serialize)]
pub struct IndexFreshness {
    /// When the index was built (RFC 3339).
    pub analyzed_at: String,
    /// Commit the index was built at, if recorded.
    pub analyzed_commit: Option<String>,
    /// Commit HEAD points to now, if git could tell.
    pub head_commit: Option<String>,
    /// The comparison of the two.
    pub state: FreshnessState,
}

impl IndexFreshness {
    /// Compares a recorded commit with the current HEAD.
    pub fn compare(analyzed_at: &str, analyzed: Option<&str>, head: Option<&str>) -> Self {
        let state = match (analyzed, head) {
            (Some(a), Some(h)) if a == h => FreshnessState::Current,
            (Some(_), Some(_)) => FreshnessState::Behind,
            _ => FreshnessState::Unknown,
        };
        Self {
            analyzed_at: analyzed_at.to_string(),
            analyzed_commit: analyzed.map(str::to_string),
            head_commit: head.map(str::to_string),
            state,
        }
    }

    /// One line for text output, e.g. `index: abc1234 = HEAD`.
    pub fn summary(&self) -> String {
        let short = |c: &str| c.chars().take(7).collect::<String>();
        match (self.state, &self.analyzed_commit, &self.head_commit) {
            (FreshnessState::Current, Some(a), _) => format!("index: {} = HEAD", short(a)),
            (FreshnessState::Behind, Some(a), Some(h)) => format!(
                "index: {} but HEAD is {}; results may be stale (myc analyze . to refresh)",
                short(a),
                short(h)
            ),
            _ => format!("index: built {}", self.analyzed_at),
        }
    }
}

/// Freshness of a registered repository's index. Runs `git rev-parse HEAD`
/// in the repository (a few milliseconds); any git failure yields
/// [`FreshnessState::Unknown`] rather than an error.
pub fn index_freshness(info: &RepoInfo) -> IndexFreshness {
    let head = crate::cache::get_head_commit(Path::new(&info.path)).ok();
    IndexFreshness::compare(
        &info.analyzed_at,
        info.analyzed_commit.as_deref(),
        head.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(
        uid: &str,
        name: &str,
        qualified: &str,
        kind: SymbolKind,
        file: &str,
        line: u32,
    ) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: line,
            end_line: line + 5,
            signature: format!("fn {name}()"),
            content: format!("fn {name}() {{ }}"),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    fn calls(source: &str, target: &str) -> Relationship {
        rel(source, target, RelationshipKind::Calls)
    }

    fn rel(source: &str, target: &str, kind: RelationshipKind) -> Relationship {
        Relationship {
            uid: format!("{source}->{target}"),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "r".to_string(),
            metadata: String::new(),
        }
    }

    /// main -> run -> parse_query -> tokenize; handler -> parse_query;
    /// two unrelated `parse` definitions; an import named like a function.
    fn fixture() -> (Vec<CodeSymbol>, Vec<Relationship>) {
        use SymbolKind::*;
        let symbols = vec![
            sym("main", "main", "app::main", Function, "src/main.rs", 1),
            sym("run", "run", "app::run", Function, "src/main.rs", 10),
            sym(
                "handler",
                "handler",
                "http::handler",
                Function,
                "src/http.rs",
                3,
            ),
            sym(
                "pq",
                "parse_query",
                "query::parse_query",
                Function,
                "src/query.rs",
                42,
            ),
            sym(
                "tok",
                "tokenize",
                "query::tokenize",
                Function,
                "src/query.rs",
                80,
            ),
            sym("pa", "parse", "a::parse", Function, "src/a.rs", 5),
            sym("pb", "parse", "b::parse", Function, "src/b.rs", 7),
            sym(
                "imp",
                "parse_query",
                "use query::parse_query",
                Import,
                "src/http.rs",
                1,
            ),
            sym(
                "item",
                "AsString",
                "fzf.Item.AsString",
                Method,
                "src/item.go",
                20,
            ),
        ];
        let rels = vec![
            calls("main", "run"),
            calls("run", "pq"),
            calls("handler", "pq"),
            calls("pq", "tok"),
            calls("pq", "unresolved-target"),
            rel("handler", "tok", RelationshipKind::Imports),
        ];
        (symbols, rels)
    }

    #[test]
    fn resolve_prefers_qualified_then_short_then_suffix() {
        let (s, _) = fixture();
        assert!(matches!(resolve_symbol(&s, "a::parse"), SymbolMatch::Unique(x) if x.uid == "pa"));
        // The Import named parse_query is not a definition, so this is unique.
        assert!(
            matches!(resolve_symbol(&s, "parse_query"), SymbolMatch::Unique(x) if x.uid == "pq")
        );
        assert!(
            matches!(resolve_symbol(&s, "Item.AsString"), SymbolMatch::Unique(x) if x.uid == "item")
        );
        assert!(
            matches!(resolve_symbol(&s, "(*Item).AsString"), SymbolMatch::Unique(x) if x.uid == "item")
        );
        assert!(
            matches!(resolve_symbol(&s, "query.tokenize"), SymbolMatch::Unique(x) if x.uid == "tok")
        );
        assert!(matches!(
            resolve_symbol(&s, "Other.AsString"),
            SymbolMatch::NotFound
        ));
        assert!(matches!(resolve_symbol(&s, "nope"), SymbolMatch::NotFound));
        assert!(matches!(resolve_symbol(&s, "  "), SymbolMatch::NotFound));
    }

    #[test]
    fn ambiguous_names_list_every_candidate_by_location() {
        let (s, _) = fixture();
        let SymbolMatch::Ambiguous(c) = resolve_symbol(&s, "parse") else {
            panic!("parse must be ambiguous");
        };
        let locs: Vec<String> = c
            .iter()
            .map(|x| format!("{}:{}", x.file_path, x.start_line))
            .collect();
        assert_eq!(locs, ["src/a.rs:5", "src/b.rs:7"]);
    }

    #[test]
    fn navigable_definitions_win_over_variables_of_the_same_name() {
        let (mut s, _) = fixture();
        s.push(sym(
            "v",
            "tokenize",
            "t::tokenize",
            SymbolKind::Variable,
            "src/t.rs",
            3,
        ));
        assert!(matches!(resolve_symbol(&s, "tokenize"), SymbolMatch::Unique(x) if x.uid == "tok"));
        // Only variables carry the name: they still resolve.
        s.push(sym(
            "w",
            "only_var",
            "t::only_var",
            SymbolKind::Constant,
            "src/t.rs",
            4,
        ));
        assert!(matches!(resolve_symbol(&s, "only_var"), SymbolMatch::Unique(x) if x.uid == "w"));
    }

    #[test]
    fn locations_resolve_like_full_symbols() {
        use myceliums_storage::SymbolLocation;
        let (s, _) = fixture();
        let locs: Vec<SymbolLocation> = s
            .iter()
            .map(|x| SymbolLocation {
                uid: x.uid.clone(),
                name: x.name.clone(),
                qualified_name: x.qualified_name.clone(),
                kind: x.kind.clone(),
                file_path: x.file_path.clone(),
                start_line: x.start_line,
            })
            .collect();
        for q in ["parse", "parse_query", "Item.AsString", "a::parse", "nope"] {
            let full = match resolve_symbol(&s, q) {
                Resolution::Unique(x) => vec![x.uid.clone()],
                Resolution::Ambiguous(xs) => xs.iter().map(|x| x.uid.clone()).collect(),
                Resolution::NotFound => vec![],
            };
            let lite = match resolve_definition(&locs, q) {
                Resolution::Unique(x) => vec![x.uid.clone()],
                Resolution::Ambiguous(xs) => xs.iter().map(|x| x.uid.clone()).collect(),
                Resolution::NotFound => vec![],
            };
            assert_eq!(full, lite, "{q}");
        }
        assert_eq!(short_name("(*Item).AsString"), "AsString");
        assert!(qualified_suffix_matches(
            "fzf.Item.AsString",
            "Item::AsString"
        ));
        assert!(!qualified_suffix_matches(
            "fzf.Other.AsString",
            "Item.AsString"
        ));
    }

    #[test]
    fn similar_names_suggests_case_insensitive_and_containing() {
        let (s, _) = fixture();
        assert_eq!(similar_names(&s, "PARSE", 5), ["parse", "parse_query"]);
        assert!(similar_names(&s, "zzz", 5).is_empty());
    }

    #[test]
    fn callers_walk_reports_depth_and_via() {
        let (s, r) = fixture();
        let pq = s.iter().find(|x| x.uid == "pq").unwrap();
        let direct = walk_calls(&s, &r, pq, CallDirection::Callers, 1);
        let names: Vec<&str> = direct.iter().map(|h| h.symbol.name.as_str()).collect();
        assert_eq!(names, ["handler", "run"], "ordered by file then line");
        assert!(direct.iter().all(|h| h.depth == 1 && h.via.is_none()));

        let deep = walk_calls(&s, &r, pq, CallDirection::Callers, 3);
        let main = deep.iter().find(|h| h.symbol.name == "main").unwrap();
        assert_eq!(main.depth, 2);
        assert_eq!(main.via.as_deref(), Some("run"));
    }

    #[test]
    fn callees_walk_skips_unresolved_targets_and_terminates_on_cycles() {
        let (s, mut r) = fixture();
        r.push(calls("tok", "pq")); // cycle
        let pq = s.iter().find(|x| x.uid == "pq").unwrap();
        let hits = walk_calls(&s, &r, pq, CallDirection::Callees, 10);
        let names: Vec<&str> = hits.iter().map(|h| h.symbol.name.as_str()).collect();
        assert_eq!(names, ["tokenize"]);
    }

    #[test]
    fn blast_radius_follows_calls_and_imports_backwards() {
        let (s, r) = fixture();
        let tok = s.iter().find(|x| x.uid == "tok").unwrap();
        let br = blast_radius(&s, &r, &[tok], 2);
        let mut names: Vec<&str> = br
            .dependents
            .iter()
            .map(|h| h.symbol.name.as_str())
            .collect();
        names.sort();
        assert_eq!(names, ["handler", "parse_query", "run"]);
        assert_eq!(br.files, ["src/http.rs", "src/main.rs", "src/query.rs"]);
    }

    #[test]
    fn symbols_in_file_matches_exact_then_unique_suffix() {
        let (s, _) = fixture();
        assert_eq!(symbols_in_file(&s, "./src/query.rs").len(), 2);
        assert_eq!(symbols_in_file(&s, "query.rs").len(), 2);
        assert_eq!(symbols_in_file(&s, "/abs/repo/src/query.rs").len(), 2);
        // `.rs` suffix names several files: not one file.
        assert!(symbols_in_file(&s, "rs").is_empty());
        assert!(symbols_in_file(&s, "src/missing.rs").is_empty());
    }

    #[test]
    fn fit_to_budget_keeps_a_prefix_and_counts_the_rest() {
        // 39 characters each: 10 estimated tokens, plus 1 for the line break.
        let items: Vec<String> = (0..10).map(|i| format!("{i:0>39}")).collect();
        let (kept, dropped) = fit_to_budget(items.clone(), 35, |s| s.clone());
        assert_eq!((kept.len(), dropped), (3, 7));
        let (kept, dropped) = fit_to_budget(items, 1, |s| s.clone());
        assert_eq!((kept.len(), dropped), (1, 9), "always at least one item");
        let (kept, dropped) = fit_to_budget(Vec::<String>::new(), 10, |s| s.clone());
        assert_eq!((kept.len(), dropped), (0, 0));
    }

    #[test]
    fn named_identifiers_picks_identifier_shaped_words() {
        assert_eq!(
            named_identifiers(
                "Who calls parse_query and Item.AsString, or getUserName? not plain words"
            ),
            ["parse_query", "Item.AsString", "getUserName"]
        );
    }

    #[test]
    fn context_pack_puts_named_symbols_first_and_respects_the_budget() {
        let (s, r) = fixture();
        let pack = build_context_pack(
            &s,
            &r,
            "how does parse_query tokenize its input",
            ContextOptions::default(),
        );
        assert_eq!(pack.symbols[0].symbol.name, "parse_query");
        assert_eq!(pack.symbols[0].reason, "named");
        assert_eq!(pack.symbols[0].callers_total, 2);
        assert_eq!(pack.symbols[0].callees_total, 1);
        assert!(pack
            .symbols
            .iter()
            .any(|e| e.symbol.name == "tokenize" && e.reason == "search"));
        assert!(!pack.truncated);
        assert!(pack.estimated_tokens <= pack.budget_tokens);
        assert_eq!(pack.files[0], "src/query.rs");

        let tiny = build_context_pack(
            &s,
            &r,
            "how does parse_query tokenize its input",
            ContextOptions {
                budget_tokens: 20,
                ..ContextOptions::default()
            },
        );
        assert!(tiny.truncated);
        assert_eq!(
            tiny.symbols.len(),
            1,
            "the best entry is kept even over budget"
        );
        assert_eq!(tiny.symbols[0].symbol.name, "parse_query");
        let text = tiny.render_text();
        assert!(text.contains("token budget"), "{text}");
    }

    #[test]
    fn context_pack_skips_searched_locals_but_keeps_named_ones() {
        let (mut s, r) = fixture();
        let mut local = sym(
            "loc",
            "query_result",
            "t::query_result",
            SymbolKind::Constant,
            "src/t.rs",
            3,
        );
        local.signature = "const query_result =\n    parse_query(x)".into();
        s.push(local);
        let pack = build_context_pack(&s, &r, "parse query", ContextOptions::default());
        assert!(pack.symbols.iter().all(|e| e.symbol.name != "query_result"));
        let named = build_context_pack(
            &s,
            &r,
            "where is query_result set",
            ContextOptions::default(),
        );
        let entry = named
            .symbols
            .iter()
            .find(|e| e.symbol.name == "query_result")
            .unwrap();
        assert_eq!(entry.signature, "const query_result = parse_query(x)");
    }

    #[test]
    fn context_pack_for_unmatched_query_is_empty_not_an_error() {
        let (s, r) = fixture();
        let pack = build_context_pack(&s, &r, "zzzz qqqq", ContextOptions::default());
        assert!(pack.symbols.is_empty());
        assert!(pack.render_text().contains("No matching symbols"));
    }

    #[test]
    fn context_pack_json_shape_is_stable() {
        let (s, r) = fixture();
        let pack = build_context_pack(&s, &r, "parse_query", ContextOptions::default());
        let v = serde_json::to_value(&pack).unwrap();
        for key in [
            "query",
            "budget_tokens",
            "estimated_tokens",
            "truncated",
            "symbols",
            "files",
        ] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        let first = &v["symbols"][0];
        for key in [
            "name",
            "qualified_name",
            "kind",
            "file",
            "line",
            "end_line",
            "signature",
            "reason",
            "callers",
            "callers_total",
            "callees",
            "callees_total",
        ] {
            assert!(
                first.get(key).is_some(),
                "missing symbols[0].{key}: {first}"
            );
        }
    }

    #[test]
    fn freshness_compares_commits() {
        let cur = IndexFreshness::compare("t", Some("abcdef0123"), Some("abcdef0123"));
        assert_eq!(cur.state, FreshnessState::Current);
        assert_eq!(cur.summary(), "index: abcdef0 = HEAD");
        let behind = IndexFreshness::compare("t", Some("abcdef0123"), Some("1234567890"));
        assert_eq!(behind.state, FreshnessState::Behind);
        assert!(behind.summary().contains("HEAD is 1234567"));
        let unknown = IndexFreshness::compare("2026-11-18T00:00:00Z", None, Some("1234567890"));
        assert_eq!(unknown.state, FreshnessState::Unknown);
        assert_eq!(unknown.summary(), "index: built 2026-11-18T00:00:00Z");
        assert_eq!(serde_json::to_value(cur.state).unwrap(), "current");
    }
}
