//! Cross-domain knowledge: document/email mentions, rationale comments, knowledge gaps.

use super::prelude::*;

#[tool_router(router = knowledge_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "query_knowledge",
        description = "Query cross-domain knowledge: find emails and documents that mention code symbols. Returns source citations with exact line numbers and context snippets. Useful for discovering how code is discussed in documentation and communication channels."
    )]
    async fn query_knowledge(
        &self,
        Parameters(params): Parameters<QueryKnowledgeParams>,
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

        let limit = params.limit.unwrap_or(20).min(100);
        let include_sources = params.include_sources.unwrap_or(true);

        // Step 1: Find matching code symbols using BM25 search
        let matching_symbols = search_symbols(&symbols, &params.query)
            .into_iter()
            .filter_map(|result| symbols.iter().find(|s| s.name == result.symbol.name))
            .collect::<Vec<_>>();

        if matching_symbols.is_empty() {
            let output = KnowledgeQueryOutput {
                query: params.query,
                total_mentions: 0,
                unique_sources: 0,
                results: vec![],
            };
            return Ok(Json(TextOutput {
                text: format::format_knowledge_query(&output),
            }));
        }

        // Step 2: Find all Mentions relationships pointing to matched symbols
        let mut results = Vec::new();
        let mut source_uids = std::collections::HashSet::new();

        for matched_symbol in &matching_symbols {
            for rel in &relationships {
                // Look for Mentions relationships where target is our matched code symbol
                if rel.kind == myceliums_storage::RelationshipKind::Mentions
                    && rel.target_uid == matched_symbol.uid
                {
                    source_uids.insert(rel.source_uid.clone());

                    // Step 3: Find source symbol
                    if let Some(source_symbol) = symbols.iter().find(|s| s.uid == rel.source_uid) {
                        // Filter to content symbols only
                        if !is_content_symbol(&source_symbol.kind) {
                            continue;
                        }

                        // Step 4: Parse metadata and extract matches
                        let matches_metadata = parse_mentions_metadata(&rel.metadata);
                        for match_info in matches_metadata {
                            let result_item = KnowledgeResultItem {
                                source_name: source_symbol.name.clone(),
                                source_kind: source_symbol.kind.to_string(),
                                source_file: source_symbol.file_path.clone(),
                                source_uid: source_symbol.uid.clone(),
                                mentioned_symbol: matched_symbol.name.clone(),
                                mentioned_kind: matched_symbol.kind.to_string(),
                                mentioned_file: matched_symbol.file_path.clone(),
                                mentioned_line: matched_symbol.start_line,
                                mentioned_uid: matched_symbol.uid.clone(),
                                match_context: if include_sources {
                                    match_info.context.clone()
                                } else {
                                    String::new()
                                },
                                match_line: match_info.line,
                                confidence: match_info.confidence,
                            };
                            results.push(result_item);
                        }
                    }
                }
            }
        }

        // Step 5: Sort and limit results
        results.sort_by(|a, b| {
            // Sort by: source_kind (Email first), then source_name, then match_line
            let kind_order = |k: &str| match k {
                "Email" => 0,
                "Document" => 1,
                "Section" => 2,
                _ => 3,
            };
            let kind_cmp = kind_order(&a.source_kind).cmp(&kind_order(&b.source_kind));
            if kind_cmp != std::cmp::Ordering::Equal {
                return kind_cmp;
            }
            let name_cmp = a.source_name.cmp(&b.source_name);
            if name_cmp != std::cmp::Ordering::Equal {
                return name_cmp;
            }
            a.match_line.cmp(&b.match_line)
        });

        let total_mentions = results.len();
        results.truncate(limit);

        let output = KnowledgeQueryOutput {
            query: params.query,
            total_mentions,
            unique_sources: source_uids.len(),
            results,
        };

        Ok(Json(TextOutput {
            text: format::format_knowledge_query(&output),
        }))
    }

    #[tool(
        name = "get_rationale",
        description = "Get design rationale comments (NOTE:, HACK:, WHY:, TODO:, FIXME:, IMPORTANT:) linked to a symbol or file. Use to understand *why* code was written a certain way — surfaces inline reasoning from comments."
    )]
    async fn get_rationale(
        &self,
        Parameters(params): Parameters<GetRationaleParams>,
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

        use myceliums_storage::RelationshipKind;

        // Collect all rationale symbols
        let rationale_syms: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| s.kind == myceliums_storage::SymbolKind::Rationale)
            .collect();

        let uid_to_sym: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build rationale_uid -> target_symbol_name map
        let mut rationale_targets: HashMap<&str, String> = HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::RationaleFor {
                if let Some(target) = uid_to_sym.get(rel.target_uid.as_str()) {
                    rationale_targets.insert(rel.source_uid.as_str(), target.name.clone());
                }
            }
        }

        let items: Vec<RationaleItem> = if let Some(ref symbol_name) = params.symbol_name {
            // Find the target symbol
            let target = symbols
                .iter()
                .find(|s| s.qualified_name == *symbol_name || s.name == *symbol_name)
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", symbol_name),
                        None,
                    )
                })?;

            // Find rationale nodes linked to this symbol via RATIONALE_FOR
            let linked_rationale_uids: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::RationaleFor && r.target_uid == target.uid)
                .map(|r| r.source_uid.as_str())
                .collect();

            rationale_syms
                .iter()
                .filter(|s| linked_rationale_uids.contains(s.uid.as_str()))
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: Some(target.name.clone()),
                })
                .collect()
        } else if let Some(ref file_path) = params.file_path {
            // Return all rationale nodes in a given file
            rationale_syms
                .iter()
                .filter(|s| s.file_path == *file_path)
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: rationale_targets.get(s.uid.as_str()).cloned(),
                })
                .collect()
        } else {
            // Return all rationale nodes in the repo
            rationale_syms
                .iter()
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: rationale_targets.get(s.uid.as_str()).cloned(),
                })
                .collect()
        };

        let total_count = items.len() as u32;
        let output = RationaleOutput {
            rationales: items,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_rationale(&output),
        }))
    }

    #[tool(
        name = "get_knowledge_gaps",
        description = "Detect structural weaknesses in the codebase: untested code (functions with no test callers), isolated modules (communities with few external connections), documentation gaps (files with many symbols but no rationale/doc nodes), and single points of failure (symbols that are the only bridge between communities). Use to prioritize testing, documentation, and refactoring efforts."
    )]
    async fn get_knowledge_gaps(
        &self,
        Parameters(params): Parameters<GetKnowledgeGapsParams>,
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
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        use myceliums_storage::RelationshipKind;

        let category_filter = params.category.as_deref();
        let mut gaps: Vec<KnowledgeGapItem> = Vec::new();

        // Pre-build lookup maps
        let uid_to_sym: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build caller/callee adjacency
        let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push(rel.source_uid.as_str());
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push(rel.target_uid.as_str());
            }
        }

        // --- 1. Untested code: functions/methods with no callers from test files ---
        if category_filter.is_none() || category_filter == Some("untested") {
            let called_uids: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::Calls)
                .map(|r| r.target_uid.as_str())
                .collect();

            for sym in &symbols {
                if !matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Function | myceliums_storage::SymbolKind::Method
                ) {
                    continue;
                }
                // Skip test files themselves and trivial symbols
                if sym.file_path.contains("test") || sym.file_path.contains("spec") {
                    continue;
                }
                if !called_uids.contains(sym.uid.as_str()) {
                    continue; // Already dead code, handled by find_dead_code
                }

                // Check if any caller is from a test file
                let has_test_caller = callers_of
                    .get(sym.uid.as_str())
                    .map(|callers| {
                        callers.iter().any(|caller_uid| {
                            uid_to_sym
                                .get(caller_uid)
                                .map(|s| {
                                    s.file_path.contains("test") || s.file_path.contains("spec")
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);

                if !has_test_caller {
                    let caller_count = callers_of
                        .get(sym.uid.as_str())
                        .map(|c| c.len())
                        .unwrap_or(0);
                    let severity = if caller_count > 5 {
                        "high"
                    } else if caller_count > 2 {
                        "medium"
                    } else {
                        "low"
                    };
                    gaps.push(KnowledgeGapItem {
                        category: "untested".to_string(),
                        severity: severity.to_string(),
                        symbol_name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        description: format!(
                            "No test callers found ({} non-test callers)",
                            caller_count
                        ),
                        suggestion: "Add unit tests covering this symbol".to_string(),
                    });
                }
            }
        }

        // --- 2. Isolated modules: communities with very few external connections ---
        if category_filter.is_none() || category_filter == Some("isolated") {
            // Build community membership from top_symbols
            let mut sym_to_community: HashMap<&str, &str> = HashMap::new();
            for community in &communities {
                for name in community.top_symbols.split(',').map(|s| s.trim()) {
                    if !name.is_empty() {
                        sym_to_community.insert(name, community.label.as_str());
                    }
                }
            }

            for community in &communities {
                if community.member_count < 2 {
                    continue;
                }
                let member_names: std::collections::HashSet<&str> = community
                    .top_symbols
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();

                // Count external edges (calls crossing community boundary)
                let member_uids: std::collections::HashSet<&str> = symbols
                    .iter()
                    .filter(|s| member_names.contains(s.name.as_str()))
                    .map(|s| s.uid.as_str())
                    .collect();

                let external_edge_count = relationships
                    .iter()
                    .filter(|r| r.kind == RelationshipKind::Calls)
                    .filter(|r| {
                        let src_in = member_uids.contains(r.source_uid.as_str());
                        let tgt_in = member_uids.contains(r.target_uid.as_str());
                        (src_in || tgt_in) && !(src_in && tgt_in)
                    })
                    .count();

                if external_edge_count <= 1 {
                    let severity = if external_edge_count == 0 {
                        "high"
                    } else {
                        "medium"
                    };
                    // Report on the community's first top symbol
                    if let Some(first_sym) = symbols
                        .iter()
                        .find(|s| member_names.contains(s.name.as_str()))
                    {
                        gaps.push(KnowledgeGapItem {
                            category: "isolated".to_string(),
                            severity: severity.to_string(),
                            symbol_name: community.label.clone(),
                            qualified_name: format!("community:{}", community.uid),
                            kind: "Community".to_string(),
                            file_path: first_sym.file_path.clone(),
                            start_line: first_sym.start_line,
                            description: format!(
                                "Community '{}' has only {} external connection(s) ({} members)",
                                community.label, external_edge_count, community.member_count
                            ),
                            suggestion: "Review whether this module should integrate with the rest of the codebase or be extracted as a standalone package".to_string(),
                        });
                    }
                }
            }
        }

        // --- 3. Documentation gaps: files with many symbols but no Rationale/Document nodes ---
        if category_filter.is_none() || category_filter == Some("undocumented") {
            // Group symbols by file
            let mut file_symbol_count: HashMap<&str, u32> = HashMap::new();
            let mut file_has_docs: std::collections::HashSet<&str> =
                std::collections::HashSet::new();

            for sym in &symbols {
                if matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Rationale
                        | myceliums_storage::SymbolKind::Document
                        | myceliums_storage::SymbolKind::Section
                ) {
                    file_has_docs.insert(sym.file_path.as_str());
                } else if matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Function
                        | myceliums_storage::SymbolKind::Method
                        | myceliums_storage::SymbolKind::Class
                        | myceliums_storage::SymbolKind::Interface
                ) {
                    *file_symbol_count.entry(sym.file_path.as_str()).or_insert(0) += 1;
                }
            }

            let threshold = 5u32; // Files with >= 5 code symbols and no docs
            for (file_path, count) in &file_symbol_count {
                if *count >= threshold && !file_has_docs.contains(file_path) {
                    let severity = if *count > 15 {
                        "high"
                    } else if *count > 8 {
                        "medium"
                    } else {
                        "low"
                    };
                    gaps.push(KnowledgeGapItem {
                        category: "undocumented".to_string(),
                        severity: severity.to_string(),
                        symbol_name: file_path.to_string(),
                        qualified_name: file_path.to_string(),
                        kind: "File".to_string(),
                        file_path: file_path.to_string(),
                        start_line: 1,
                        description: format!(
                            "{} symbols with no rationale or documentation nodes",
                            count
                        ),
                        suggestion: "Add inline rationale comments (NOTE:, WHY:, HACK:) or documentation to explain design decisions".to_string(),
                    });
                }
            }
        }

        // --- 4. Single points of failure: symbols that are the only bridge between communities ---
        if category_filter.is_none() || category_filter == Some("single_point_of_failure") {
            // Build community membership by UID
            let mut uid_to_community: HashMap<&str, &str> = HashMap::new();
            for community in &communities {
                let member_names: std::collections::HashSet<&str> = community
                    .top_symbols
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                for sym in &symbols {
                    if member_names.contains(sym.name.as_str()) {
                        uid_to_community.insert(sym.uid.as_str(), community.label.as_str());
                    }
                }
            }

            // Find symbols that bridge two communities with only one edge between them
            // Count cross-community edges per community pair
            let mut community_pair_bridges: HashMap<(String, String), Vec<&str>> = HashMap::new();

            for rel in &relationships {
                if rel.kind != RelationshipKind::Calls {
                    continue;
                }
                let src_comm = uid_to_community.get(rel.source_uid.as_str());
                let tgt_comm = uid_to_community.get(rel.target_uid.as_str());
                if let (Some(src_c), Some(tgt_c)) = (src_comm, tgt_comm) {
                    if src_c != tgt_c {
                        let pair = if *src_c < *tgt_c {
                            (src_c.to_string(), tgt_c.to_string())
                        } else {
                            (tgt_c.to_string(), src_c.to_string())
                        };
                        community_pair_bridges
                            .entry(pair)
                            .or_default()
                            .push(rel.source_uid.as_str());
                    }
                }
            }

            // Symbols that are the sole bridge between two communities
            for (pair, bridge_uids) in &community_pair_bridges {
                if bridge_uids.len() == 1 {
                    if let Some(sym) = uid_to_sym.get(bridge_uids[0]) {
                        gaps.push(KnowledgeGapItem {
                            category: "single_point_of_failure".to_string(),
                            severity: "high".to_string(),
                            symbol_name: sym.name.clone(),
                            qualified_name: sym.qualified_name.clone(),
                            kind: sym.kind.to_string(),
                            file_path: sym.file_path.clone(),
                            start_line: sym.start_line,
                            description: format!(
                                "Only bridge between communities '{}' and '{}'",
                                pair.0, pair.1
                            ),
                            suggestion: "Consider adding redundant paths or abstracting the interface to reduce single-point-of-failure risk".to_string(),
                        });
                    }
                }
            }
        }

        // Sort by severity (high first)
        gaps.sort_by(|a, b| {
            let severity_order = |s: &str| -> u8 {
                match s {
                    "high" => 0,
                    "medium" => 1,
                    "low" => 2,
                    _ => 3,
                }
            };
            severity_order(&a.severity)
                .cmp(&severity_order(&b.severity))
                .then_with(|| a.category.cmp(&b.category))
        });

        let summary = KnowledgeGapSummary {
            untested_count: gaps.iter().filter(|g| g.category == "untested").count() as u32,
            isolated_count: gaps.iter().filter(|g| g.category == "isolated").count() as u32,
            undocumented_count: gaps.iter().filter(|g| g.category == "undocumented").count() as u32,
            single_point_of_failure_count: gaps
                .iter()
                .filter(|g| g.category == "single_point_of_failure")
                .count() as u32,
        };
        let total_count = gaps.len() as u32;

        let output = KnowledgeGapsOutput {
            gaps,
            total_count,
            summary,
        };
        Ok(Json(TextOutput {
            text: format::format_knowledge_gaps(&output),
        }))
    }
}

// Helper function to check if a symbol is a content symbol (Email, Document, Section)
fn is_content_symbol(kind: &myceliums_storage::SymbolKind) -> bool {
    matches!(
        kind,
        myceliums_storage::SymbolKind::Email
            | myceliums_storage::SymbolKind::Document
            | myceliums_storage::SymbolKind::Section
    )
}

// Helper struct to represent parsed mention metadata
struct MentionMatch {
    line: u32,
    context: String,
    confidence: f64,
}

// Parse mentions metadata JSON string
fn parse_mentions_metadata(metadata: &str) -> Vec<MentionMatch> {
    if metadata.is_empty() {
        return vec![];
    }

    // Try to parse as JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(metadata) {
        if let Some(matches) = json.get("matches").and_then(|m| m.as_array()) {
            return matches
                .iter()
                .filter_map(|m| {
                    let line = m.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                    let context = m
                        .get("context")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let method = m.get("method").and_then(|m| m.as_str()).unwrap_or("regex");

                    // Set confidence: 1.0 for regex, variable for llm
                    let confidence = match method {
                        "regex" => 1.0,
                        "llm" => 0.9, // Default LLM confidence
                        _ => 0.8,
                    };

                    if line > 0 && !context.is_empty() {
                        Some(MentionMatch {
                            line,
                            context,
                            confidence,
                        })
                    } else {
                        None
                    }
                })
                .collect();
        }
    }

    // Fallback: return empty list if parsing fails
    vec![]
}
