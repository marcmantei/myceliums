//! Graph navigation: symbols, callers/callees, paths, communities, processes, stats, schema.

use super::prelude::*;

#[tool_router(router = graph_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "symbol_context",
        description = "Get a symbol's full context: source code, callers, and callees. Use this to understand how a function is used before modifying it — reveals dependencies that file reading alone cannot."
    )]
    pub(crate) async fn symbol_context(
        &self,
        Parameters(params): Parameters<SymbolContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let context = self
            .service
            .get_symbol_context(&params.repo_id, &params.symbol_name)
            .await
            .map_err(service_error)?;

        let symbol = context.symbol;
        let output = SymbolContextOutput {
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content: symbol.content.clone(),
            callers: context.callers,
            callees: context.callees,
            metadata: symbol.metadata.clone(),
        };
        Ok(Json(TextOutput {
            text: format::format_symbol_context(&output),
        }))
    }

    #[tool(
        name = "get_processes",
        description = "Get execution flows showing how functions chain together (e.g. request handler → validation → database → response). Use to understand architecture and data flow before refactoring."
    )]
    async fn get_processes(
        &self,
        Parameters(params): Parameters<GetProcessesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let processes = store
            .get_processes()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Apply filters
        let filter = ProcessFilter {
            entry: params.entry,
            filter: params.filter,
            limit: params.limit,
            min_steps: params.min_steps,
        };
        let filtered = filter.apply(&processes);

        let items: Vec<ProcessItem> = filtered
            .into_iter()
            .map(|p| ProcessItem {
                name: p.name,
                entry_point: p.entry_point,
                step_count: p.step_count,
                description: p.description,
            })
            .collect();
        Ok(Json(TextOutput {
            text: format::format_processes(&items),
        }))
    }

    #[tool(
        name = "get_communities",
        description = "List all detected communities with summary stats and top symbols. Use to understand code organization and find related modules."
    )]
    async fn get_communities(
        &self,
        Parameters(params): Parameters<GetCommunitiesParams>,
    ) -> Result<Json<CommunitiesOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let items: Vec<CommunityItem> = communities
            .into_iter()
            .map(|c| CommunityItem {
                uid: c.uid,
                label: c.label,
                member_count: c.member_count,
                top_symbols: c.top_symbols,
                summary: c.summary,
            })
            .collect();

        Ok(Json(CommunitiesOutput { communities: items }))
    }

    #[tool(
        name = "get_community_detail",
        description = "Get full details of a community: member symbols, internal relationships, and entry points. Use to understand community structure before refactoring."
    )]
    async fn get_community_detail(
        &self,
        Parameters(params): Parameters<GetCommunityDetailParams>,
    ) -> Result<Json<CommunityDetailOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let community = communities
            .iter()
            .find(|c| c.uid == params.community_id || c.label == params.community_id)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!("Community not found: {}", params.community_id),
                    None,
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
        description = "Get the complete definition and source code of a symbol. Use to understand what a function/class does before using or modifying it."
    )]
    async fn get_symbol_definition(
        &self,
        Parameters(params): Parameters<GetSymbolDefinitionParams>,
    ) -> Result<Json<SymbolDefinitionOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Try qualified name first (exact), then short name
        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();

        let symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        Ok(Json(SymbolDefinitionOutput {
            uid: symbol.uid.clone(),
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content: symbol.content.clone(),
        }))
    }

    #[tool(
        name = "get_callers",
        description = "Find all functions that call a given symbol, with optional depth limit. Use to understand impact of changes or find usage patterns."
    )]
    async fn get_callers(
        &self,
        Parameters(params): Parameters<GetCallersParams>,
    ) -> Result<Json<CallersOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();
        let target_symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        use myceliums_storage::RelationshipKind;
        let max_depth = params.max_depth.unwrap_or(3);

        // Pre-build adjacency: target_uid -> [source_uids] for CALLS edges
        let mut callers_of: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push(rel.source_uid.as_str());
            }
        }

        // Pre-build symbol lookup
        let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // BFS traversal
        let mut callers = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        queue.push_back((target_symbol.uid.as_str(), 0u32));
        visited.insert(target_symbol.uid.as_str());

        while let Some((uid, depth)) = queue.pop_front() {
            if depth > 0 && depth <= max_depth {
                if let Some(sym) = sym_by_uid.get(uid) {
                    callers.push(CallerItem {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        depth,
                    });
                }
            }

            if depth < max_depth {
                if let Some(sources) = callers_of.get(uid) {
                    for &source_uid in sources {
                        if !visited.contains(source_uid) {
                            visited.insert(source_uid);
                            queue.push_back((source_uid, depth + 1));
                        }
                    }
                }
            }
        }

        let total_count = callers.len() as u32;
        let depth_limited = callers.iter().any(|c| c.depth >= max_depth);

        Ok(Json(CallersOutput {
            symbol_name: target_symbol.name.clone(),
            callers,
            total_count,
            depth_limited,
        }))
    }

    #[tool(
        name = "get_callees",
        description = "Find all functions called by a given symbol, with optional depth limit. Use to understand dependencies and call chains."
    )]
    async fn get_callees(
        &self,
        Parameters(params): Parameters<GetCalleesParams>,
    ) -> Result<Json<CalleesOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();
        let source_symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        use myceliums_storage::RelationshipKind;
        let max_depth = params.max_depth.unwrap_or(3);

        // Pre-build adjacency: source_uid -> [target_uids] for CALLS edges
        let mut callees_of: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push(rel.target_uid.as_str());
            }
        }

        let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // BFS traversal
        let mut callees = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        queue.push_back((source_symbol.uid.as_str(), 0u32));
        visited.insert(source_symbol.uid.as_str());

        while let Some((uid, depth)) = queue.pop_front() {
            if depth > 0 && depth <= max_depth {
                if let Some(sym) = sym_by_uid.get(uid) {
                    callees.push(CalleeItem {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        depth,
                    });
                }
            }

            if depth < max_depth {
                if let Some(targets) = callees_of.get(uid) {
                    for &target_uid in targets {
                        if !visited.contains(target_uid) {
                            visited.insert(target_uid);
                            queue.push_back((target_uid, depth + 1));
                        }
                    }
                }
            }
        }

        let total_count = callees.len() as u32;
        let depth_limited = callees.iter().any(|c| c.depth >= max_depth);

        Ok(Json(CalleesOutput {
            symbol_name: source_symbol.name.clone(),
            callees,
            total_count,
            depth_limited,
        }))
    }

    #[tool(
        name = "get_file_symbols",
        description = "List all symbols defined in a specific file with kinds and signatures. Use for file-level navigation and understanding file contents."
    )]
    async fn get_file_symbols(
        &self,
        Parameters(params): Parameters<GetFileSymbolsParams>,
    ) -> Result<Json<FileSymbolsOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

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

        let total_count = file_symbols.len() as u32;

        Ok(Json(FileSymbolsOutput {
            file_path: params.file_path,
            symbols: file_symbols,
            total_count,
        }))
    }

    #[tool(
        name = "find_path",
        description = "Find the shortest path between two symbols in the knowledge graph using BFS across all relationship types (CALLS, CONTAINED_BY, IMPORTS, etc.). Answers 'how are these two things connected?' directly. Returns the ordered list of symbols and edge types along the path."
    )]
    async fn find_path(
        &self,
        Parameters(params): Parameters<FindPathParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = registered_db_path(&repo_id)?;
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Resolve from_symbol
        let from_symbol = Self::resolve_symbol(&symbols, &params.from_symbol)?;
        // Resolve to_symbol
        let to_symbol = Self::resolve_symbol(&symbols, &params.to_symbol)?;

        let max_depth = params.max_depth.unwrap_or(10);

        // Build bidirectional adjacency list across ALL relationship types
        // Each entry: neighbor_uid -> edge_type_label
        let mut adjacency: HashMap<&str, Vec<(&str, String)>> = HashMap::new();
        for rel in &relationships {
            let edge_label = rel.kind.to_string();
            adjacency
                .entry(rel.source_uid.as_str())
                .or_default()
                .push((rel.target_uid.as_str(), edge_label.clone()));
            adjacency
                .entry(rel.target_uid.as_str())
                .or_default()
                .push((rel.source_uid.as_str(), format!("{}_REV", edge_label)));
        }

        // BFS from from_symbol to to_symbol
        let sym_by_uid: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

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

        Ok(Json(TextOutput {
            text: format::format_path(&output),
        }))
    }

    #[tool(
        name = "get_stats",
        description = "Get codebase statistics: symbol counts by kind, files, relationships, languages, and communities. Use to understand overall codebase structure."
    )]
    async fn get_stats(
        &self,
        Parameters(params): Parameters<GetStatsParams>,
    ) -> Result<Json<StatsOutput>, rmcp::ErrorData> {
        let db_path = registered_db_path(&params.repo_id)?;
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let files = store
            .get_files()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let processes = store
            .get_processes()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

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
        description = "Get property definitions and schema information for entity types or edge types in the ontology. Returns detailed information about what properties are expected for each specified entity type."
    )]
    async fn get_schema(
        &self,
        Parameters(params): Parameters<GetSchemaParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        // Load the ontology from the config directory
        let ontology_dir = std::path::PathBuf::from("config/ontology");
        let ontology = Ontology::load_from_directory(&ontology_dir).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to load ontology: {}", e), None)
        })?;

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

        Ok(Json(TextOutput { text: output }))
    }
}
