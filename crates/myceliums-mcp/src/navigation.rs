//! Graph navigation behind the MCP tools: an adapter over
//! [`myceliums_core::navigation`], the implementation the CLI
//! (`myc callers/callees/impact/context`) and the grep hook use too, so all
//! three give the same answer for the same name.
//!
//! This module adds only what the MCP contract needs on top:
//!
//! * [`resolve`] accepts `file_path:qualified_name` handles (and
//!   `file_path:qualified_name:line` for overloads in one file), turns
//!   core's ambiguous candidates into a recoverable error that lists those
//!   handles,
//!   suggests similar names when nothing matches, and falls back to symbols
//!   that are not code definitions (documents, emails) which core does not
//!   resolve;
//! * every hit carries how its CALLS edge was resolved (#96), from the edge
//!   core reports with it.

use crate::errors::ToolError;
use crate::repo::{located_name, CallLabels};
use myceliums_core::dispatch::{Dispatch, DispatchCut};
use myceliums_core::navigation::{self as core_nav, Reached, Resolution};
pub(crate) use myceliums_core::navigation::{ChainStep, DepthCut, NoChain};
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};

pub(crate) use core_nav::CallDirection as Direction;

/// How many names a "not found" error suggests.
const SUGGESTED: usize = 5;
/// How many candidates an "ambiguous symbol" error lists.
const AMBIGUOUS_SHOWN: usize = 8;

/// Finds exactly one symbol for `name`: a `file_path:qualified_name`
/// handle, else what core's resolver picks (qualified name, short name,
/// qualified suffix such as `Item.AsString`; functions and types win over
/// variables), else any symbol by qualified or short name.
///
/// Not found and ambiguous are recoverable errors naming the next step.
pub(crate) fn resolve<'a>(
    symbols: &'a [CodeSymbol],
    name: &str,
) -> Result<&'a CodeSymbol, ToolError> {
    let name = name.trim();
    if let Some(symbol) = symbols.iter().find(|s| is_located_name(s, name)) {
        return Ok(symbol);
    }
    match core_nav::resolve_symbol(symbols, name) {
        Resolution::Unique(symbol) => Ok(symbol),
        Resolution::Ambiguous(candidates) => ambiguous(name, &candidates),
        // Core resolves code definitions only; documents, emails and other
        // indexed nodes keep resolving by name as before.
        Resolution::NotFound => crate::repo::resolve_symbol(symbols, name).map_err(|e| {
            let suggestions = core_nav::similar_names(symbols, name, SUGGESTED);
            if matches!(&e, ToolError::Recoverable { problem, .. } if problem.contains("not found"))
                && !suggestions.is_empty()
            {
                ToolError::symbol_not_found_did_you_mean(name, &suggestions)
            } else {
                e
            }
        }),
    }
}

/// Whether `name` is `symbol`'s [`located_name`], or that followed by
/// `:<start line>`.
fn is_located_name(symbol: &CodeSymbol, name: &str) -> bool {
    name.strip_prefix(symbol.file_path.as_str())
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| {
            rest == symbol.qualified_name
                || rest == format!("{}:{}", symbol.qualified_name, symbol.start_line)
        })
}

/// Several definitions match, as `myc callers` reports too: list handles that
/// tell them apart — `file_path:qualified_name`, or with `:line` when
/// several share one (overloads in one file). Only candidates that not even
/// the line tells apart are one definition.
fn ambiguous<'a>(name: &str, candidates: &[&'a CodeSymbol]) -> Result<&'a CodeSymbol, ToolError> {
    let distinct = |handles: &mut Vec<String>| {
        handles.sort();
        handles.dedup();
        handles.len()
    };
    let mut handles: Vec<String> = candidates.iter().map(|s| located_name(s)).collect();
    if distinct(&mut handles) < candidates.len() {
        handles = candidates
            .iter()
            .map(|s| format!("{}:{}", located_name(s), s.start_line))
            .collect();
        if distinct(&mut handles) == 1 {
            return Ok(candidates[0]);
        }
    }
    let more = handles.len().saturating_sub(AMBIGUOUS_SHOWN);
    let more = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    let shown: Vec<String> = handles
        .iter()
        .take(AMBIGUOUS_SHOWN)
        .map(|h| format!("`{h}`"))
        .collect();
    Err(ToolError::recoverable(
        format!("Symbol `{name}` is ambiguous: {} matches.", handles.len()),
        format!(
            "pass one of these as the symbol name: {}{more}",
            shown.join(", ")
        ),
    ))
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
    /// `name-match`, `ambiguous`, or `dispatch` for a hit reached through
    /// dynamic dispatch); `None` for other edge kinds.
    pub(crate) resolved: Option<&'static str>,
    /// Set for a hit reached through dynamic dispatch (#190).
    pub(crate) dispatch: Option<Dispatch<'a>>,
}

impl Hit<'_> {
    /// `name`, marked with the edge resolution when the edge is a guess.
    pub(crate) fn marked_name(&self) -> String {
        format!("{}{}", self.symbol.name, self.mark())
    }

    /// How the hit was reached, for `resolved` fields: the edge's label, or
    /// `dispatch via Base.m, confidence 0.8`.
    pub(crate) fn resolution(&self) -> String {
        match &self.dispatch {
            Some(dispatch) => dispatch.describe(),
            None => self.resolved.unwrap_or("exact").to_string(),
        }
    }

    /// ` (dispatch via Base.m, confidence 0.8)` for a hit reached through
    /// dispatch, ` (resolved: name-match)` for a name-based guess, else
    /// nothing.
    pub(crate) fn mark(&self) -> String {
        match (&self.dispatch, self.resolved) {
            (Some(dispatch), _) => format!(" ({})", dispatch.describe()),
            (None, Some(label)) if CallLabels::is_heuristic(label) => {
                format!(" (resolved: {label})")
            }
            _ => String::new(),
        }
    }
}

/// Labels core's hits with the resolution of their CALLS edges.
fn labelled<'a>(symbols: &'a [CodeSymbol], reached: Vec<Reached<'a>>) -> Vec<Hit<'a>> {
    let labels = CallLabels::new(symbols);
    reached
        .into_iter()
        .map(|r| Hit {
            symbol: r.symbol,
            depth: r.depth,
            via: r.via,
            edge: r.edge,
            resolved: match r.dispatch {
                Some(_) => Some(DISPATCH),
                None => (r.edge.kind == RelationshipKind::Calls)
                    .then(|| labels.label(r.edge, &edge_target(symbols, &r).name)),
            },
            dispatch: r.dispatch,
        })
        .collect()
}

/// The label of a hit reached through dynamic dispatch.
pub(crate) const DISPATCH: &str = "dispatch";

/// The symbol at the target end of the edge that reached `r`.
fn edge_target<'a>(symbols: &'a [CodeSymbol], r: &Reached<'a>) -> &'a CodeSymbol {
    if r.edge.target_uid == r.symbol.uid {
        r.symbol
    } else if let Some(via) = r.via.filter(|v| v.uid == r.edge.target_uid) {
        via
    } else {
        symbols
            .iter()
            .find(|s| s.uid == r.edge.target_uid)
            .unwrap_or(r.symbol)
    }
}

/// Callers or callees of `start` over CALLS edges, up to `max_depth` hops,
/// ordered by depth, then file and line (core's order).
pub(crate) fn walk_calls<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    start: &'a CodeSymbol,
    direction: Direction,
    max_depth: u32,
) -> Vec<Hit<'a>> {
    labelled(
        symbols,
        core_nav::walk_calls_reached(symbols, relationships, start, direction, max_depth),
    )
}

/// [`walk_calls`], with what the depth limit cut (#169) and the calls whose
/// implementations it lists only in part (#190).
pub(crate) fn walk_calls_bounded<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    start: &'a CodeSymbol,
    direction: Direction,
    max_depth: u32,
) -> (Vec<Hit<'a>>, DepthCut<'a>, Vec<DispatchCut<'a>>) {
    let walk = core_nav::walk_calls_bounded(symbols, relationships, start, direction, max_depth);
    (labelled(symbols, walk.hits), walk.cut, walk.dispatch_cut)
}

/// Symbols that call or import any of `roots`, transitively up to
/// `max_depth` (core's blast radius): the code a change to the roots can
/// break. Roots themselves are not reported.
#[cfg(test)]
pub(crate) fn dependents<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    roots: &[&'a CodeSymbol],
    max_depth: u32,
) -> Vec<Hit<'a>> {
    dependents_bounded(symbols, relationships, roots, max_depth).0
}

/// [`dependents`], with what the depth limit cut (#169).
pub(crate) fn dependents_bounded<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    roots: &[&'a CodeSymbol],
    max_depth: u32,
) -> (Vec<Hit<'a>>, DepthCut<'a>) {
    let walk = core_nav::dependents_bounded(symbols, relationships, roots, max_depth);
    (labelled(symbols, walk.hits), walk.cut)
}

/// What `symbol` overrides and what implements it (#190), as
/// `file:qualified_name` handles ordered by file and line: the methods it
/// overrides or implements, and the concrete implementations of it.
pub(crate) fn overrides(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    symbol: &CodeSymbol,
) -> (Vec<String>, Vec<String>) {
    let index = myceliums_core::dispatch::DispatchIndex::new(relationships);
    let handles = |uids: Vec<&str>| -> Vec<String> {
        let mut found: Vec<&CodeSymbol> = symbols
            .iter()
            .filter(|s| uids.contains(&s.uid.as_str()))
            .collect();
        found.sort_by(|a, b| (&a.file_path, a.start_line).cmp(&(&b.file_path, b.start_line)));
        found.into_iter().map(located_name).collect()
    };
    let up = index
        .overridden(&symbol.uid)
        .iter()
        .map(|o| o.other)
        .collect();
    let down = index
        .implementations(&symbol.uid)
        .iter()
        .map(|o| o.other)
        .collect();
    (handles(up), handles(down))
}

/// The definitions in the indexed file `path` (core's matching: exact path,
/// else a unique path suffix).
pub(crate) fn symbols_in_file<'a>(symbols: &'a [CodeSymbol], path: &str) -> Vec<&'a CodeSymbol> {
    core_nav::symbols_in_file(symbols, path)
}

/// How `from` reaches `to` over CALLS edges, with the shortest chain's edges
/// labelled.
#[derive(Debug)]
pub(crate) struct CallChains<'a> {
    /// One shortest chain, `from` first; every later symbol comes with the
    /// resolution of the CALLS edge that reached it (`dispatch via …` for a
    /// call that reaches it through dynamic dispatch).
    pub(crate) shortest: Vec<(&'a CodeSymbol, Option<String>)>,
    /// Every symbol strictly between the ends on a chain within the widened
    /// length (see [`core_nav::call_chains`]).
    pub(crate) on_chains: Vec<ChainStep<'a>>,
    /// Symbols on longer chains within `max_depth` that `on_chains` leaves
    /// out.
    pub(crate) longer: Vec<ChainStep<'a>>,
}

impl CallChains<'_> {
    /// Calls in the shortest chain.
    pub(crate) fn len(&self) -> u32 {
        self.shortest.len().saturating_sub(1) as u32
    }
}

/// Call chains from `from` to `to`, or what the search reached when `to` is
/// not reachable within `max_depth` calls (see [`core_nav::call_chains`]).
pub(crate) fn call_chains<'a>(
    symbols: &'a [CodeSymbol],
    relationships: &'a [Relationship],
    from: &'a CodeSymbol,
    to: &'a CodeSymbol,
    max_depth: u32,
    extra: u32,
) -> Result<CallChains<'a>, NoChain<'a>> {
    let chains = core_nav::call_chains(symbols, relationships, from, to, max_depth, extra)?;
    let labels = CallLabels::new(symbols);
    Ok(CallChains {
        shortest: chains
            .shortest
            .into_iter()
            .zip(chains.dispatched)
            .map(|((symbol, edge), dispatch)| {
                let label = match dispatch {
                    Some(dispatch) => Some(dispatch.describe()),
                    None => edge.map(|e| labels.label(e, &symbol.name).to_string()),
                };
                (symbol, label)
            })
            .collect(),
        on_chains: chains.on_chains,
        longer: chains.longer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn sym(uid: &str, name: &str, qualified: &str, kind: SymbolKind, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.into(),
            name: name.into(),
            qualified_name: qualified.into(),
            kind,
            file_path: file.into(),
            start_line: 1,
            end_line: 2,
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

    fn graph() -> (Vec<CodeSymbol>, Vec<Relationship>) {
        use SymbolKind::*;
        let symbols = vec![
            sym("main", "main", "main", Function, "src/main.ts"),
            sym(
                "get",
                "getUser",
                "UserService.getUser",
                Method,
                "src/user.ts",
            ),
            sym("find", "findById", "Database.findById", Method, "src/db.ts"),
            // A local variable sharing the method's name.
            sym("var", "getUser", "main.getUser", Variable, "src/main.ts"),
            sym("doc", "Setup guide", "Setup guide", Section, "README.md"),
        ];
        let rels = vec![
            edge(RelationshipKind::Calls, "main", "get"),
            edge(RelationshipKind::Calls, "get", "find"),
            edge(RelationshipKind::Imports, "main", "find"),
        ];
        (symbols, rels)
    }

    #[test]
    fn resolution_follows_core_and_keeps_the_mcp_extras() {
        let (s, _) = graph();
        // Core: the method wins over the variable of the same name, and a
        // qualified suffix resolves.
        assert_eq!(resolve(&s, "getUser").unwrap().uid, "get");
        assert_eq!(resolve(&s, "UserService.getUser").unwrap().uid, "get");
        // MCP: located handles, and non-definitions by name.
        assert_eq!(resolve(&s, "src/main.ts:main.getUser").unwrap().uid, "var");
        assert_eq!(resolve(&s, "Setup guide").unwrap().uid, "doc");
        // Not found suggests similar names.
        let err = resolve(&s, "getuse").unwrap_err().message();
        assert!(
            err.contains("not found") && err.contains("`getUser`"),
            "{err}"
        );
    }

    #[test]
    fn overloads_are_ambiguous_like_in_the_cli_and_a_line_handle_picks_one() {
        let mut first = sym("o1", "f", "C.f", SymbolKind::Method, "src/c.ts");
        let mut second = sym("o2", "f", "C.f", SymbolKind::Method, "src/c.ts");
        first.start_line = 3;
        second.start_line = 9;
        let s = vec![first, second];
        assert!(matches!(
            core_nav::resolve_symbol(&s, "C.f"),
            Resolution::Ambiguous(_)
        ));
        let msg = resolve(&s, "C.f").unwrap_err().message();
        assert!(msg.contains("ambiguous: 2 matches"), "{msg}");
        assert!(msg.contains("`src/c.ts:C.f:3`, `src/c.ts:C.f:9`"), "{msg}");
        assert_eq!(resolve(&s, "src/c.ts:C.f:9").unwrap().uid, "o2");
    }

    #[test]
    fn hits_carry_the_resolution_of_their_calls_edge() {
        let (s, r) = graph();
        let find = &s[2];
        let callers = walk_calls(&s, &r, find, Direction::Callers, 2);
        let names: Vec<String> = callers.iter().map(Hit::marked_name).collect();
        // Edges without a recorded resolution are name matches; the shared
        // name `getUser` makes main's edge into it ambiguous (#96).
        assert_eq!(
            names,
            [
                "getUser (resolved: name-match)",
                "main (resolved: ambiguous)"
            ]
        );
        assert_eq!(callers[1].via.unwrap().uid, "get");

        let deps = dependents(&s, &r, &[find], 1);
        let main = deps.iter().find(|h| h.symbol.uid == "main").unwrap();
        assert_eq!(main.edge.kind, RelationshipKind::Imports);
        assert_eq!(main.resolved, None, "only CALLS edges carry a resolution");

        let chains = call_chains(&s, &r, &s[0], find, 5, 0).unwrap();
        assert_eq!(chains.len(), 2);
        assert_eq!(chains.shortest[0].1, None);
        assert_eq!(chains.shortest[1].1.as_deref(), Some("ambiguous"));
        assert_eq!(chains.shortest[2].1.as_deref(), Some("name-match"));
    }

    /// An interface method `Port.send` with `impls` implementations, and a
    /// caller through the interface.
    fn dispatch_graph(impls: usize) -> (Vec<CodeSymbol>, Vec<Relationship>) {
        use myceliums_core::dispatch::{DispatchConfidence, OverriddenKind};
        let mut symbols = vec![
            sym(
                "caller",
                "notify",
                "Alerts.notify",
                SymbolKind::Method,
                "src/a.java",
            ),
            sym(
                "port",
                "send",
                "Port.send",
                SymbolKind::Method,
                "src/port.java",
            ),
        ];
        let mut call = edge(RelationshipKind::Calls, "caller", "port");
        call.metadata = "line:3;resolved:exact".into();
        let mut rels = vec![call];
        for i in 0..impls {
            let uid = format!("impl{i}");
            symbols.push(sym(
                &uid,
                "send",
                &format!("Port{i}.send"),
                SymbolKind::Method,
                &format!("src/port{i}.java"),
            ));
            let mut overrides = edge(RelationshipKind::Overrides, &uid, "port");
            overrides.metadata = DispatchConfidence {
                overridden: OverriddenKind::Abstract,
                implementations: impls,
                abstract_override: false,
                universal: false,
            }
            .to_metadata();
            rels.push(overrides);
        }
        (symbols, rels)
    }

    #[test]
    fn dispatch_hits_say_through_which_method() {
        let (s, r) = dispatch_graph(2);
        let impl0 = s.iter().find(|x| x.uid == "impl0").unwrap();
        let callers = walk_calls(&s, &r, impl0, Direction::Callers, 1);
        let names: Vec<String> = callers.iter().map(Hit::marked_name).collect();
        assert_eq!(names, ["notify (dispatch via Port.send, confidence 0.8)"]);
        assert_eq!(callers[0].resolved, Some(DISPATCH));
        assert_eq!(
            callers[0].resolution(),
            "dispatch via Port.send, confidence 0.8"
        );
        // impact_of_change on the implementation reaches the caller too.
        let deps = dependents(&s, &r, &[impl0], 1);
        assert_eq!(deps.len(), 1);
        assert!(deps[0].dispatch.is_some());

        let callees = walk_calls(&s, &r, &s[0], Direction::Callees, 1);
        let names: Vec<String> = callees.iter().map(Hit::marked_name).collect();
        assert_eq!(
            names,
            [
                "send",
                "send (dispatch via Port.send, confidence 0.8)",
                "send (dispatch via Port.send, confidence 0.8)"
            ]
        );
        // trace_flow through the interface to an implementation.
        let chains = call_chains(&s, &r, &s[0], impl0, 3, 0).unwrap();
        assert_eq!(
            chains.shortest[1].1.as_deref(),
            Some("dispatch via Port.send, confidence 0.8")
        );
        let (up, down) = overrides(&s, &r, impl0);
        assert_eq!(
            (up.as_slice(), down.len()),
            (["src/port.java:Port.send".to_string()].as_slice(), 0)
        );
        let (_, down) = overrides(&s, &r, &s[1]);
        assert_eq!(
            down,
            ["src/port0.java:Port0.send", "src/port1.java:Port1.send"]
        );
    }

    #[test]
    fn a_callee_list_caps_the_implementations_of_a_wide_call() {
        let (s, r) = dispatch_graph(20);
        let (hits, _, cut) = walk_calls_bounded(&s, &r, &s[0], Direction::Callees, 1);
        let dispatched = hits.iter().filter(|h| h.dispatch.is_some()).count();
        assert_eq!(
            dispatched,
            myceliums_core::dispatch::MAX_LISTED_IMPLEMENTATIONS
        );
        assert_eq!(cut.len(), 1);
        assert_eq!((cut[0].base.uid.as_str(), cut[0].total), ("port", 20));
        // A wide interface: low confidence.
        assert!(hits
            .iter()
            .filter_map(|h| h.dispatch)
            .all(|d| d.confidence <= 0.2));
    }

    #[test]
    fn answers_match_the_cli() {
        // `myc callers/callees` and `myc impact` print core's GraphHits; the
        // MCP tools must list the same symbols, depths and via names.
        let (s, r) = graph();
        let find = &s[2];
        let key = |q: &str, d: u32, v: Option<String>| (q.to_string(), d, v);
        for direction in [Direction::Callers, Direction::Callees] {
            let start = if direction == Direction::Callers {
                find
            } else {
                &s[0]
            };
            let mcp: Vec<_> = walk_calls(&s, &r, start, direction, 3)
                .iter()
                .map(|h| {
                    key(
                        &h.symbol.qualified_name,
                        h.depth,
                        h.via.map(|v| v.name.clone()),
                    )
                })
                .collect();
            let cli: Vec<_> = core_nav::walk_calls(&s, &r, start, direction, 3)
                .into_iter()
                .map(|h| key(&h.symbol.qualified_name, h.depth, h.via))
                .collect();
            assert!(!mcp.is_empty());
            assert_eq!(mcp, cli, "{direction:?}");
        }
        let mcp: Vec<_> = dependents(&s, &r, &[find], 2)
            .iter()
            .map(|h| {
                key(
                    &h.symbol.qualified_name,
                    h.depth,
                    h.via.map(|v| v.name.clone()),
                )
            })
            .collect();
        let cli: Vec<_> = core_nav::blast_radius(&s, &r, &[find], 2)
            .dependents
            .into_iter()
            .map(|h| key(&h.symbol.qualified_name, h.depth, h.via))
            .collect();
        assert_eq!(mcp, cli);
    }
}
