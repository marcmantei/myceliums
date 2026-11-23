//! Toolsets: which tools a server exposes.
//!
//! Every tool belongs to exactly one [`Toolset`]. A server exposes the
//! [`Toolset::Core`] tools always, plus the groups named on the command line
//! (`myc mcp --toolsets core,architecture`) or in the `mcp.toolsets` config
//! key; `all` names every group, deprecated aliases included.
//!
//! Why a small default: agents pick tools well from a short, task-named list
//! and badly from 55 generic names behind tool search (#99: 0 of 90 sessions
//! called a myceliums tool). So the core tools are named in task language and
//! marked to load up front in Claude Code (`_meta["anthropic/alwaysLoad"]`);
//! the rest is opt-in.
//!
//! [`CATALOG`] is the single source of truth: `tests` check that it names
//! every routed tool exactly once, so a new tool cannot ship unassigned.

use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::Tool;
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// A group of tools that is exposed or hidden as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Toolset {
    /// The default: task-named tools for the common structural questions.
    Core,
    /// More navigation and queries: file outlines, paths, Cypher, flows, communities, stats.
    Graph,
    /// Change review: review summaries, review questions, rename previews.
    Review,
    /// Metrics, cycles, coupling, hotspots, lint, diagrams, drift, ownership, contracts, service names.
    Architecture,
    /// Docs and design knowledge: mentions, rationale comments, knowledge gaps, ADRs.
    Knowledge,
    /// Indexed email: search, threads, people.
    Email,
    /// Comparing and porting features between repositories.
    CrossRepo,
    /// Index maintenance beyond `analyze` (deleting an index) and this
    /// server's local call statistics.
    Admin,
    /// Old tool names kept for one release; each names its replacement.
    Deprecated,
}

impl Toolset {
    /// Every toolset, in display order.
    pub const ALL: [Toolset; 9] = [
        Toolset::Core,
        Toolset::Graph,
        Toolset::Review,
        Toolset::Architecture,
        Toolset::Knowledge,
        Toolset::Email,
        Toolset::CrossRepo,
        Toolset::Admin,
        Toolset::Deprecated,
    ];

    /// The name used on the command line and in the config.
    pub fn name(self) -> &'static str {
        match self {
            Toolset::Core => "core",
            Toolset::Graph => "graph",
            Toolset::Review => "review",
            Toolset::Architecture => "architecture",
            Toolset::Knowledge => "knowledge",
            Toolset::Email => "email",
            Toolset::CrossRepo => "cross_repo",
            Toolset::Admin => "admin",
            Toolset::Deprecated => "deprecated",
        }
    }

    /// One line on what the group holds.
    pub fn summary(self) -> &'static str {
        match self {
            Toolset::Core => "search, callers/callees, impact, symbol context, call chains, dead code, index (always on)",
            Toolset::Graph => "file outlines, paths over any edge, Cypher, execution flows, communities, stats, schema",
            Toolset::Review => "review summaries of a diff, review questions, rename previews",
            Toolset::Architecture => "centrality, cycles, coupling, hotspots, lint, diagrams, drift, ownership, git history, API contracts, service names",
            Toolset::Knowledge => "docs/email mentions of code, rationale comments, knowledge gaps, architecture decision records",
            Toolset::Email => "indexed email: search, threads, people",
            Toolset::CrossRepo => "isolate a feature, compare two repositories, plan a port",
            Toolset::Admin => "delete an index, local tool-call statistics of this server",
            Toolset::Deprecated => "old tool names, kept for one release (each names its replacement)",
        }
    }

    fn from_name(name: &str) -> Option<Toolset> {
        let name = name.trim().to_ascii_lowercase().replace('-', "_");
        Toolset::ALL.into_iter().find(|t| t.name() == name)
    }
}

impl fmt::Display for Toolset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which toolsets a server exposes. Always contains [`Toolset::Core`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolsets(BTreeSet<Toolset>);

impl Toolsets {
    /// The default profile: the core tools only.
    pub fn core() -> Self {
        Toolsets(BTreeSet::from([Toolset::Core]))
    }

    /// Every tool, deprecated aliases included.
    pub fn all() -> Self {
        Toolsets(Toolset::ALL.into_iter().collect())
    }

    /// Core plus `sets`.
    pub fn with(sets: impl IntoIterator<Item = Toolset>) -> Self {
        let mut selected = Self::core();
        selected.0.extend(sets);
        selected
    }

    /// Whether `set` is exposed.
    pub fn contains(&self, set: Toolset) -> bool {
        self.0.contains(&set)
    }

    /// Parses a list of names (`core`, `architecture`, ..., or `all`). An
    /// empty list is the default profile.
    pub fn parse_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut selected = Self::core();
        for name in names.into_iter().map(str::trim).filter(|n| !n.is_empty()) {
            if name.eq_ignore_ascii_case("all") {
                return Ok(Self::all());
            }
            let set = Toolset::from_name(name).ok_or_else(|| {
                format!(
                    "unknown toolset {name:?}; valid: all, {}",
                    Toolset::ALL.map(Toolset::name).join(", ")
                )
            })?;
            selected.0.insert(set);
        }
        Ok(selected)
    }

    /// The names of the exposed toolsets, in display order.
    pub fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|t| t.name()).collect()
    }
}

impl Default for Toolsets {
    fn default() -> Self {
        Self::core()
    }
}

/// Comma-separated names, e.g. `core,architecture` or `all`.
impl FromStr for Toolsets {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_names(s.split(','))
    }
}

impl fmt::Display for Toolsets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.names().join(","))
    }
}

/// Every tool and its toolset. Deprecated aliases are listed in
/// [`DEPRECATED`] with their replacement.
pub(crate) const CATALOG: &[(&str, Toolset)] = &[
    // Core: one tool per common structural question, named the way the
    // question is asked.
    ("search_code", Toolset::Core),
    ("explain_symbol", Toolset::Core),
    ("find_callers", Toolset::Core),
    ("find_callees", Toolset::Core),
    ("impact_of_change", Toolset::Core),
    ("trace_flow", Toolset::Core),
    ("find_dead_code", Toolset::Core),
    ("list_repos", Toolset::Core),
    ("analyze", Toolset::Core),
    // Graph
    ("get_file_symbols", Toolset::Graph),
    ("find_path", Toolset::Graph),
    ("cypher_query", Toolset::Graph),
    ("get_processes", Toolset::Graph),
    ("get_communities", Toolset::Graph),
    ("get_community_detail", Toolset::Graph),
    ("get_stats", Toolset::Graph),
    ("get_schema", Toolset::Graph),
    // Review
    ("get_review_context", Toolset::Review),
    ("get_suggested_questions", Toolset::Review),
    ("rename_symbol", Toolset::Review),
    // Architecture
    ("get_god_nodes", Toolset::Architecture),
    ("get_centrality_report", Toolset::Architecture),
    ("get_community_metrics", Toolset::Architecture),
    ("detect_circular_dependencies", Toolset::Architecture),
    ("get_dependencies", Toolset::Architecture),
    ("get_module_coupling", Toolset::Architecture),
    ("get_surprising_connections", Toolset::Architecture),
    ("quality_hotspots", Toolset::Architecture),
    ("architecture_lint", Toolset::Architecture),
    ("architecture_view", Toolset::Architecture),
    ("export_mermaid", Toolset::Architecture),
    ("detect_architecture_drift", Toolset::Architecture),
    ("get_ownership", Toolset::Architecture),
    ("get_git_context", Toolset::Architecture),
    ("get_contracts", Toolset::Architecture),
    ("map_service", Toolset::Architecture),
    ("get_service_map", Toolset::Architecture),
    // Knowledge
    ("query_knowledge", Toolset::Knowledge),
    ("get_rationale", Toolset::Knowledge),
    ("get_knowledge_gaps", Toolset::Knowledge),
    ("record_decision", Toolset::Knowledge),
    ("get_decisions", Toolset::Knowledge),
    ("link_decision", Toolset::Knowledge),
    // Email
    ("search_emails", Toolset::Email),
    ("get_conversation", Toolset::Email),
    ("get_person_context", Toolset::Email),
    // Cross-repo
    ("isolate_intent", Toolset::CrossRepo),
    ("differentiate_intent", Toolset::CrossRepo),
    ("plan_adaptation", Toolset::CrossRepo),
    // Admin
    ("delete", Toolset::Admin),
    ("server_stats", Toolset::Admin),
];

/// Deprecated aliases and the tool that replaces each. They stay in the
/// [`Toolset::Deprecated`] group (part of `all`) for one release.
pub(crate) const DEPRECATED: &[(&str, &str)] = &[
    ("context_search", "search_code"),
    ("search_documents", "search_code"),
    ("hybrid_search", "search_code"),
    ("semantic_search", "search_code"),
    ("symbol_context", "explain_symbol"),
    ("get_symbol_definition", "explain_symbol"),
    ("get_callers", "find_callers"),
    ("get_callees", "find_callees"),
    ("detect_impact", "impact_of_change"),
    ("get_graph_diff", "detect_architecture_drift"),
    ("snapshot_diff", "detect_architecture_drift"),
];

/// The `_meta` key Claude Code reads to load a tool up front instead of
/// deferring it behind tool search (code.claude.com/docs/en/mcp, "Mark a
/// tool to load upfront or stay deferred").
pub(crate) const ALWAYS_LOAD_META: &str = "anthropic/alwaysLoad";

/// The toolset of `tool`, or `None` for a name the catalog does not know.
pub(crate) fn toolset_of(tool: &str) -> Option<Toolset> {
    if DEPRECATED.iter().any(|(old, _)| *old == tool) {
        return Some(Toolset::Deprecated);
    }
    CATALOG
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, set)| *set)
}

/// `tool` as the `'static` name the catalog spells it with, or `None` for a
/// name this server does not know.
pub(crate) fn static_name(tool: &str) -> Option<&'static str> {
    CATALOG
        .iter()
        .map(|(name, _)| *name)
        .chain(DEPRECATED.iter().map(|(old, _)| *old))
        .find(|name| *name == tool)
}

/// The tool that replaces the deprecated alias `tool`.
pub(crate) fn replacement_of(tool: &str) -> Option<&'static str> {
    DEPRECATED
        .iter()
        .find(|(old, _)| *old == tool)
        .map(|(_, new)| *new)
}

/// Keeps the routes of the selected toolsets and decorates them: core tools
/// are marked to load up front, deprecated aliases name their replacement.
pub(crate) fn select<S>(mut router: ToolRouter<S>, toolsets: &Toolsets) -> ToolRouter<S> {
    router
        .map
        .retain(|name, _| toolset_of(name).is_some_and(|set| toolsets.contains(set)));
    for route in router.map.values_mut() {
        decorate(&mut route.attr);
    }
    router
}

fn decorate(tool: &mut Tool) {
    let mut schema = serde_json::Value::Object((*tool.input_schema).clone());
    compact_schema(&mut schema);
    if let serde_json::Value::Object(schema) = schema {
        tool.input_schema = std::sync::Arc::new(schema);
    }
    match toolset_of(&tool.name) {
        Some(Toolset::Core) => {
            let mut meta = tool.meta.take().unwrap_or_default();
            meta.0
                .insert(ALWAYS_LOAD_META.to_string(), serde_json::Value::Bool(true));
            tool.meta = Some(meta);
        }
        Some(Toolset::Deprecated) => {
            if let Some(new) = replacement_of(&tool.name) {
                let old = tool.description.as_deref().unwrap_or_default();
                tool.description = Some(
                    format!(
                        "DEPRECATED: use `{new}` instead (this alias is removed in the next release). {old}"
                    )
                    .into(),
                );
            }
        }
        _ => {}
    }
}

/// Drops what an agent does not need from a generated input schema (#181).
/// The schema is sent with every model call, so each keyword costs tokens in
/// every turn of a session. Removed: `$schema`, `title`, `format` (the
/// bounds that matter stay as `minimum`/`maximum`), a `minimum` of 0, and
/// the `null` that an `Option` adds to `type` and `enum`: a property outside
/// `required` is optional already, and the handlers accept `null` either
/// way. Property names are never touched, only the keywords of each schema.
pub(crate) fn compact_schema(schema: &mut serde_json::Value) {
    use serde_json::Value;
    let Value::Object(map) = schema else {
        return;
    };
    for keyword in ["$schema", "title", "format"] {
        map.remove(keyword);
    }
    if map.get("minimum").and_then(Value::as_u64) == Some(0) {
        map.remove("minimum");
    }
    if let Some(Value::Array(types)) = map.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            map.insert("type".into(), only);
        }
    }
    if let Some(Value::Array(values)) = map.get_mut("enum") {
        values.retain(|v| !v.is_null());
    }
    for key in ["properties", "$defs", "definitions"] {
        if let Some(Value::Object(children)) = map.get_mut(key) {
            children.values_mut().for_each(compact_schema);
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(children)) = map.get_mut(key) {
            children.iter_mut().for_each(compact_schema);
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = map.get_mut(key) {
            compact_schema(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MyceliumsMcp;

    #[test]
    fn schemas_lose_boilerplate_but_keep_names_bounds_and_required() {
        let mut schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "P",
            "type": "object",
            "required": ["title"],
            "properties": {
                "title": {"type": "string", "description": "a param named title"},
                "format": {"type": ["string", "null"], "enum": ["a", "b", null]},
                "limit": {"type": ["integer", "null"], "format": "uint", "minimum": 1, "maximum": 200},
                "offset": {"type": ["integer", "null"], "format": "uint", "minimum": 0},
                "tags": {"type": ["array", "null"], "items": {"type": "string", "format": "x"}}
            }
        });
        compact_schema(&mut schema);
        assert_eq!(
            schema,
            serde_json::json!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": {"type": "string", "description": "a param named title"},
                    "format": {"type": "string", "enum": ["a", "b"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200},
                    "offset": {"type": "integer"},
                    "tags": {"type": "array", "items": {"type": "string"}}
                }
            })
        );
    }

    #[test]
    fn the_catalog_names_every_routed_tool_exactly_once() {
        let routed: BTreeSet<String> = MyceliumsMcp::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut listed: Vec<&str> = CATALOG.iter().map(|(n, _)| *n).collect();
        listed.extend(DEPRECATED.iter().map(|(old, _)| *old));
        let unique: BTreeSet<String> = listed.iter().map(|n| n.to_string()).collect();
        assert_eq!(unique.len(), listed.len(), "a tool is listed twice");
        assert_eq!(routed, unique, "catalog and router disagree");
        for (old, new) in DEPRECATED {
            assert_eq!(
                toolset_of(new),
                CATALOG.iter().find(|(n, _)| n == new).map(|(_, s)| *s),
                "{old} must point at a live tool, not another alias"
            );
            assert!(toolset_of(new).is_some(), "{old} -> {new}");
        }
    }

    #[test]
    fn core_is_small_and_always_on() {
        let core = CATALOG.iter().filter(|(_, s)| *s == Toolset::Core).count();
        assert!((8..=12).contains(&core), "{core} core tools");
        assert!(Toolsets::default().contains(Toolset::Core));
        let arch: Toolsets = "architecture".parse().unwrap();
        assert!(arch.contains(Toolset::Core) && arch.contains(Toolset::Architecture));
        assert!(!arch.contains(Toolset::Deprecated));
    }

    #[test]
    fn names_parse_leniently_and_unknown_names_are_rejected() {
        let t: Toolsets = " Core, cross-repo ,EMAIL".parse().unwrap();
        assert_eq!(t.names(), vec!["core", "email", "cross_repo"]);
        assert_eq!("".parse::<Toolsets>().unwrap(), Toolsets::core());
        assert_eq!("core,all".parse::<Toolsets>().unwrap(), Toolsets::all());
        let err = "core,archtecture".parse::<Toolsets>().unwrap_err();
        assert!(
            err.contains("\"archtecture\"") && err.contains("architecture"),
            "{err}"
        );
        assert_eq!(Toolsets::all().to_string().split(',').count(), 9);
    }

    #[test]
    fn selection_marks_core_tools_and_deprecated_aliases() {
        let core = select(MyceliumsMcp::tool_router(), &Toolsets::core()).list_all();
        assert!(core
            .iter()
            .all(|t| toolset_of(&t.name) == Some(Toolset::Core)));
        for tool in &core {
            let meta = tool.meta.as_ref().expect("core tools carry _meta");
            assert_eq!(
                meta.0.get(ALWAYS_LOAD_META),
                Some(&serde_json::Value::Bool(true))
            );
        }

        let all = select(MyceliumsMcp::tool_router(), &Toolsets::all()).list_all();
        assert_eq!(all.len(), CATALOG.len() + DEPRECATED.len());
        let alias = all.iter().find(|t| t.name == "get_callers").unwrap();
        assert!(alias
            .description
            .as_deref()
            .unwrap()
            .starts_with("DEPRECATED: use `find_callers` instead"));
        assert!(alias.meta.is_none(), "only core tools load up front");
        let arch = all.iter().find(|t| t.name == "get_god_nodes").unwrap();
        assert!(arch.meta.is_none());
    }
}
