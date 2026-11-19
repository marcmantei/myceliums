//! Graph navigation behind the task-named tools: symbol resolution, caller
//! and callee walks, dependents (the blast radius of a change) and call
//! chains between two symbols.
//!
//! Every tool that answers "who calls / what calls / what depends on / how
//! does A reach B" goes through this module, so `find_callers`,
//! `explain_symbol`, `impact_of_change`, `trace_flow` and the deprecated
//! `get_callers`/`get_callees`/`symbol_context` aliases give the same answer.
//!
//! **Seam for #126.** Core's `myceliums_core::navigation` (PR #133) answers
//! the same questions for the CLI (`myc callers/callees/impact`) and the grep
//! hook. Once it is on main, the bodies of [`resolve`], [`walk_calls`] and
//! [`dependents`] delegate to its `resolve_symbol`, `walk_calls` and
//! `blast_radius`; no handler changes. Two things to reconcile then: core's
//! resolver also matches qualified suffixes (`Item.AsString`) and returns
//! candidates instead of an error (map them onto
//! [`crate::errors::ToolError`] as [`crate::repo::resolve_symbol`] does), and
//! core's `GraphHit` names `via` but not the edge, which the MCP tools need
//! for the CALLS resolution label (#96).

use crate::errors::ToolError;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet, VecDeque};

/// Per symbol uid: calls from the start, and the (previous uid, edge) that
/// reached it on a shortest way (`None` for the start).
type Distances<'a> = HashMap<&'a str, (u32, Option<(&'a str, &'a Relationship)>)>;

/// Finds exactly one symbol by `file_path:qualified_name`, qualified name or
/// short name; not found and ambiguous are recoverable errors.
pub(crate) fn resolve<'a>(
    symbols: &'a [CodeSymbol],
    name: &str,
) -> Result<&'a CodeSymbol, ToolError> {
    crate::repo::resolve_symbol(symbols, name)
}

/// Which way to follow CALLS edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Who calls the start symbol.
    Callers,
    /// What the start symbol calls.
    Callees,
}

/// A symbol reached by a walk.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hit<'a> {
    pub(crate) symbol: &'a CodeSymbol,
    /// Edges from the nearest start symbol (1 = direct).
    pub(crate) depth: u32,
    /// For depth > 1: the symbol one step closer to the start.
    pub(crate) via: Option<&'a CodeSymbol>,
    /// The edge that reached `symbol`.
    pub(crate) edge: &'a Relationship,
    /// For a CALLS edge: how its target was resolved (`exact`, `import`,
    /// `name-match`, `ambiguous`); `None` for other edge kinds.
    pub(crate) resolved: Option<&'static str>,
}

impl Hit<'_> {
    /// `name`, marked with the edge resolution when the edge is a guess.
    pub(crate) fn marked_name(&self) -> String {
        match self.resolved {
            Some(label) => crate::repo::CallLabels::mark(label, &self.symbol.name),
            None => self.symbol.name.clone(),
        }
    }
}

/// Callers or callees of `start` over CALLS edges, up to `max_depth` hops.
/// Each symbol is reported once, at its shortest distance; ordered by depth,
/// then file and line.
pub(crate) fn walk_calls<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    start: &'a CodeSymbol,
    direction: Direction,
    max_depth: u32,
) -> Vec<Hit<'a>> {
    walk(
        symbols,
        relationships,
        &[start],
        &[RelationshipKind::Calls],
        direction == Direction::Callers,
        max_depth,
    )
}

/// Symbols that call or import any of `roots`, transitively up to
/// `max_depth` (the code a change to the roots can break). Roots themselves
/// are not reported.
pub(crate) fn dependents<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    roots: &[&'a CodeSymbol],
    max_depth: u32,
) -> Vec<Hit<'a>> {
    walk(
        symbols,
        relationships,
        roots,
        &[RelationshipKind::Calls, RelationshipKind::Imports],
        true,
        max_depth,
    )
}

/// Breadth-first walk over `kinds` edges from `starts`; `backwards` follows
/// edges from target to source.
fn walk<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    starts: &[&'a CodeSymbol],
    kinds: &[RelationshipKind],
    backwards: bool,
    max_depth: u32,
) -> Vec<Hit<'a>> {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let labels = crate::repo::CallLabels::new(symbols);
    let mut next: HashMap<&str, Vec<(&str, &Relationship)>> = HashMap::new();
    for rel in relationships.iter().filter(|r| kinds.contains(&r.kind)) {
        let (from, to) = if backwards {
            (rel.target_uid.as_str(), rel.source_uid.as_str())
        } else {
            (rel.source_uid.as_str(), rel.target_uid.as_str())
        };
        next.entry(from).or_default().push((to, rel));
    }

    let mut seen: HashSet<&str> = starts.iter().map(|s| s.uid.as_str()).collect();
    let mut queue: VecDeque<(&str, u32)> = starts.iter().map(|s| (s.uid.as_str(), 0)).collect();
    let mut hits = Vec::new();
    while let Some((uid, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for &(n, edge) in next.get(uid).map(Vec::as_slice).unwrap_or(&[]) {
            // Edges into nodes that are not symbols are neither reported
            // nor walked through.
            let Some(symbol) = by_uid.get(n) else {
                continue;
            };
            if !seen.insert(n) {
                continue;
            }
            let via = if depth > 0 {
                by_uid.get(uid).copied()
            } else {
                None
            };
            let resolved = (edge.kind == RelationshipKind::Calls).then(|| {
                let target = by_uid
                    .get(edge.target_uid.as_str())
                    .map(|t| t.name.as_str())
                    .unwrap_or_default();
                labels.label(edge, target)
            });
            hits.push(Hit {
                symbol,
                depth: depth + 1,
                via,
                edge,
                resolved,
            });
            queue.push_back((n, depth + 1));
        }
    }
    hits.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    hits
}

fn sort_key<'a>(hit: &Hit<'a>) -> (u32, &'a str, u32, &'a str) {
    (
        hit.depth,
        hit.symbol.file_path.as_str(),
        hit.symbol.start_line,
        hit.symbol.qualified_name.as_str(),
    )
}

/// A symbol between the two ends of [`CallChains`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainStep<'a> {
    pub(crate) symbol: &'a CodeSymbol,
    /// Calls from the start to this symbol, on the shortest way.
    pub(crate) from_start: u32,
    /// Calls in the shortest chain that passes through this symbol.
    pub(crate) chain_len: u32,
}

/// How `from` reaches `to` over CALLS edges.
#[derive(Debug)]
pub(crate) struct CallChains<'a> {
    /// One shortest chain, `from` first and `to` last; every symbol after the
    /// first comes with the resolution of the CALLS edge that reached it.
    pub(crate) shortest: Vec<(&'a CodeSymbol, Option<&'static str>)>,
    /// Every symbol strictly between the ends on a chain of at most
    /// `shortest + extra` calls, ordered by chain length, then distance from
    /// the start, then file and line.
    pub(crate) on_chains: Vec<ChainStep<'a>>,
}

impl CallChains<'_> {
    /// Calls in the shortest chain.
    pub(crate) fn len(&self) -> u32 {
        self.shortest.len().saturating_sub(1) as u32
    }
}

/// Call chains from `from` to `to` (following CALLS edges forwards), or
/// `None` when `to` is not reachable within `max_depth` calls. `extra`
/// widens [`CallChains::on_chains`] to chains up to that many calls longer
/// than the shortest (still at most `max_depth`).
pub(crate) fn call_chains<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    from: &'a CodeSymbol,
    to: &'a CodeSymbol,
    max_depth: u32,
    extra: u32,
) -> Option<CallChains<'a>> {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut forward: HashMap<&str, Vec<(&str, &Relationship)>> = HashMap::new();
    let mut backward: HashMap<&str, Vec<&str>> = HashMap::new();
    for rel in relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
    {
        let (s, t) = (rel.source_uid.as_str(), rel.target_uid.as_str());
        if by_uid.contains_key(s) && by_uid.contains_key(t) {
            forward.entry(s).or_default().push((t, rel));
            backward.entry(t).or_default().push(s);
        }
    }

    // Distances from the start (with the edge that reached each symbol).
    let mut from_start: Distances = HashMap::new();
    from_start.insert(from.uid.as_str(), (0, None));
    let mut queue = VecDeque::from([(from.uid.as_str(), 0u32)]);
    while let Some((uid, d)) = queue.pop_front() {
        if d >= max_depth {
            continue;
        }
        for &(n, rel) in forward.get(uid).map(Vec::as_slice).unwrap_or(&[]) {
            if let std::collections::hash_map::Entry::Vacant(e) = from_start.entry(n) {
                e.insert((d + 1, Some((uid, rel))));
                queue.push_back((n, d + 1));
            }
        }
    }
    let shortest_len = from_start.get(to.uid.as_str())?.0;

    // Distances to the end, walking CALLS edges backwards.
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

    let labels = crate::repo::CallLabels::new(symbols);
    let mut shortest = Vec::new();
    let mut cursor = to.uid.as_str();
    while let Some((_, step)) = from_start.get(cursor) {
        let symbol = by_uid[cursor];
        shortest.push((symbol, step.map(|(_, rel)| labels.label(rel, &symbol.name))));
        match step {
            Some((prev, _)) => cursor = prev,
            None => break,
        }
    }
    shortest.reverse();

    let limit = shortest_len.saturating_add(extra).min(max_depth);
    let mut on_chains: Vec<ChainStep> = from_start
        .iter()
        .filter(|(uid, _)| **uid != from.uid && **uid != to.uid)
        .filter_map(|(uid, (d, _))| {
            let rest = to_end.get(uid)?;
            let chain_len = d + rest;
            (chain_len <= limit).then(|| ChainStep {
                symbol: by_uid[uid],
                from_start: *d,
                chain_len,
            })
        })
        .collect();
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
    Some(CallChains {
        shortest,
        on_chains,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn sym(name: &str, file: &str, line: u32) -> CodeSymbol {
        CodeSymbol {
            uid: name.into(),
            name: name.into(),
            qualified_name: name.into(),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: line,
            end_line: line + 1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    fn edge(kind: RelationshipKind, from: &str, to: &str) -> Relationship {
        Relationship {
            uid: format!("{from}->{to}"),
            source_uid: from.into(),
            target_uid: to.into(),
            kind,
            repo_id: "r".into(),
            metadata: String::new(),
        }
    }

    fn calls(from: &str, to: &str) -> Relationship {
        edge(RelationshipKind::Calls, from, to)
    }

    /// main → a → c → target, main → b → target, b → d (dead end),
    /// main → e → f → g → target (a longer chain).
    fn graph() -> (Vec<CodeSymbol>, Vec<Relationship>) {
        let symbols = ["main", "a", "b", "c", "d", "e", "f", "g", "target"]
            .iter()
            .enumerate()
            .map(|(i, n)| sym(n, "src/x.rs", i as u32 + 1))
            .collect();
        let rels = vec![
            calls("main", "a"),
            calls("a", "c"),
            calls("c", "target"),
            calls("main", "b"),
            calls("b", "target"),
            calls("b", "d"),
            calls("main", "e"),
            calls("e", "f"),
            calls("f", "g"),
            calls("g", "target"),
        ];
        (symbols, rels)
    }

    fn names(hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| format!("{}@{}", h.symbol.name, h.depth))
            .collect()
    }

    #[test]
    fn walks_report_each_symbol_once_at_its_shortest_depth() {
        let (s, r) = graph();
        let target = &s[8];
        let callers = walk_calls(&s, &r, target, Direction::Callers, 1);
        assert_eq!(names(&callers), ["b@1", "c@1", "g@1"]);
        let deep = walk_calls(&s, &r, target, Direction::Callers, 3);
        assert_eq!(
            names(&deep),
            ["b@1", "c@1", "g@1", "main@2", "a@2", "f@2", "e@3"]
        );
        let a = deep.iter().find(|h| h.symbol.name == "a").unwrap();
        assert_eq!(a.via.unwrap().name, "c");
        assert_eq!(a.edge.uid, "a->c");
        // Edges without a recorded resolution are name matches (#96).
        assert_eq!(a.resolved, Some("name-match"));
        assert_eq!(a.marked_name(), "a (resolved: name-match)");

        let callees = walk_calls(&s, &r, &s[0], Direction::Callees, 1);
        assert_eq!(names(&callees), ["a@1", "b@1", "e@1"]);
    }

    #[test]
    fn dependents_follow_calls_and_imports_backwards() {
        let (s, mut r) = graph();
        r.push(edge(RelationshipKind::Imports, "d", "c"));
        let deps = dependents(&s, &r, &[&s[3]], 2);
        assert_eq!(names(&deps), ["a@1", "d@1", "main@2", "b@2"]);
        let d = deps.iter().find(|h| h.symbol.name == "d").unwrap();
        assert_eq!(d.edge.kind, RelationshipKind::Imports);
        assert_eq!(d.resolved, None, "only CALLS edges carry a resolution");
        let two_roots = dependents(&s, &r, &[&s[3], &s[1]], 1);
        assert_eq!(names(&two_roots), ["main@1", "d@1"]);
    }

    #[test]
    fn call_chains_name_the_shortest_chain_and_everything_on_one() {
        let (s, r) = graph();
        let chains = call_chains(&s, &r, &s[0], &s[8], 10, 0).unwrap();
        assert_eq!(chains.len(), 2);
        let shortest: Vec<&str> = chains
            .shortest
            .iter()
            .map(|(s, _)| s.name.as_str())
            .collect();
        assert_eq!(shortest, ["main", "b", "target"]);
        assert!(chains.shortest[0].1.is_none() && chains.shortest[2].1.is_some());
        let on: Vec<(&str, u32)> = chains
            .on_chains
            .iter()
            .map(|c| (c.symbol.name.as_str(), c.chain_len))
            .collect();
        assert_eq!(on, [("b", 2)], "d is a dead end, not on a chain");

        let wider = call_chains(&s, &r, &s[0], &s[8], 10, 2).unwrap();
        let on: Vec<(&str, u32)> = wider
            .on_chains
            .iter()
            .map(|c| (c.symbol.name.as_str(), c.chain_len))
            .collect();
        assert_eq!(
            on,
            [("b", 2), ("a", 3), ("c", 3), ("e", 4), ("f", 4), ("g", 4)]
        );
        // max_depth caps both the search and the widened set.
        let capped = call_chains(&s, &r, &s[0], &s[8], 3, 5).unwrap();
        assert!(capped.on_chains.iter().all(|c| c.chain_len <= 3));
        assert!(call_chains(&s, &r, &s[0], &s[8], 1, 0).is_none());
        assert!(
            call_chains(&s, &r, &s[8], &s[0], 10, 0).is_none(),
            "chains follow calls forwards"
        );
    }
}
