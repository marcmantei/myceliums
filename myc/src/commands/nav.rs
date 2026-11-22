//! One-shot graph navigation for agents and hooks: `myc callers`,
//! `myc callees`, `myc impact <symbol|file>` and `myc context` (#126).
//!
//! Each command prints compact, token-budgeted text (or `--json`) and ends
//! with the index freshness, so an agent calling it through a shell gets a
//! complete answer in one call. The graph logic lives in
//! [`myceliums_core::navigation`]; this module only resolves the repository,
//! loads the tables and formats.
//!
//! Exit codes: 0 answered, 1 symbol or file not found (with suggestions),
//! 2 ambiguous name (candidates with file:line on stdout).

use std::io::Write as _;

use anyhow::{Context as _, Result};
use myceliums_core::gaps::Gaps;
use myceliums_core::navigation::{
    blast_radius, build_context_pack, build_context_pack_ranked, dependents_bounded, fit_to_budget,
    index_freshness, resolve_symbol, similar_names, symbols_in_file, walk_calls_bounded,
    CallDirection, ContextOptions, GraphHit, IndexFreshness, Reached, SymbolMatch, SymbolRef,
};
use myceliums_core::GraphTable;
use myceliums_storage::{CodeSymbol, Relationship, RepoInfo, RepoRegistry, Store};
use serde_json::json;
use std::sync::Arc;

/// Default output budget, in estimated tokens, for every navigation command.
pub const DEFAULT_BUDGET: usize = 2000;

/// Output format shared by the navigation commands.
#[derive(Debug, Clone, Copy)]
pub struct OutputOptions {
    /// Print JSON instead of text.
    pub json: bool,
    /// Upper bound for the output size in estimated tokens.
    pub budget: usize,
}

/// The repository a navigation command reads.
///
/// An explicit `--repo` (id or path) wins. Otherwise the indexed repository
/// containing the working directory is used. Unlike other commands there is
/// no fallback to the most recently analyzed repository: an agent in an
/// unindexed directory must not get answers about a different codebase.
fn resolve_repo(repo: Option<&str>) -> Result<RepoInfo> {
    if let Some(r) = repo {
        return Ok(crate::repo::resolve_repo(r)?.1);
    }
    let registry = RepoRegistry::load(&crate::repo::registry_path())?;
    let cwd = std::env::current_dir().context("Cannot read the working directory")?;
    registry.find_containing(&cwd).cloned().with_context(|| {
        format!(
            "No myceliums index covers {}. Run `myc analyze .` in the project root, or pass --repo.",
            cwd.display()
        )
    })
}

/// A repository's symbols and relationships, loaded once per command.
struct Graph {
    info: RepoInfo,
    store: Arc<Store>,
    symbols: GraphTable<CodeSymbol>,
    relationships: GraphTable<Relationship>,
}

/// Reads both tables from one [`myceliums_core::GraphService`] snapshot, so they come from
/// the same index generation and a missing index is an error rather than a
/// freshly created empty database.
async fn load_graph(repo: Option<&str>) -> Result<Graph> {
    let info = resolve_repo(repo)?;
    let service = crate::repo::graph_service();
    let graph = service.graph(&info.id).await?;
    Ok(Graph {
        symbols: graph.symbols().await?,
        relationships: graph.relationships().await?,
        store: graph.store(),
        info,
    })
}

/// Prints `out`, flushes, and exits with `code` (non-zero outcomes that are
/// still answers: not found, ambiguous).
fn finish(out: &str, code: i32) -> ! {
    print!("{out}");
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

fn ref_line(r: &SymbolRef) -> String {
    format!("{} ({}) {}", r.qualified_name, r.kind, r.location())
}

fn hit_line(h: &GraphHit, show_depth: bool) -> String {
    let mut line = String::from("  ");
    if show_depth {
        line.push_str(&format!("[{}] ", h.depth));
    }
    line.push_str(&format!(
        "{}  {}",
        h.symbol.qualified_name,
        h.symbol.location()
    ));
    if let Some(via) = &h.via {
        line.push_str(&format!("  via {via}"));
    }
    line
}

/// Resolves `name` or exits with the not-found / ambiguous answer.
fn require_symbol<'a>(graph: &'a Graph, name: &str, opts: OutputOptions) -> &'a CodeSymbol {
    match resolve_symbol(&graph.symbols, name) {
        SymbolMatch::Unique(sym) => sym,
        SymbolMatch::Ambiguous(candidates) => {
            let refs: Vec<SymbolRef> = candidates.iter().map(|s| SymbolRef::of(s)).collect();
            finish(&render_ambiguous(&graph.info, name, &refs, opts), 2)
        }
        SymbolMatch::NotFound => {
            let suggestions = similar_names(&graph.symbols, name, 5);
            finish(&render_not_found(&graph.info, name, &suggestions, opts), 1)
        }
    }
}

fn render_ambiguous(
    info: &RepoInfo,
    name: &str,
    refs: &[SymbolRef],
    opts: OutputOptions,
) -> String {
    if opts.json {
        return json!({
            "repo": info.name,
            "error": "ambiguous",
            "query": name,
            "candidates": refs,
        })
        .to_string()
            + "\n";
    }
    let (shown, omitted) = fit_to_budget(refs.to_vec(), opts.budget, ref_line);
    let mut out = format!(
        "'{name}' is ambiguous: {} definitions. Re-run with a qualified name:\n",
        refs.len()
    );
    for r in &shown {
        out.push_str(&format!("  {}\n", ref_line(r)));
    }
    if omitted > 0 {
        out.push_str(&format!("  +{omitted} more\n"));
    }
    out
}

fn render_not_found(
    info: &RepoInfo,
    name: &str,
    suggestions: &[String],
    opts: OutputOptions,
) -> String {
    if opts.json {
        return json!({
            "repo": info.name,
            "error": "not_found",
            "query": name,
            "suggestions": suggestions,
        })
        .to_string()
            + "\n";
    }
    let mut out = format!("No definition named '{name}' in the {} index.", info.name);
    if !suggestions.is_empty() {
        out.push_str(&format!(" Similar: {}.", suggestions.join(", ")));
    }
    out.push('\n');
    out
}

fn freshness_json(f: &IndexFreshness) -> serde_json::Value {
    serde_json::to_value(f).unwrap_or(serde_json::Value::Null)
}

/// What a walk did not follow (#169), as the `myc` commands report it: the
/// text block and its JSON form, with the command that continues.
struct Incomplete {
    block: String,
    json: serde_json::Value,
}

impl Incomplete {
    fn new(gaps: &Gaps, noun: &str, next: Vec<String>) -> Option<Self> {
        if gaps.is_empty() {
            return None;
        }
        let next = next.join("; then ");
        Some(Incomplete {
            block: gaps.block(noun, &next),
            json: json!({
                "reasons": gaps.reasons(noun),
                "branches_cut": gaps.branches(),
                "next": next,
            }),
        })
    }
}

/// The call resolution statistics of the index, when recorded.
async fn call_stats(graph: &Graph) -> Option<myceliums_core::CallResolutionStats> {
    myceliums_core::CallResolutionStats::load(&graph.store)
        .await
        .ok()
        .flatten()
}

/// `myc analyze <path> --force`: what records what only a full analysis can
/// (#169).
fn full_analysis(info: &RepoInfo) -> String {
    format!("myc analyze {} --force", shell_word(&info.path))
}

/// `name` quoted for a shell command line when it needs it.
fn shell_word(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'))
    {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "'\\''"))
    }
}

/// Renders a callers/callees answer.
#[allow(clippy::too_many_arguments)]
fn render_calls(
    info: &RepoInfo,
    freshness: &IndexFreshness,
    target: &CodeSymbol,
    direction: CallDirection,
    depth: u32,
    hits: Vec<GraphHit>,
    incomplete: Option<Incomplete>,
    opts: OutputOptions,
) -> String {
    let total = hits.len();
    let show_depth = depth > 1;
    let (shown, omitted) = fit_to_budget(hits, opts.budget, |h| hit_line(h, show_depth));
    let label = match direction {
        CallDirection::Callers => "callers",
        CallDirection::Callees => "callees",
    };
    if opts.json {
        return json!({
            "repo": info.name,
            "freshness": freshness_json(freshness),
            "symbol": SymbolRef::of(target),
            "direction": label,
            "max_depth": depth,
            "total": total,
            "omitted": omitted,
            "results": shown,
            "incomplete": incomplete.as_ref().map(|i| &i.json),
        })
        .to_string()
            + "\n";
    }
    let scope = if depth > 1 {
        format!("up to depth {depth}")
    } else {
        "direct".to_string()
    };
    let mut out = format!(
        "{label} of {}: {total} {scope}\n",
        ref_line(&SymbolRef::of(target))
    );
    // Right under the first line: a cut answer never reads as complete.
    if let Some(incomplete) = &incomplete {
        out.push_str(&incomplete.block);
    }
    for h in &shown {
        out.push_str(&hit_line(h, show_depth));
        out.push('\n');
    }
    if omitted > 0 {
        out.push_str(&format!("  +{omitted} more (raise --budget)\n"));
    }
    out.push_str(&freshness.summary());
    out.push('\n');
    out
}

/// `myc callers <symbol>` / `myc callees <symbol>`.
pub async fn cmd_calls(
    symbol: &str,
    direction: CallDirection,
    depth: u32,
    repo: Option<&str>,
    opts: OutputOptions,
) -> Result<()> {
    let graph = load_graph(repo).await?;
    let target = require_symbol(&graph, symbol, opts);
    let depth = depth.max(1);
    let walk = walk_calls_bounded(
        &graph.symbols,
        &graph.relationships,
        target,
        direction,
        depth,
    );
    // What the walk did not follow (#169). The depth limit counts only for
    // a transitive walk: direct callers are the whole answer to "who calls".
    let stats = call_stats(&graph).await;
    let (command, noun) = match direction {
        CallDirection::Callers => ("callers", "callers"),
        CallDirection::Callees => ("callees", "callees"),
    };
    let mut gaps = Gaps::default();
    let mut next = Vec::new();
    let repo_flag = format!("--repo {}", shell_word(&graph.info.id));
    if depth > 1 {
        gaps.depth_cut(depth, walk.cut.clone());
        if gaps.depth.is_some() {
            next.push(format!(
                "myc {command} {} --depth {} {repo_flag}",
                shell_word(symbol),
                depth + 1
            ));
        }
    }
    let walked = std::iter::once(target).chain(
        walk.hits
            .iter()
            .filter(|h| h.depth < depth)
            .map(|h| h.symbol),
    );
    match direction {
        CallDirection::Callers => gaps.unlinked_into(stats.as_ref(), walked),
        CallDirection::Callees => gaps.unlinked_out_of(stats.as_ref(), walked),
    }
    if gaps.needs_full_analysis() {
        next.push(full_analysis(&graph.info));
    }
    if let Some(read) = gaps.read_the_source() {
        next.push(read);
    }
    let incomplete = Incomplete::new(&gaps, noun, next);
    let hits: Vec<GraphHit> = walk.hits.iter().map(Reached::to_hit).collect();
    let freshness = index_freshness(&graph.info);
    print!(
        "{}",
        render_calls(
            &graph.info,
            &freshness,
            target,
            direction,
            depth,
            hits,
            incomplete,
            opts
        )
    );
    Ok(())
}

/// Whether `target` names a file rather than a symbol.
fn looks_like_path(target: &str) -> bool {
    target.contains('/')
        || target.contains('\\')
        || std::path::Path::new(target).extension().is_some_and(|ext| {
            ext.len() <= 5
                && ext
                    .to_string_lossy()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric())
        }) && !target.contains("::")
}

/// `myc impact <symbol|file>`: what depends on a symbol, or on every
/// definition in a file, over incoming call and import edges.
pub async fn cmd_impact_target(
    target: &str,
    depth: u32,
    repo: Option<&str>,
    opts: OutputOptions,
) -> Result<()> {
    let graph = load_graph(repo).await?;
    let depth = depth.max(1);
    let file_roots = if looks_like_path(target) {
        symbols_in_file(&graph.symbols, target)
    } else {
        vec![]
    };
    let (subject, roots) = if !file_roots.is_empty() {
        (format!("file {}", file_roots[0].file_path), file_roots)
    } else if looks_like_path(target)
        && matches!(
            resolve_symbol(&graph.symbols, target),
            SymbolMatch::NotFound
        )
    {
        let msg = if opts.json {
            json!({"repo": graph.info.name, "error": "not_found", "query": target}).to_string()
                + "\n"
        } else {
            format!(
                "No indexed definitions in a file matching '{target}' in the {} index.\n",
                graph.info.name
            )
        };
        finish(&msg, 1)
    } else {
        let sym = require_symbol(&graph, target, opts);
        (ref_line(&SymbolRef::of(sym)), vec![sym])
    };

    let radius = blast_radius(&graph.symbols, &graph.relationships, &roots, depth);
    // What the walk did not follow (#169): the depth limit, and unlinked
    // call sites named like the roots or the dependents short of it.
    let stats = call_stats(&graph).await;
    let walk = dependents_bounded(&graph.symbols, &graph.relationships, &roots, depth);
    let mut gaps = Gaps::default();
    let mut next = Vec::new();
    gaps.depth_cut(depth, walk.cut.clone());
    if gaps.depth.is_some() {
        next.push(format!(
            "myc impact {} --depth {} --repo {}",
            shell_word(target),
            depth + 1,
            shell_word(&graph.info.id)
        ));
    }
    gaps.unlinked_into(
        stats.as_ref(),
        roots.iter().copied().chain(
            walk.hits
                .iter()
                .filter(|h| h.depth < depth)
                .map(|h| h.symbol),
        ),
    );
    if gaps.needs_full_analysis() {
        next.push(full_analysis(&graph.info));
    }
    if let Some(read) = gaps.read_the_source() {
        next.push(read);
    }
    let incomplete = Incomplete::new(&gaps, "dependents", next);
    let freshness = index_freshness(&graph.info);
    let files_line = format!(
        "files ({}): {}",
        radius.files.len(),
        radius.files.join(", ")
    );
    let remaining = opts
        .budget
        .saturating_sub(myceliums_core::navigation::estimate_tokens(&files_line));
    let total = radius.dependents.len();
    let (shown, omitted) = fit_to_budget(radius.dependents, remaining, |h| hit_line(h, true));

    if opts.json {
        print!(
            "{}",
            json!({
                "repo": graph.info.name,
                "freshness": freshness_json(&freshness),
                "target": target,
                "roots": radius.roots,
                "max_depth": depth,
                "total": total,
                "omitted": omitted,
                "dependents": shown,
                "files": radius.files,
                "incomplete": incomplete.as_ref().map(|i| &i.json),
            })
            .to_string()
                + "\n"
        );
        return Ok(());
    }
    let mut out = format!(
        "impact of {subject}, depth <= {depth}: {total} dependents in {} files\n",
        radius.files.len()
    );
    if let Some(incomplete) = &incomplete {
        out.push_str(&incomplete.block);
    }
    for h in &shown {
        out.push_str(&hit_line(h, true));
        out.push('\n');
    }
    if omitted > 0 {
        out.push_str(&format!("  +{omitted} more (raise --budget)\n"));
    }
    out.push_str(&files_line);
    out.push('\n');
    out.push_str(&freshness.summary());
    out.push('\n');
    print!("{out}");
    Ok(())
}

/// `myc context <task text>`: a token-budgeted context pack.
///
/// Ranks with lexical (BM25) search by default: no model load, deterministic,
/// fast. `hybrid` adds vector search (needs an index built with embeddings
/// and loads the embedding model, which takes seconds).
pub async fn cmd_context(
    query: &str,
    repo: Option<&str>,
    hybrid: bool,
    opts: OutputOptions,
) -> Result<()> {
    if query.trim().is_empty() {
        anyhow::bail!(
            "Give the task or question as text, e.g. myc context how are sessions refreshed"
        );
    }
    let graph = load_graph(repo).await?;
    let options = ContextOptions {
        budget_tokens: opts.budget,
        ..ContextOptions::default()
    };
    let pack = if hybrid {
        let embedder = myceliums_core::embedder_for_index(&graph.store).await?;
        // Context packs keep test code where the ranking puts it, like the
        // keyword path: only `myc search` demotes tests (#146).
        let results = myceliums_core::hybrid_search_with(
            &embedder,
            &graph.symbols,
            &*graph.store,
            query,
            options.max_symbols * 2,
            &myceliums_core::HybridParams::with_tests(myceliums_core::TestPolicy::include()),
        )
        .await?;
        let ranked: Vec<&str> = results.iter().map(|r| r.symbol.uid.as_str()).collect();
        build_context_pack_ranked(
            &graph.symbols,
            &graph.relationships,
            query,
            &ranked,
            options,
        )
    } else {
        build_context_pack(&graph.symbols, &graph.relationships, query, options)
    };
    let freshness = index_freshness(&graph.info);
    if opts.json {
        let mut value = serde_json::to_value(&pack)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("repo".into(), json!(graph.info.name));
            obj.insert("freshness".into(), freshness_json(&freshness));
        }
        println!("{value}");
        return Ok(());
    }
    println!(
        "context for \"{}\" ({} symbols, ~{} tokens)",
        myceliums_core::text::truncate_ellipsis(query.trim(), 80),
        pack.symbols.len(),
        pack.estimated_tokens
    );
    print!("{}", pack.render_text());
    println!("{}", freshness.summary());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn info() -> RepoInfo {
        RepoInfo {
            id: "demo-1".into(),
            name: "demo".into(),
            path: "/nonexistent".into(),
            analyzed_at: "2026-11-18T00:00:00Z".into(),
            symbol_count: 0,
            file_count: 0,
            analyzed_commit: Some("abcdef0123".into()),
            vector_geometry_version: 0,
            classification_version: 0,
            call_record_version: 0,
        }
    }

    fn sym(uid: &str, name: &str, file: &str, line: u32) -> CodeSymbol {
        CodeSymbol {
            uid: uid.into(),
            name: name.into(),
            qualified_name: format!("m::{name}"),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: line,
            end_line: line + 1,
            signature: String::new(),
            content: String::new(),
            repo_id: "demo-1".into(),
            metadata: None,
        }
    }

    fn hit(name: &str, depth: u32, via: Option<&str>) -> GraphHit {
        GraphHit {
            symbol: SymbolRef::of(&sym(name, name, "src/x.rs", 3)),
            depth,
            via: via.map(str::to_string),
        }
    }

    fn opts(json: bool, budget: usize) -> OutputOptions {
        OutputOptions { json, budget }
    }

    #[test]
    fn calls_text_lists_location_per_line_and_ends_with_freshness() {
        let fresh = IndexFreshness::compare("t", Some("abcdef0123"), Some("abcdef0123"));
        let out = render_calls(
            &info(),
            &fresh,
            &sym("t", "target", "src/t.rs", 9),
            CallDirection::Callers,
            1,
            vec![hit("a", 1, None), hit("b", 1, None)],
            None,
            opts(false, 2000),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "callers of m::target (Function) src/t.rs:9: 2 direct"
        );
        assert_eq!(lines[1], "  m::a  src/x.rs:3");
        assert_eq!(lines.last().copied(), Some("index: abcdef0 = HEAD"));
    }

    #[test]
    fn calls_text_shows_depth_and_via_for_transitive_walks_and_obeys_budget() {
        let fresh = IndexFreshness::compare("t", None, None);
        let hits: Vec<GraphHit> = (0..50)
            .map(|i| hit(&format!("f{i}"), 2, Some("mid")))
            .collect();
        let out = render_calls(
            &info(),
            &fresh,
            &sym("t", "target", "src/t.rs", 9),
            CallDirection::Callees,
            3,
            hits,
            None,
            opts(false, 60),
        );
        assert!(
            out.starts_with("callees of m::target (Function) src/t.rs:9: 50 up to depth 3\n"),
            "{out}"
        );
        assert!(out.contains("  [2] m::f0  src/x.rs:3  via mid\n"), "{out}");
        assert!(out.contains("more (raise --budget)"), "{out}");
    }

    #[test]
    fn calls_json_has_a_stable_shape() {
        let fresh = IndexFreshness::compare("t", Some("a"), Some("b"));
        let out = render_calls(
            &info(),
            &fresh,
            &sym("t", "target", "src/t.rs", 9),
            CallDirection::Callers,
            1,
            vec![hit("a", 1, None)],
            None,
            opts(true, 2000),
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["incomplete"].is_null());
        assert_eq!(v["direction"], "callers");
        assert_eq!(v["total"], 1);
        assert_eq!(v["omitted"], 0);
        assert_eq!(v["freshness"]["state"], "behind");
        assert_eq!(v["symbol"]["file"], "src/t.rs");
        assert_eq!(v["results"][0]["name"], "a");
        assert_eq!(v["results"][0]["line"], 3);
        assert_eq!(v["results"][0]["depth"], 1);
        assert!(v["results"][0].get("via").is_none());
    }

    #[test]
    fn a_cut_walk_says_so_right_under_the_first_line() {
        let fresh = IndexFreshness::compare("t", Some("abcdef0123"), Some("abcdef0123"));
        let mid = sym("mid", "mid", "src/m.rs", 4);
        let mut gaps = Gaps::default();
        gaps.depth_cut(
            2,
            myceliums_core::navigation::DepthCut {
                frontier: vec![&mid],
                beyond: 3,
            },
        );
        let incomplete = || {
            Incomplete::new(
                &gaps,
                "callers",
                vec!["myc callers target --depth 3".to_string()],
            )
        };
        let out = render_calls(
            &info(),
            &fresh,
            &sym("t", "target", "src/t.rs", 9),
            CallDirection::Callers,
            2,
            vec![hit("a", 1, None), hit("mid", 2, Some("a"))],
            incomplete(),
            opts(false, 2000),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[1..4],
            [
                "Incomplete (3 branch(es) not followed):",
                "  - depth limit 2: 3 more callers one step further, through 1 symbol(s) at depth 2: `mid`",
                "Next: myc callers target --depth 3",
            ]
        );
        assert_eq!(lines[4], "  [1] m::a  src/x.rs:3");
        assert_eq!(lines.last().copied(), Some("index: abcdef0 = HEAD"));
        let json = render_calls(
            &info(),
            &fresh,
            &sym("t", "target", "src/t.rs", 9),
            CallDirection::Callers,
            2,
            vec![],
            incomplete(),
            opts(true, 2000),
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["incomplete"]["branches_cut"], 3);
        assert_eq!(v["incomplete"]["next"], "myc callers target --depth 3");
        assert!(Incomplete::new(&Gaps::default(), "callers", vec![]).is_none());
        assert_eq!(shell_word("Item.AsString"), "Item.AsString");
        assert_eq!(shell_word("a b"), "'a b'");
    }

    #[test]
    fn ambiguous_and_not_found_answers_name_candidates() {
        let refs = vec![
            SymbolRef::of(&sym("1", "parse", "src/a.rs", 5)),
            SymbolRef::of(&sym("2", "parse", "src/b.rs", 7)),
        ];
        let text = render_ambiguous(&info(), "parse", &refs, opts(false, 2000));
        assert!(text.contains("2 definitions"), "{text}");
        assert!(
            text.contains("  m::parse (Function) src/a.rs:5\n"),
            "{text}"
        );
        let v: serde_json::Value =
            serde_json::from_str(&render_ambiguous(&info(), "parse", &refs, opts(true, 2000)))
                .unwrap();
        assert_eq!(v["error"], "ambiguous");
        assert_eq!(v["candidates"][1]["file"], "src/b.rs");

        let nf = render_not_found(&info(), "pars", &["parse".into()], opts(false, 2000));
        assert_eq!(
            nf,
            "No definition named 'pars' in the demo index. Similar: parse.\n"
        );
        let v: serde_json::Value =
            serde_json::from_str(&render_not_found(&info(), "x", &[], opts(true, 10))).unwrap();
        assert_eq!(v["error"], "not_found");
    }

    #[test]
    fn path_detection_distinguishes_files_from_symbols() {
        for p in ["src/a.rs", "a.rs", "pkg/mod.go", "lib\\x.py"] {
            assert!(looks_like_path(p), "{p}");
        }
        for s in [
            "parse_query",
            "Item.AsString",
            "auth::login",
            "Foo.bar_baz_qux",
        ] {
            assert!(!looks_like_path(s), "{s}");
        }
    }
}
