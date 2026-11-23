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

use crate::dispatch::{Dispatch, DispatchCut, DispatchIndex, Override, MAX_LISTED_IMPLEMENTATIONS};
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
    /// Set when the hit was reached through dynamic dispatch (#190):
    /// `dispatch via Base.m, confidence 0.8`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<String>,
}

/// Which way to follow `Calls` edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDirection {
    /// Follow edges backwards: who calls the start symbol.
    Callers,
    /// Follow edges forwards: what the start symbol calls.
    Callees,
}

/// A symbol reached by a walk, by reference: what [`GraphHit`] reports, plus
/// the symbol one step closer to the start and the edge that reached it. The
/// MCP server needs the edge to say how a CALLS edge was resolved (#96).
#[derive(Debug, Clone, Copy)]
pub struct Reached<'a> {
    /// The symbol reached.
    pub symbol: &'a CodeSymbol,
    /// Number of edges from the nearest start symbol (1 = direct).
    pub depth: u32,
    /// For depth > 1: the symbol one step closer to the start.
    pub via: Option<&'a CodeSymbol>,
    /// The edge through which `symbol` was reached. For a hit reached by
    /// dispatch, the CALLS edge to the overridden method.
    pub edge: &'a Relationship,
    /// Set when the hit was reached through dynamic dispatch (#190): a
    /// caller of a method the symbol overrides, or an implementation of a
    /// method the symbol calls.
    pub dispatch: Option<Dispatch<'a>>,
}

impl Reached<'_> {
    /// The owned [`GraphHit`] for this hit.
    pub fn to_hit(&self) -> GraphHit {
        GraphHit {
            symbol: SymbolRef::of(self.symbol),
            depth: self.depth,
            via: self.via.map(|v| v.name.clone()),
            dispatch: self.dispatch.as_ref().map(Dispatch::describe),
        }
    }
}

/// Where a breadth-first walk stopped at its depth limit (#169): the
/// symbols at the last depth whose edges lead on to symbols the walk did not
/// reach. Empty when the limit cut nothing.
#[derive(Debug, Clone, Default)]
pub struct DepthCut<'a> {
    /// Symbols at the depth limit with edges the walk did not follow,
    /// ordered by file and line.
    pub frontier: Vec<&'a CodeSymbol>,
    /// Distinct symbols one more hop would add.
    pub beyond: usize,
}

impl DepthCut<'_> {
    /// Whether the depth limit left anything out.
    pub fn is_cut(&self) -> bool {
        self.beyond > 0
    }
}

/// What a bounded walk reached, and where its depth limit cut it.
#[derive(Debug, Clone)]
pub struct Walk<'a> {
    /// The symbols reached, ordered by depth, then file and line.
    pub hits: Vec<Reached<'a>>,
    /// What lies past the depth limit.
    pub cut: DepthCut<'a>,
    /// Calls whose implementations the walk listed only in part
    /// ([`MAX_LISTED_IMPLEMENTATIONS`] each), ordered by file and line.
    pub dispatch_cut: Vec<DispatchCut<'a>>,
}

/// The [`DepthCut`] of a walk whose symbols at `max_depth` are `last`, over
/// the neighbours `next` gives, given every uid it saw.
fn depth_cut<'a, 'n>(
    last: impl Iterator<Item = &'a CodeSymbol>,
    next: impl Fn(&str) -> Vec<&'n str>,
    by_uid: &HashMap<&str, &'a CodeSymbol>,
    seen: &HashSet<&str>,
) -> DepthCut<'a> {
    let mut beyond: HashSet<&str> = HashSet::new();
    let mut frontier = Vec::new();
    for symbol in last {
        let mut cut = false;
        for n in next(symbol.uid.as_str()) {
            if !seen.contains(n) && by_uid.contains_key(n) {
                beyond.insert(n);
                cut = true;
            }
        }
        if cut {
            frontier.push(symbol);
        }
    }
    frontier.sort_by(|a, b| {
        (a.file_path.as_str(), a.start_line).cmp(&(b.file_path.as_str(), b.start_line))
    });
    frontier.dedup_by(|a, b| a.uid == b.uid);
    DepthCut {
        frontier,
        beyond: beyond.len(),
    }
}

/// A neighbour of a symbol in a walk: its uid, the edge that leads there,
/// and how dispatch got there (if it did).
type Step<'a> = (&'a str, &'a Relationship, Option<(&'a str, Override<'a>)>);

/// The neighbours of `uid` over `next` edges and, when `dispatch` is given
/// (CALLS walks), through dynamic dispatch (#190):
///
/// - backwards (callers): the callers of every method `uid` overrides;
/// - forwards (callees): the implementations of every method `uid` calls,
///   at most [`MAX_LISTED_IMPLEMENTATIONS`] per call, nearest file first;
///   a call with more is pushed to `cut`.
///
/// Direct neighbours come first. Dispatch starts only from CALLS edges that
/// are not `ambiguous` and are not `super` calls ([`DispatchIndex::dispatches`]).
fn neighbours_of<'a>(
    uid: &str,
    next: &HashMap<&str, Vec<(&'a str, &'a Relationship)>>,
    dispatch: Option<&DispatchIndex<'a>>,
    backwards: bool,
    by_uid: &HashMap<&str, &'a CodeSymbol>,
    mut cut: Option<&mut Vec<(&'a str, usize)>>,
) -> Vec<Step<'a>> {
    let direct = next.get(uid).map(Vec::as_slice).unwrap_or(&[]);
    let mut out: Vec<Step<'a>> = direct.iter().map(|&(n, e)| (n, e, None)).collect();
    let Some(index) = dispatch else {
        return out;
    };
    if backwards {
        for ov in index.overridden(uid) {
            for &(caller, edge) in next.get(ov.other).map(Vec::as_slice).unwrap_or(&[]) {
                if index.dispatches(edge) {
                    out.push((caller, edge, Some((ov.other, *ov))));
                }
            }
        }
    } else {
        for &(target, edge) in direct {
            if !index.dispatches(edge) {
                continue;
            }
            let mut impls: Vec<Override<'a>> = index.implementations(target).to_vec();
            if impls.is_empty() {
                continue;
            }
            impls.sort_by_key(|ov| {
                by_uid
                    .get(ov.other)
                    .map(|s| (s.file_path.as_str(), s.start_line))
            });
            if impls.len() > MAX_LISTED_IMPLEMENTATIONS {
                if let Some(cut) = cut.as_deref_mut() {
                    cut.push((target, impls.len()));
                }
                impls.truncate(MAX_LISTED_IMPLEMENTATIONS);
            }
            out.extend(
                impls
                    .into_iter()
                    .map(|ov| (ov.other, edge, Some((target, ov)))),
            );
        }
    }
    out
}

/// Breadth-first walk over `kinds` edges from `starts`, up to `max_depth`.
///
/// `backwards` follows edges from target to source (callers, dependents).
/// A walk over CALLS edges also follows dynamic dispatch (#190, see
/// [`neighbours_of`]); a hit reached that way says so in
/// [`Reached::dispatch`]. Each symbol is reported once, at its shortest
/// distance (a direct edge wins over dispatch at the same distance). Start
/// symbols are not reported. Hits are ordered by depth, then file and line.
fn walk<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    starts: &[&'a CodeSymbol],
    kinds: &[RelationshipKind],
    backwards: bool,
    max_depth: u32,
) -> Walk<'a> {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut next: HashMap<&str, Vec<(&str, &Relationship)>> = HashMap::new();
    for rel in relationships.iter().filter(|r| kinds.contains(&r.kind)) {
        let (from, to) = if backwards {
            (rel.target_uid.as_str(), rel.source_uid.as_str())
        } else {
            (rel.source_uid.as_str(), rel.target_uid.as_str())
        };
        next.entry(from).or_default().push((to, rel));
    }
    let index = kinds
        .contains(&RelationshipKind::Calls)
        .then(|| DispatchIndex::new(relationships))
        .filter(|index| !index.is_empty());

    let mut seen: HashSet<&str> = starts.iter().map(|s| s.uid.as_str()).collect();
    let mut queue: VecDeque<(&str, u32)> = starts.iter().map(|s| (s.uid.as_str(), 0)).collect();
    let mut hits = Vec::new();
    let mut cut_calls: Vec<(&str, usize)> = Vec::new();
    while let Some((uid, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let steps = neighbours_of(
            uid,
            &next,
            index.as_ref(),
            backwards,
            &by_uid,
            Some(&mut cut_calls),
        );
        for (n, edge, dispatched) in steps {
            if !seen.insert(n) {
                continue;
            }
            // Edges into nodes that are not symbols (files, unresolved
            // targets) are not navigable and are not walked through either.
            let Some(sym) = by_uid.get(n) else {
                continue;
            };
            let via = if depth > 0 {
                by_uid.get(uid).copied()
            } else {
                None
            };
            let dispatch = dispatched.and_then(|(base, ov)| {
                Some(Dispatch {
                    base: by_uid.get(base)?,
                    edge: ov.edge,
                    confidence: ov.confidence.confidence(),
                })
            });
            hits.push(Reached {
                symbol: sym,
                depth: depth + 1,
                via,
                edge,
                dispatch,
            });
            queue.push_back((n, depth + 1));
        }
    }
    hits.sort_by(|a, b| {
        (a.depth, a.symbol.file_path.as_str(), a.symbol.start_line).cmp(&(
            b.depth,
            b.symbol.file_path.as_str(),
            b.symbol.start_line,
        ))
    });
    let last = hits
        .iter()
        .filter(|h| h.depth == max_depth)
        .map(|h| h.symbol);
    let cut = depth_cut(
        last,
        |uid| {
            neighbours_of(uid, &next, index.as_ref(), backwards, &by_uid, None)
                .into_iter()
                .map(|(n, _, _)| n)
                .collect()
        },
        &by_uid,
        &seen,
    );
    let mut dispatch_cut: Vec<DispatchCut> = Vec::new();
    for (base, total) in cut_calls {
        let Some(base) = by_uid.get(base).copied() else {
            continue;
        };
        if !dispatch_cut.iter().any(|c| c.base.uid == base.uid) {
            dispatch_cut.push(DispatchCut {
                base,
                total,
                listed: MAX_LISTED_IMPLEMENTATIONS,
            });
        }
    }
    dispatch_cut.sort_by(|a, b| {
        (a.base.file_path.as_str(), a.base.start_line)
            .cmp(&(b.base.file_path.as_str(), b.base.start_line))
    });
    Walk {
        hits,
        cut,
        dispatch_cut,
    }
}

/// Callers or callees of `start` over `Calls` edges, up to `max_depth` hops.
pub fn walk_calls(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    start: &CodeSymbol,
    direction: CallDirection,
    max_depth: u32,
) -> Vec<GraphHit> {
    walk_calls_reached(symbols, relationships, start, direction, max_depth)
        .iter()
        .map(Reached::to_hit)
        .collect()
}

/// [`walk_calls`], by reference: each hit with its `via` symbol and edge.
pub fn walk_calls_reached<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    start: &'a CodeSymbol,
    direction: CallDirection,
    max_depth: u32,
) -> Vec<Reached<'a>> {
    walk_calls_bounded(symbols, relationships, start, direction, max_depth).hits
}

/// [`walk_calls_reached`], with what the depth limit cut (#169).
pub fn walk_calls_bounded<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    start: &'a CodeSymbol,
    direction: CallDirection,
    max_depth: u32,
) -> Walk<'a> {
    walk(
        symbols,
        relationships,
        &[start],
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
    let dependents: Vec<GraphHit> = dependents_reached(symbols, relationships, roots, max_depth)
        .iter()
        .map(Reached::to_hit)
        .collect();
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

/// The dependents of [`blast_radius`], by reference: symbols that call or
/// import any of `roots`, transitively, each with its `via` symbol and edge.
pub fn dependents_reached<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    roots: &[&'a CodeSymbol],
    max_depth: u32,
) -> Vec<Reached<'a>> {
    dependents_bounded(symbols, relationships, roots, max_depth).hits
}

/// [`dependents_reached`], with what the depth limit cut (#169).
pub fn dependents_bounded<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    roots: &[&'a CodeSymbol],
    max_depth: u32,
) -> Walk<'a> {
    walk(
        symbols,
        relationships,
        roots,
        &[RelationshipKind::Calls, RelationshipKind::Imports],
        true,
        max_depth,
    )
}

// ── Call chains ──────────────────────────────────────────────────────

/// A symbol between the two ends of [`CallChains`].
#[derive(Debug, Clone, Copy)]
pub struct ChainStep<'a> {
    /// The symbol.
    pub symbol: &'a CodeSymbol,
    /// Calls from the start to this symbol, on the shortest way.
    pub from_start: u32,
    /// Calls in the shortest chain that passes through this symbol.
    pub chain_len: u32,
}

/// How `from` reaches `to` over `Calls` edges.
#[derive(Debug, Clone)]
pub struct CallChains<'a> {
    /// One shortest chain, `from` first and `to` last; every symbol after the
    /// first comes with the CALLS edge that reached it.
    pub shortest: Vec<(&'a CodeSymbol, Option<&'a Relationship>)>,
    /// For each symbol of [`shortest`](Self::shortest), in the same order:
    /// set when the call reaches it through dynamic dispatch (#190), the
    /// CALLS edge then binding the method it overrides.
    pub dispatched: Vec<Option<Dispatch<'a>>>,
    /// Every symbol strictly between the ends on a chain of at most
    /// `shortest + extra` calls, ordered by chain length, then distance from
    /// the start, then file and line.
    pub on_chains: Vec<ChainStep<'a>>,
    /// Symbols on longer chains, still within `max_depth`, that
    /// [`on_chains`](Self::on_chains) leaves out (#169); same order.
    pub longer: Vec<ChainStep<'a>>,
}

/// Why `from` reaches `to` over no `Calls` chain within `max_depth`: what
/// the walk from `from` did reach, and where the depth limit cut it (#169).
#[derive(Debug, Clone)]
pub struct NoChain<'a> {
    /// Every symbol reachable from `from` within `max_depth` calls, with its
    /// distance, nearest first, then by file and line.
    pub reached: Vec<(&'a CodeSymbol, u32)>,
    /// Symbols at call `max_depth` that call further.
    pub cut: DepthCut<'a>,
}

impl CallChains<'_> {
    /// Calls in the shortest chain.
    pub fn len(&self) -> u32 {
        self.shortest.len().saturating_sub(1) as u32
    }

    /// Whether the chain is empty (never, for a found chain).
    pub fn is_empty(&self) -> bool {
        self.shortest.is_empty()
    }
}

/// Per symbol uid: calls from the start, and the (previous uid, edge) that
/// reached it on a shortest way (`None` for the start).
type Distances<'a> = HashMap<&'a str, (u32, Option<(&'a str, &'a Relationship)>)>;

/// A forward step of a call chain: the callee, the CALLS edge, and the
/// dispatch that reached it, if any.
type ChainEdge<'a> = (&'a str, &'a Relationship, Option<Dispatch<'a>>);

/// Call chains from `from` to `to`, following `Calls` edges forwards, or
/// what the search reached when `to` is not reachable within `max_depth`
/// calls. `extra` widens [`CallChains::on_chains`] to chains up to that many
/// calls longer than the shortest (still at most `max_depth`).
pub fn call_chains<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    from: &'a CodeSymbol,
    to: &'a CodeSymbol,
    max_depth: u32,
    extra: u32,
) -> Result<CallChains<'a>, NoChain<'a>> {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    // A call to an overridden method also reaches its implementations
    // (#190): every one of them, so a chain to any implementation is found.
    let index = DispatchIndex::new(relationships);
    let mut forward: HashMap<&str, Vec<ChainEdge>> = HashMap::new();
    let mut backward: HashMap<&str, Vec<&str>> = HashMap::new();
    for rel in relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
    {
        let (s, t) = (rel.source_uid.as_str(), rel.target_uid.as_str());
        let (Some(_), Some(&base)) = (by_uid.get(s), by_uid.get(t)) else {
            continue;
        };
        forward.entry(s).or_default().push((t, rel, None));
        backward.entry(t).or_default().push(s);
        if !index.dispatches(rel) {
            continue;
        }
        for ov in index.implementations(t) {
            if by_uid.contains_key(ov.other) {
                let dispatch = Dispatch {
                    base,
                    edge: ov.edge,
                    confidence: ov.confidence.confidence(),
                };
                forward
                    .entry(s)
                    .or_default()
                    .push((ov.other, rel, Some(dispatch)));
                backward.entry(ov.other).or_default().push(s);
            }
        }
    }
    // Direct calls first: at one distance, a chain prefers them.
    for edges in forward.values_mut() {
        edges.sort_by_key(|(_, _, dispatch)| dispatch.is_some());
    }
    let mut dispatch_of: HashMap<(&str, &str), Dispatch> = HashMap::new();

    let mut from_start: Distances = HashMap::new();
    from_start.insert(from.uid.as_str(), (0, None));
    let mut queue = VecDeque::from([(from.uid.as_str(), 0u32)]);
    while let Some((uid, d)) = queue.pop_front() {
        if d >= max_depth {
            continue;
        }
        for &(n, rel, dispatch) in forward.get(uid).map(Vec::as_slice).unwrap_or(&[]) {
            if let std::collections::hash_map::Entry::Vacant(e) = from_start.entry(n) {
                e.insert((d + 1, Some((uid, rel))));
                if let Some(dispatch) = dispatch {
                    dispatch_of.insert((uid, n), dispatch);
                }
                queue.push_back((n, d + 1));
            }
        }
    }
    let Some(&(shortest_len, _)) = from_start.get(to.uid.as_str()) else {
        let mut reached: Vec<(&CodeSymbol, u32)> = from_start
            .iter()
            .filter(|(uid, _)| **uid != from.uid)
            .map(|(uid, (d, _))| (by_uid[uid], *d))
            .collect();
        reached.sort_by(|a, b| {
            (
                a.1,
                a.0.file_path.as_str(),
                a.0.start_line,
                &a.0.qualified_name,
            )
                .cmp(&(
                    b.1,
                    b.0.file_path.as_str(),
                    b.0.start_line,
                    &b.0.qualified_name,
                ))
        });
        let seen: HashSet<&str> = from_start.keys().copied().collect();
        let last = reached
            .iter()
            .filter(|(_, d)| *d == max_depth)
            .map(|(s, _)| *s);
        let cut = depth_cut(
            last,
            |uid| {
                forward
                    .get(uid)
                    .map(|edges| edges.iter().map(|&(n, _, _)| n).collect())
                    .unwrap_or_default()
            },
            &by_uid,
            &seen,
        );
        return Err(NoChain { reached, cut });
    };

    let mut to_end: HashMap<&str, u32> = HashMap::from([(to.uid.as_str(), 0)]);
    let mut queue = VecDeque::from([(to.uid.as_str(), 0u32)]);
    while let Some((uid, d)) = queue.pop_front() {
        if d >= max_depth {
            continue;
        }
        for &n in backward.get(uid).map(Vec::as_slice).unwrap_or(&[]) {
            if let std::collections::hash_map::Entry::Vacant(e) = to_end.entry(n) {
                e.insert(d + 1);
                queue.push_back((n, d + 1));
            }
        }
    }

    let mut shortest = Vec::new();
    let mut dispatched = Vec::new();
    let mut cursor = to.uid.as_str();
    while let Some((_, step)) = from_start.get(cursor) {
        shortest.push((by_uid[cursor], step.map(|(_, rel)| rel)));
        dispatched.push(step.and_then(|(prev, _)| dispatch_of.get(&(prev, cursor)).copied()));
        match step {
            Some((prev, _)) => cursor = prev,
            None => break,
        }
    }
    shortest.reverse();
    dispatched.reverse();

    let limit = shortest_len.saturating_add(extra).min(max_depth);
    let (mut on_chains, mut longer): (Vec<ChainStep>, Vec<ChainStep>) = from_start
        .iter()
        .filter(|(uid, _)| **uid != from.uid && **uid != to.uid)
        .filter_map(|(uid, (d, _))| {
            let chain_len = d + to_end.get(uid)?;
            (chain_len <= max_depth).then(|| ChainStep {
                symbol: by_uid[uid],
                from_start: *d,
                chain_len,
            })
        })
        .partition(|step| step.chain_len <= limit);
    let order = |a: &ChainStep, b: &ChainStep| {
        (
            a.chain_len,
            a.from_start,
            a.symbol.file_path.as_str(),
            a.symbol.start_line,
            a.symbol.qualified_name.as_str(),
        )
            .cmp(&(
                b.chain_len,
                b.from_start,
                b.symbol.file_path.as_str(),
                b.symbol.start_line,
                b.symbol.qualified_name.as_str(),
            ))
    };
    longer.sort_by(order);
    on_chains.sort_by(|a, b| {
        (
            a.chain_len,
            a.from_start,
            a.symbol.file_path.as_str(),
            a.symbol.start_line,
            a.symbol.qualified_name.as_str(),
        )
            .cmp(&(
                b.chain_len,
                b.from_start,
                b.symbol.file_path.as_str(),
                b.symbol.start_line,
                b.symbol.qualified_name.as_str(),
            ))
    });
    Ok(CallChains {
        shortest,
        dispatched,
        on_chains,
        longer,
    })
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

/// A git call for the freshness line that takes longer than this is
/// abandoned: the line is a courtesy and must not stall an answer.
const FRESHNESS_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Freshness of a registered repository's index. Runs `git rev-parse HEAD`
/// in the repository through [`crate::git_read`] (no optional locks, killed
/// after 1.5 s); any git failure yields [`FreshnessState::Unknown`] rather
/// than an error.
pub fn index_freshness(info: &RepoInfo) -> IndexFreshness {
    let head = crate::git_read::git_read(
        Path::new(&info.path),
        &["rev-parse", "HEAD"],
        FRESHNESS_GIT_TIMEOUT,
    )
    .ok()
    .map(|out| out.trim().to_string())
    .filter(|commit| !commit.is_empty());
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
    fn reached_hits_carry_via_and_edge_and_match_graph_hits() {
        let (s, r) = fixture();
        let tok = s.iter().find(|x| x.uid == "tok").unwrap();
        let reached = walk_calls_reached(&s, &r, tok, CallDirection::Callers, 3);
        let names: Vec<(&str, u32)> = reached
            .iter()
            .map(|h| (h.symbol.name.as_str(), h.depth))
            .collect();
        assert_eq!(
            names,
            [("parse_query", 1), ("handler", 2), ("run", 2), ("main", 3)]
        );
        let run = reached.iter().find(|h| h.symbol.uid == "run").unwrap();
        assert_eq!(run.via.unwrap().uid, "pq");
        assert_eq!(run.edge.uid, "run->pq");
        let hits: Vec<GraphHit> = reached.iter().map(Reached::to_hit).collect();
        assert_eq!(hits, walk_calls(&s, &r, tok, CallDirection::Callers, 3));

        let deps = dependents_reached(&s, &r, &[tok], 1);
        let handler = deps.iter().find(|h| h.symbol.uid == "handler").unwrap();
        assert_eq!(handler.edge.kind, RelationshipKind::Imports);
    }

    #[test]
    fn call_chains_name_the_shortest_chain_and_everything_on_one() {
        use SymbolKind::Function;
        // main -> a -> c -> target, main -> b -> target, b -> d (dead end),
        // main -> e -> f -> g -> target.
        let symbols: Vec<CodeSymbol> = ["main", "a", "b", "c", "d", "e", "f", "g", "target"]
            .iter()
            .enumerate()
            .map(|(i, n)| sym(n, n, n, Function, "src/x.rs", i as u32 + 1))
            .collect();
        let rels: Vec<Relationship> = [
            ("main", "a"),
            ("a", "c"),
            ("c", "target"),
            ("main", "b"),
            ("b", "target"),
            ("b", "d"),
            ("main", "e"),
            ("e", "f"),
            ("f", "g"),
            ("g", "target"),
        ]
        .iter()
        .map(|(f, t)| calls(f, t))
        .collect();
        let (main, target) = (&symbols[0], &symbols[8]);
        let on = |c: &CallChains| -> Vec<(String, u32)> {
            c.on_chains
                .iter()
                .map(|s| (s.symbol.name.clone(), s.chain_len))
                .collect()
        };

        let chains = call_chains(&symbols, &rels, main, target, 10, 0).unwrap();
        assert_eq!(chains.len(), 2);
        let shortest: Vec<&str> = chains
            .shortest
            .iter()
            .map(|(s, _)| s.name.as_str())
            .collect();
        assert_eq!(shortest, ["main", "b", "target"]);
        assert!(chains.shortest[0].1.is_none());
        assert_eq!(chains.shortest[2].1.unwrap().uid, "b->target");
        assert_eq!(on(&chains), [("b".to_string(), 2)], "d is a dead end");

        let wider = call_chains(&symbols, &rels, main, target, 10, 2).unwrap();
        let expected: Vec<(String, u32)> =
            [("b", 2), ("a", 3), ("c", 3), ("e", 4), ("f", 4), ("g", 4)]
                .iter()
                .map(|(n, l)| (n.to_string(), *l))
                .collect();
        assert_eq!(on(&wider), expected);
        let capped = call_chains(&symbols, &rels, main, target, 3, 5).unwrap();
        assert!(capped.on_chains.iter().all(|c| c.chain_len <= 3));
        assert!(capped.longer.is_empty(), "e-f-g is longer than max_depth");

        // What the window leaves out is named, not silently dropped (#169).
        let longer: Vec<(String, u32)> = chains
            .longer
            .iter()
            .map(|s| (s.symbol.name.clone(), s.chain_len))
            .collect();
        let expected: Vec<(String, u32)> = [("a", 3), ("c", 3), ("e", 4), ("f", 4), ("g", 4)]
            .iter()
            .map(|(n, l)| (n.to_string(), *l))
            .collect();
        assert_eq!(longer, expected);
        assert!(wider.longer.is_empty());

        // No chain within one call: what was reached, and what the depth
        // limit cut.
        let none = call_chains(&symbols, &rels, main, target, 1, 0).unwrap_err();
        let reached: Vec<&str> = none.reached.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(reached, ["a", "b", "e"]);
        let frontier: Vec<&str> = none.cut.frontier.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(frontier, ["a", "b", "e"]);
        assert_eq!(none.cut.beyond, 4, "c, target, d and f");
        let backwards = call_chains(&symbols, &rels, target, main, 10, 0).unwrap_err();
        assert!(
            backwards.reached.is_empty() && !backwards.cut.is_cut(),
            "chains follow calls forwards"
        );
    }

    #[test]
    fn a_bounded_walk_names_what_its_depth_limit_cut() {
        use SymbolKind::Function;
        let symbols: Vec<CodeSymbol> = ["main", "a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, n)| sym(n, n, n, Function, "src/x.rs", i as u32 + 1))
            .collect();
        // main -> a -> b -> c, and main -> b.
        let rels: Vec<Relationship> = [("main", "a"), ("a", "b"), ("b", "c"), ("main", "b")]
            .iter()
            .map(|(f, t)| calls(f, t))
            .collect();
        let main = &symbols[0];
        let one = walk_calls_bounded(&symbols, &rels, main, CallDirection::Callees, 1);
        let names: Vec<&str> = one.hits.iter().map(|h| h.symbol.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        // a -> b is no cut: b is listed already. b -> c is.
        let frontier: Vec<&str> = one.cut.frontier.iter().map(|s| s.name.as_str()).collect();
        assert_eq!((frontier, one.cut.beyond), (vec!["b"], 1));
        let all = walk_calls_bounded(&symbols, &rels, main, CallDirection::Callees, 2);
        assert_eq!(all.hits.len(), 3);
        assert!(!all.cut.is_cut());
        let callers = walk_calls_bounded(&symbols, &rels, &symbols[3], CallDirection::Callers, 1);
        assert_eq!((callers.cut.frontier.len(), callers.cut.beyond), (1, 2));
        let deps = dependents_bounded(&symbols, &rels, &[&symbols[3]], 3);
        assert_eq!(deps.hits.len(), 3);
        assert!(!deps.cut.is_cut());
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
