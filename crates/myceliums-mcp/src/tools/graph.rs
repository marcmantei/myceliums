//! Graph navigation: symbol context, callers/callees, call chains, paths,
//! communities, processes, stats, schema.
//!
//! The caller/callee walks go through [`navigation`], so the core tools and
//! their deprecated aliases answer alike.

use super::prelude::*;

/// Callers/callees listed inline by `explain_symbol` before pointing at
/// `find_callers` / `find_callees` for the rest.
const CONTEXT_EDGES: usize = 25;
/// Source lines `explain_symbol` shows by default, and at most.
const SOURCE_LINES: Budget = Budget::new(120, 5000);

#[tool_router(router = graph_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "explain_symbol",
        description = "Show a function, method or class with everything needed to understand it before changing it: its source, signature, file and line range, and who calls it and what it calls (with how each call edge was resolved).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn explain_symbol(
        &self,
        Parameters(params): Parameters<ExplainSymbolParams>,
    ) -> TextResult {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let max_lines = Page::new(params.max_source_lines, None, SOURCE_LINES).limit;
        self.symbol_context_text(&repo_id, &params.symbol_name, max_lines)
            .await
    }

    #[tool(
        name = "symbol_context",
        description = "Get a symbol's full context: source code, callers, and callees. Use this to understand how a function is used before modifying it — reveals dependencies that file reading alone cannot.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn symbol_context(
        &self,
        Parameters(params): Parameters<SymbolContextParams>,
    ) -> TextResult {
        self.symbol_context_text(&params.repo_id, &params.symbol_name, SOURCE_LINES.default)
            .await
    }

    #[tool(
        name = "find_callers",
        description = "Who calls this function or method? Lists the callers of a symbol with file and line; depth > 1 adds the callers' callers. Each caller carries how its CALLS edge was resolved (exact, import, name-match, ambiguous): verify name-match and ambiguous ones.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn find_callers(
        &self,
        Parameters(params): Parameters<FindCallsParams>,
    ) -> Result<Json<CallersOutput>, ToolError> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let list = self
            .call_list(
                &repo_id,
                &params.symbol_name,
                navigation::Direction::Callers,
                params.depth.unwrap_or(1),
                params.limit,
                params.offset,
            )
            .await?;
        Ok(Json(list.into_callers()))
    }

    #[tool(
        name = "find_callees",
        description = "What does this function or method call? Lists the functions a symbol calls with file and line; depth > 1 follows the calls further down. Each callee carries how its CALLS edge was resolved (exact, import, name-match, ambiguous).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn find_callees(
        &self,
        Parameters(params): Parameters<FindCallsParams>,
    ) -> Result<Json<CalleesOutput>, ToolError> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let list = self
            .call_list(
                &repo_id,
                &params.symbol_name,
                navigation::Direction::Callees,
                params.depth.unwrap_or(1),
                params.limit,
                params.offset,
            )
            .await?;
        Ok(Json(list.into_callees()))
    }

    #[tool(
        name = "trace_flow",
        description = "How does control get from one function to another? Follows calls from from_symbol down to to_symbol and lists the shortest call chain plus every function on a chain of that length (extra_calls widens it), with file and line.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn trace_flow(
        &self,
        Parameters(params): Parameters<TraceFlowParams>,
    ) -> TextResult {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let from = navigation::resolve(&symbols, &params.from_symbol)?;
        let to = navigation::resolve(&symbols, &params.to_symbol)?;
        if from.uid == to.uid {
            return Err(ToolError::invalid(
                "from_symbol and to_symbol name the same symbol",
            ));
        }
        let max_depth = params.max_depth.unwrap_or(12).clamp(1, 30);
        let extra = params.extra_calls.unwrap_or(0).min(5);
        let page = Page::new(params.limit, params.offset, Budget::LIST);
        let chains = navigation::call_chains(&symbols, &relationships, from, to, max_depth, extra);
        let body = format::format_call_chains(from, to, chains.as_ref(), max_depth, page);
        self.graph_text(&repo_id, body).await
    }

    #[tool(
        name = "get_processes",
        description = "Get execution flows showing how functions chain together (e.g. request handler → validation → database → response). Use to understand architecture and data flow before refactoring.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_processes(
        &self,
        Parameters(params): Parameters<GetProcessesParams>,
    ) -> TextResult {
        let processes = self.processes(&params.repo_id).await?;

        // Filter first, then page the filtered list.
        let filter = ProcessFilter {
            entry: params.entry,
            filter: params.filter,
            limit: None,
            min_steps: params.min_steps,
        };
        let page =
            Page::new(params.limit, params.offset, Budget::LIST).take(filter.apply(&processes));

        let items: Vec<ProcessItem> = page
            .items
            .into_iter()
            .map(|p| ProcessItem {
                name: p.name,
                entry_point: p.entry_point,
                step_count: p.step_count,
                description: p.description,
            })
            .collect();
        let body = with_notice(format::format_processes(&items), page.notice.as_deref());
        self.graph_text(&params.repo_id, body).await
    }

    #[tool(
        name = "get_communities",
        description = "List all detected communities with summary stats and top symbols. Use to understand code organization and find related modules.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_communities(
        &self,
        Parameters(params): Parameters<GetCommunitiesParams>,
    ) -> Result<Json<CommunitiesOutput>, ToolError> {
        let communities = self.communities(&params.repo_id).await?;
        let index_status = self.index_status(&params.repo_id).await?;

        let total_count = communities.len();
        let page = Page::new(params.limit, params.offset, Budget::LIST)
            .take(communities.iter().collect::<Vec<_>>());
        let items: Vec<CommunityItem> = page
            .items
            .iter()
            .map(|c| CommunityItem {
                uid: c.uid.clone(),
                label: c.label.clone(),
                member_count: c.member_count,
                top_symbols: c.top_symbols.clone(),
                summary: c.summary.clone(),
            })
            .collect();

        Ok(Json(CommunitiesOutput {
            index_status,
            communities: items,
            total_count,
            notice: page.notice,
        }))
    }

    #[tool(
        name = "get_community_detail",
        description = "Get a community's summary, its top member symbols, and its internal/external edge counts. Use to understand community structure before refactoring.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_community_detail(
        &self,
        Parameters(params): Parameters<GetCommunityDetailParams>,
    ) -> Result<Json<CommunityDetailOutput>, ToolError> {
        let communities = self.communities(&params.repo_id).await?;

        let symbols = self.symbols(&params.repo_id).await?;

        let relationships = self.relationships(&params.repo_id).await?;

        let community = communities
            .iter()
            .find(|c| c.uid == params.community_id || c.label == params.community_id)
            .ok_or_else(|| {
                ToolError::recoverable(
                    format!("Community `{}` not found.", params.community_id),
                    "pass a uid or label listed by get_communities",
                )
            })?;

        // top_symbols is a comma-separated list of names (not UIDs)
        let member_names: std::collections::HashSet<String> = community
            .top_symbols
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Match symbols by name — top_symbols only has top ~5 names,
        // so this returns the most important community members
        let community_symbols: Vec<SymbolItem> = symbols
            .iter()
            .filter(|s| member_names.contains(&s.name))
            .map(|s| SymbolItem {
                uid: s.uid.clone(),
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                file_path: s.file_path.clone(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();

        // Build UID set from matched symbols for edge counting
        let member_uids: std::collections::HashSet<String> =
            community_symbols.iter().map(|s| s.uid.clone()).collect();

        // Count internal and external edges
        let mut internal_edges = 0u32;
        let mut external_edges = 0u32;

        for rel in &relationships {
            let src_in = member_uids.contains(&rel.source_uid);
            let tgt_in = member_uids.contains(&rel.target_uid);
            if src_in || tgt_in {
                if src_in && tgt_in {
                    internal_edges += 1;
                } else {
                    external_edges += 1;
                }
            }
        }

        Ok(Json(CommunityDetailOutput {
            index_status: self.index_status(&params.repo_id).await?,
            uid: community.uid.clone(),
            label: community.label.clone(),
            member_count: community.member_count,
            summary: community.summary.clone(),
            symbols: community_symbols,
            internal_edge_count: internal_edges,
            external_edge_count: external_edges,
        }))
    }

    #[tool(
        name = "get_symbol_definition",
        description = "Get the complete definition and source code of a symbol. Use to understand what a function/class does before using or modifying it.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_symbol_definition(
        &self,
        Parameters(params): Parameters<GetSymbolDefinitionParams>,
    ) -> Result<Json<SymbolDefinitionOutput>, ToolError> {
        let symbols = self.symbols(&params.repo_id).await?;
        let symbol = resolve_symbol(&symbols, &params.symbol_name)?;
        let origin = format!(
            "{}:{}-{}",
            symbol.file_path, symbol.start_line, symbol.end_line
        );

        Ok(Json(SymbolDefinitionOutput {
            index_status: self.index_status(&params.repo_id).await?,
            uid: symbol.uid.clone(),
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content: untrusted(&origin, &symbol.content),
        }))
    }

    #[tool(
        name = "get_callers",
        description = "Find all functions that call a given symbol, with optional depth limit. Use to understand impact of changes or find usage patterns. Each caller carries how its CALLS edge was resolved.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn get_callers(
        &self,
        Parameters(params): Parameters<GetCallersParams>,
    ) -> Result<Json<CallersOutput>, ToolError> {
        let list = self
            .call_list(
                &params.repo_id,
                &params.symbol_name,
                navigation::Direction::Callers,
                params.max_depth.unwrap_or(3),
                params.limit,
                params.offset,
            )
            .await?;
        Ok(Json(list.into_callers()))
    }

    #[tool(
        name = "get_callees",
        description = "Find all functions called by a given symbol, with optional depth limit. Use to understand dependencies and call chains. Each callee carries how its CALLS edge was resolved.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_callees(
        &self,
        Parameters(params): Parameters<GetCalleesParams>,
    ) -> Result<Json<CalleesOutput>, ToolError> {
        let list = self
            .call_list(
                &params.repo_id,
                &params.symbol_name,
                navigation::Direction::Callees,
                params.max_depth.unwrap_or(3),
                params.limit,
                params.offset,
            )
            .await?;
        Ok(Json(list.into_callees()))
    }

    #[tool(
        name = "get_file_symbols",
        description = "List all symbols defined in a specific file with kinds and signatures. Use for file-level navigation and understanding file contents.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_file_symbols(
        &self,
        Parameters(params): Parameters<GetFileSymbolsParams>,
    ) -> Result<Json<FileSymbolsOutput>, ToolError> {
        let symbols = self.symbols(&params.repo_id).await?;

        let file_symbols: Vec<FileSymbolItem> = symbols
            .iter()
            .filter(|s| s.file_path == params.file_path)
            .map(|s| FileSymbolItem {
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();
        if file_symbols.is_empty() {
            let files = self.files(&params.repo_id).await?;
            if !files.iter().any(|f| f.path == params.file_path) {
                return Err(ToolError::recoverable(
                    format!("File `{}` is not in the index.", params.file_path),
                    "pass the path relative to the repository root (e.g. src/main.rs), or find the file with search_code",
                ));
            }
        }

        let total_count = file_symbols.len() as u32;
        let page = Page::new(params.limit, params.offset, Budget::new(50, 500)).take(file_symbols);

        Ok(Json(FileSymbolsOutput {
            index_status: self.index_status(&params.repo_id).await?,
            file_path: params.file_path,
            symbols: page.items,
            total_count,
            notice: page.notice,
        }))
    }

    #[tool(
        name = "find_path",
        description = "Find the shortest path between two symbols in the knowledge graph using BFS across all relationship types (CALLS, CONTAINED_BY, IMPORTS, etc.). Answers 'how are these two things connected?' directly. Returns the ordered list of symbols and edge types along the path.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn find_path(&self, Parameters(params): Parameters<FindPathParams>) -> TextResult {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;

        let relationships = self.relationships(&repo_id).await?;

        let from_symbol = resolve_symbol(&symbols, &params.from_symbol)?;
        let to_symbol = resolve_symbol(&symbols, &params.to_symbol)?;

        let max_depth = params.max_depth.unwrap_or(10).clamp(1, 30);

        let sym_by_uid: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
        let labels = crate::repo::CallLabels::new(&symbols);

        // Build bidirectional adjacency list across ALL relationship types
        // Each entry: neighbor_uid -> edge_type_label (heuristic CALLS edges
        // carry their resolution).
        let mut adjacency: HashMap<&str, Vec<(&str, String)>> = HashMap::new();
        for rel in &relationships {
            let mut edge_label = rel.kind.to_string();
            if rel.kind == myceliums_storage::RelationshipKind::Calls {
                if let Some(target) = sym_by_uid.get(rel.target_uid.as_str()) {
                    let resolved = labels.label(rel, &target.name);
                    if crate::repo::CallLabels::is_heuristic(resolved) {
                        edge_label = format!("{edge_label} resolved: {resolved}");
                    }
                }
            }
            adjacency
                .entry(rel.source_uid.as_str())
                .or_default()
                .push((rel.target_uid.as_str(), edge_label.clone()));
            adjacency
                .entry(rel.target_uid.as_str())
                .or_default()
                .push((rel.source_uid.as_str(), format!("{}_REV", edge_label)));
        }

        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        // parent map: child_uid -> (parent_uid, edge_type)
        let mut parent: HashMap<&str, (&str, String)> = HashMap::new();
        let mut queue: std::collections::VecDeque<(&str, u32)> = std::collections::VecDeque::new();

        queue.push_back((from_symbol.uid.as_str(), 0));
        visited.insert(from_symbol.uid.as_str());

        let mut found = false;
        while let Some((uid, depth)) = queue.pop_front() {
            if uid == to_symbol.uid.as_str() {
                found = true;
                break;
            }
            if depth >= max_depth {
                continue;
            }
            if let Some(neighbors) = adjacency.get(uid) {
                for (neighbor_uid, edge_type) in neighbors {
                    if !visited.contains(neighbor_uid) {
                        visited.insert(neighbor_uid);
                        parent.insert(neighbor_uid, (uid, edge_type.clone()));
                        queue.push_back((neighbor_uid, depth + 1));
                    }
                }
            }
        }

        let output = if found {
            // Reconstruct path from to_symbol back to from_symbol
            let mut path_uids: Vec<(&str, String)> = Vec::new();
            let mut current = to_symbol.uid.as_str();
            while current != from_symbol.uid.as_str() {
                if let Some((prev, edge)) = parent.get(current) {
                    path_uids.push((current, edge.clone()));
                    current = prev;
                } else {
                    break;
                }
            }
            path_uids.reverse();

            let mut steps = Vec::new();
            // First step: the from_symbol itself (no incoming edge)
            steps.push(PathStepItem {
                symbol_name: from_symbol.name.clone(),
                qualified_name: from_symbol.qualified_name.clone(),
                kind: from_symbol.kind.to_string(),
                file_path: from_symbol.file_path.clone(),
                start_line: from_symbol.start_line,
                edge_type: String::new(),
            });
            for (uid, edge) in &path_uids {
                if let Some(sym) = sym_by_uid.get(uid) {
                    steps.push(PathStepItem {
                        symbol_name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        edge_type: edge.clone(),
                    });
                }
            }

            let total_depth = path_uids.len() as u32;
            FindPathOutput {
                from_symbol: from_symbol.name.clone(),
                to_symbol: to_symbol.name.clone(),
                steps,
                total_depth,
                found: true,
            }
        } else {
            FindPathOutput {
                from_symbol: from_symbol.name.clone(),
                to_symbol: to_symbol.name.clone(),
                steps: Vec::new(),
                total_depth: 0,
                found: false,
            }
        };

        self.graph_text(&repo_id, format::format_path(&output))
            .await
    }

    #[tool(
        name = "get_stats",
        description = "Get codebase statistics: symbol counts by kind, files, relationships, languages, and communities. Use to understand overall codebase structure.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn get_stats(
        &self,
        Parameters(params): Parameters<GetStatsParams>,
    ) -> Result<Json<StatsOutput>, ToolError> {
        let symbols = self.symbols(&params.repo_id).await?;

        let files = self.files(&params.repo_id).await?;

        let relationships = self.relationships(&params.repo_id).await?;

        let communities = self.communities(&params.repo_id).await?;

        let processes = self.processes(&params.repo_id).await?;

        // Count symbols by kind
        let mut symbol_counts = std::collections::HashMap::new();
        for symbol in &symbols {
            *symbol_counts.entry(symbol.kind.to_string()).or_insert(0u32) += 1;
        }

        // Count relationships by kind
        let mut relationship_counts = std::collections::HashMap::new();
        for rel in &relationships {
            *relationship_counts
                .entry(rel.kind.to_string())
                .or_insert(0u32) += 1;
        }

        // Count files by language
        let mut language_counts = std::collections::HashMap::new();
        for file in &files {
            *language_counts.entry(file.language.clone()).or_insert(0u32) += 1;
        }

        Ok(Json(StatsOutput {
            index_status: self.index_status(&params.repo_id).await?,
            symbol_counts,
            total_symbols: symbols.len() as u32,
            total_files: files.len() as u32,
            total_relationships: relationships.len() as u32,
            relationship_counts,
            language_counts,
            community_count: communities.len() as u32,
            process_count: processes.len() as u32,
        }))
    }

    #[tool(
        name = "get_schema",
        description = "Get property definitions and schema information for entity types or edge types in the ontology. Returns detailed information about what properties are expected for each specified entity type.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_schema(&self, Parameters(params): Parameters<GetSchemaParams>) -> TextResult {
        // Load the ontology from the config directory
        let ontology_dir = std::path::PathBuf::from("config/ontology");
        let ontology = Ontology::load_from_directory(&ontology_dir)
            .map_err(|e| ToolError::internal("cannot load the ontology", e))?;

        let mut output = String::new();
        let include_edges = params.include_edges.unwrap_or(false);

        // Parse the entity types from the comma-separated input
        let entity_types: Vec<&str> = params.entity_types.split(',').map(|s| s.trim()).collect();

        output.push_str("# Ontology Schema\n\n");

        // Display entity schemas
        output.push_str("## Entity Types\n\n");
        for entity_type in &entity_types {
            match ontology.get_entity(entity_type) {
                Some(entity) => {
                    output.push_str(&format!("### {}\n", entity.name));
                    output.push_str(&format!("**Description**: {}\n\n", entity.description));

                    if !entity.tags.is_empty() {
                        output.push_str(&format!("**Tags**: {}\n\n", entity.tags.join(", ")));
                    }

                    output.push_str("**Properties**:\n\n");
                    output.push_str("| Name | Type | Required | Description |\n");
                    output.push_str("|------|------|----------|-------------|\n");

                    for prop in &entity.properties {
                        let required = if prop.required { "Yes" } else { "No" };
                        let desc = prop.description.as_deref().unwrap_or("-");
                        output.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            prop.name, prop.prop_type, required, desc
                        ));
                    }
                    output.push('\n');
                }
                None => {
                    output.push_str(&format!("❌ Unknown entity type: {}\n\n", entity_type));
                }
            }
        }

        // Display edge schemas if requested
        if include_edges {
            output.push_str("## Edge Types\n\n");
            for edge in ontology.get_edges().values() {
                output.push_str(&format!("### {}\n", edge.name));
                output.push_str(&format!("**Description**: {}\n\n", edge.description));
                output.push_str(&format!("**From Types**: {}\n", edge.from_types.join(", ")));
                output.push_str(&format!("**To Types**: {}\n\n", edge.to_types.join(", ")));

                if !edge.tags.is_empty() {
                    output.push_str(&format!("**Tags**: {}\n\n", edge.tags.join(", ")));
                }

                if !edge.properties.is_empty() {
                    output.push_str("**Properties**:\n\n");
                    output.push_str("| Name | Type | Required | Description |\n");
                    output.push_str("|------|------|----------|-------------|\n");

                    for prop in &edge.properties {
                        let required = if prop.required { "Yes" } else { "No" };
                        let desc = prop.description.as_deref().unwrap_or("-");
                        output.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            prop.name, prop.prop_type, required, desc
                        ));
                    }
                    output.push('\n');
                }
            }
        }

        text(output)
    }
}

/// One page of callers or callees, before it becomes the tool's output type.
pub(crate) struct CallList {
    index_status: String,
    symbol_name: String,
    items: Vec<CallerItem>,
    total_count: u32,
    depth_limited: bool,
    notice: Option<String>,
}

impl CallList {
    fn into_callers(self) -> CallersOutput {
        CallersOutput {
            index_status: self.index_status,
            symbol_name: self.symbol_name,
            callers: self.items,
            total_count: self.total_count,
            depth_limited: self.depth_limited,
            notice: self.notice,
        }
    }

    fn into_callees(self) -> CalleesOutput {
        CalleesOutput {
            index_status: self.index_status,
            symbol_name: self.symbol_name,
            callees: self
                .items
                .into_iter()
                .map(|c| CalleeItem {
                    name: c.name,
                    qualified_name: c.qualified_name,
                    kind: c.kind,
                    file_path: c.file_path,
                    start_line: c.start_line,
                    end_line: c.end_line,
                    depth: c.depth,
                    via: c.via,
                    resolved: c.resolved,
                })
                .collect(),
            total_count: self.total_count,
            depth_limited: self.depth_limited,
            notice: self.notice,
        }
    }
}

impl MyceliumsMcp {
    /// Callers or callees of `name`, `depth` hops deep, one page of them.
    pub(crate) async fn call_list(
        &self,
        repo_id: &str,
        name: &str,
        direction: navigation::Direction,
        depth: u32,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CallList, ToolError> {
        let symbols = self.symbols(repo_id).await?;
        let relationships = self.relationships(repo_id).await?;
        let start = navigation::resolve(&symbols, name)?;
        let max_depth = depth.clamp(1, 10);
        let hits = navigation::walk_calls(&symbols, &relationships, start, direction, max_depth);
        let total_count = hits.len() as u32;
        let depth_limited = hits.iter().any(|h| h.depth >= max_depth);
        let page = Page::new(limit, offset, Budget::LIST).take(hits);
        let items = page
            .items
            .iter()
            .map(|h| CallerItem {
                name: h.symbol.name.clone(),
                qualified_name: h.symbol.qualified_name.clone(),
                kind: h.symbol.kind.to_string(),
                file_path: h.symbol.file_path.clone(),
                start_line: h.symbol.start_line,
                end_line: h.symbol.end_line,
                depth: h.depth,
                // The short name, as `myc callers --json` reports it.
                via: h.via.map(|v| v.name.clone()),
                resolved: h.resolved.unwrap_or("exact").to_string(),
            })
            .collect();
        Ok(CallList {
            index_status: self.index_status(repo_id).await?,
            symbol_name: start.name.clone(),
            items,
            total_count,
            depth_limited,
            notice: page.notice,
        })
    }

    /// A symbol's source (at most `max_lines` lines), signature, location,
    /// and direct callers and callees (the first [`CONTEXT_EDGES`] of each).
    async fn symbol_context_text(&self, repo_id: &str, name: &str, max_lines: usize) -> TextResult {
        let symbols = self.symbols(repo_id).await?;
        let relationships = self.relationships(repo_id).await?;
        let symbol = navigation::resolve(&symbols, name)?;
        let edges = |direction| -> Vec<String> {
            navigation::walk_calls(&symbols, &relationships, symbol, direction, 1)
                .iter()
                .map(navigation::Hit::marked_name)
                .collect()
        };
        let callers = edges(navigation::Direction::Callers);
        let callees = edges(navigation::Direction::Callees);
        let (content, source_notice) = source_budget(symbol, max_lines);

        let output = SymbolContextOutput {
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content,
            callers_total: callers.len(),
            callers: callers.into_iter().take(CONTEXT_EDGES).collect(),
            callees_total: callees.len(),
            callees: callees.into_iter().take(CONTEXT_EDGES).collect(),
            metadata: symbol.metadata.clone(),
            source_notice,
        };
        self.graph_text(repo_id, format::format_symbol_context(&output))
            .await
    }
}

/// The first `max_lines` lines of `symbol`'s source and, when that cut
/// anything, a notice naming how to get the rest.
fn source_budget(
    symbol: &myceliums_storage::CodeSymbol,
    max_lines: usize,
) -> (String, Option<String>) {
    let total = symbol.content.lines().count();
    if total <= max_lines {
        return (symbol.content.clone(), None);
    }
    let shown: Vec<&str> = symbol.content.lines().take(max_lines).collect();
    let first = symbol.start_line;
    let notice = format!(
        "Source cut at {max_lines} of {total} lines — call explain_symbol again with max_source_lines={total}, or read {} lines {}-{}.",
        symbol.file_path,
        first.saturating_add(max_lines as u32),
        first.saturating_add(total as u32).saturating_sub(1),
    );
    (shown.join("\n"), Some(notice))
}

#[cfg(test)]
mod tests {
    use super::source_budget;

    #[test]
    fn long_sources_are_cut_with_a_way_to_get_the_rest() {
        let symbol = myceliums_storage::CodeSymbol {
            uid: "u".into(),
            name: "f".into(),
            qualified_name: "f".into(),
            kind: myceliums_storage::SymbolKind::Function,
            file_path: "src/f.rs".into(),
            start_line: 10,
            end_line: 14,
            signature: String::new(),
            content: "l10\nl11\nl12\nl13\nl14".into(),
            repo_id: "r".into(),
            metadata: None,
        };
        let (all, none) = source_budget(&symbol, 5);
        assert_eq!((all.as_str(), none), (symbol.content.as_str(), None));
        let (cut, notice) = source_budget(&symbol, 2);
        assert_eq!(cut, "l10\nl11");
        assert_eq!(
            notice.unwrap(),
            "Source cut at 2 of 5 lines — call explain_symbol again with max_source_lines=5, or read src/f.rs lines 12-14."
        );
    }
}
