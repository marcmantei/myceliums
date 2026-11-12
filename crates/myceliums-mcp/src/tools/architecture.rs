//! Structural metrics and architecture views: centrality, cycles, coupling, hotspots, lint, drift, ownership.

use super::prelude::*;

#[tool_router(router = architecture_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "find_dead_code",
        description = "Find symbols with no incoming function calls (potential dead code). Exclude common entry points with exclude_patterns. Use before cleanup/refactoring."
    )]
    async fn find_dead_code(
        &self,
        Parameters(params): Parameters<FindDeadCodeParams>,
    ) -> Result<Json<DeadCodeOutput>, rmcp::ErrorData> {
        let symbols = self.symbols(&params.repo_id).await?;

        let relationships = self.relationships(&params.repo_id).await?;

        use myceliums_storage::RelationshipKind;

        // Pre-build set of UIDs that are call targets — O(R) instead of O(S*R)
        let called_uids: std::collections::HashSet<&str> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .map(|r| r.target_uid.as_str())
            .collect();

        let mut dead_code: Vec<DeadCodeItem> = symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    myceliums_storage::SymbolKind::Function | myceliums_storage::SymbolKind::Method
                )
            })
            .filter(|s| !called_uids.contains(s.uid.as_str()))
            .filter(|s| {
                // Filter by exclude_patterns if provided
                if let Some(ref patterns) = params.exclude_patterns {
                    let exclude = patterns.split(',').collect::<Vec<_>>();
                    !exclude
                        .iter()
                        .any(|pattern| s.name.contains(pattern.trim()))
                } else {
                    true
                }
            })
            .map(|s| DeadCodeItem {
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                file_path: s.file_path.clone(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();

        let total = dead_code.len() as u32;
        if let Some(limit) = params.limit {
            dead_code.truncate(limit);
        }

        Ok(Json(DeadCodeOutput {
            symbols: dead_code,
            total_count: total,
        }))
    }

    #[tool(
        name = "get_god_nodes",
        description = "Identify the highest-degree symbols (god nodes) in the call graph. Returns the top-N most connected symbols ranked by total incoming + outgoing CALLS edges. High-coupling nodes (degree > threshold) are flagged as architectural bottlenecks."
    )]
    async fn get_god_nodes(
        &self,
        Parameters(params): Parameters<GetGodNodesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let top_n = params.top_n.unwrap_or(10);
        let coupling_threshold = params.coupling_threshold.unwrap_or(20);
        let god_nodes = compute_god_nodes(&symbols, &relationships, top_n, coupling_threshold);

        let high_coupling_count = god_nodes.iter().filter(|n| n.is_high_coupling).count();
        let total_symbols = symbols.len();

        let nodes: Vec<GodNodeItemOutput> = god_nodes
            .into_iter()
            .map(|n| GodNodeItemOutput {
                name: n.name,
                qualified_name: n.qualified_name,
                kind: n.kind,
                file_path: n.file_path,
                degree: n.degree,
                in_degree: n.in_degree,
                out_degree: n.out_degree,
                is_high_coupling: n.is_high_coupling,
            })
            .collect();

        let output = GodNodesOutput {
            nodes,
            total_symbols,
            high_coupling_count,
        };
        Ok(Json(TextOutput {
            text: format::format_god_nodes(&output),
        }))
    }

    #[tool(
        name = "get_surprising_connections",
        description = "Detect surprising cross-community CALLS edges: connections between symbols in different Leiden communities that rarely interact. Ranked by surprise score (0–1); higher means more isolated/unexpected coupling. Use to find hidden architectural dependencies."
    )]
    async fn get_surprising_connections(
        &self,
        Parameters(params): Parameters<GetSurprisingConnectionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let min_surprise = params.min_surprise_score.unwrap_or(0.1);
        let limit = params.limit.unwrap_or(50);

        let connections =
            compute_surprising_connections(&symbols, &relationships, min_surprise, limit)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let total_cross_community_edges = connections.len();
        let items: Vec<SurprisingConnectionItemOutput> = connections
            .into_iter()
            .map(|c| SurprisingConnectionItemOutput {
                source_name: c.source_name,
                source_qualified_name: c.source_qualified_name,
                target_name: c.target_name,
                target_qualified_name: c.target_qualified_name,
                source_community: c.source_community,
                target_community: c.target_community,
                surprise_score: c.surprise_score,
            })
            .collect();

        let output = SurprisingConnectionsOutput {
            connections: items,
            total_cross_community_edges,
        };
        Ok(Json(TextOutput {
            text: format::format_surprising_connections(&output),
        }))
    }

    #[tool(
        name = "get_git_context",
        description = "Get git ownership and history metadata for a symbol. Returns last author, modification date, commit count, and age in days."
    )]
    async fn get_git_context(
        &self,
        Parameters(params): Parameters<GetGitContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(Some(&params.repo_id))?;

        let symbols = self.symbols(&repo_id).await?;

        let symbol_lower = params.symbol_name.to_lowercase();
        let matching_symbols: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&symbol_lower)
                    || s.qualified_name.to_lowercase().contains(&symbol_lower)
            })
            .collect();

        if matching_symbols.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No symbol found matching '{}'", params.symbol_name),
            }));
        }

        let mut output = String::new();
        for (idx, symbol) in matching_symbols.iter().enumerate() {
            if idx > 0 {
                output.push_str("\n---\n");
            }
            output.push_str(&format!("Symbol: {} ({})\n", symbol.name, symbol.kind));
            output.push_str(&format!("Qualified Name: {}\n", symbol.qualified_name));
            output.push_str(&format!(
                "Location: {}:{}-{}\n",
                symbol.file_path, symbol.start_line, symbol.end_line
            ));

            // Parse and display git metadata if available
            if let Some(metadata_json) = &symbol.metadata {
                if let Ok(metadata) =
                    serde_json::from_str::<myceliums_storage::SymbolMetadata>(metadata_json)
                {
                    if let Some(git_meta) = metadata.git {
                        output.push_str("\nGit Metadata:\n");
                        output.push_str(&format!("  Last Author: {}\n", git_meta.last_author));
                        output.push_str(&format!("  Last Modified: {}\n", git_meta.last_modified));
                        output
                            .push_str(&format!("  Commits Touching: {}\n", git_meta.commit_count));
                        output.push_str(&format!("  Age (days): {}\n", git_meta.age_days));
                        if let Some(hash) = git_meta.last_commit_hash {
                            output.push_str(&format!("  Last Commit: {}\n", hash));
                        }
                    } else {
                        output.push_str("Git Metadata: Not available (symbol may not be in git)\n");
                    }
                }
            }
        }

        Ok(Json(TextOutput { text: output }))
    }

    // ── Cross-repo comparison tools (premium) ────────────────────────────

    #[tool(
        name = "get_centrality_report",
        description = "Compute centrality metrics (degree, betweenness, closeness, eigenvector) for all symbols in the call graph. Returns the top-N symbols ranked by the chosen metric. Betweenness identifies bridge/bottleneck symbols, closeness measures how central a symbol is, eigenvector highlights symbols connected to other important symbols."
    )]
    async fn get_centrality_report(
        &self,
        Parameters(params): Parameters<GetCentralityParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let centrality = compute_centrality(&relationships)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let metric = params.metric.as_deref().unwrap_or("betweenness");
        let top_n = params.top_n.unwrap_or(15);

        let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        let mut nodes: Vec<CentralityNodeOutput> = centrality
            .values()
            .filter_map(|c| {
                uid_to_symbol
                    .get(c.uid.as_str())
                    .map(|sym| CentralityNodeOutput {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        degree: c.degree,
                        betweenness: c.betweenness,
                        closeness: c.closeness,
                        eigenvector: c.eigenvector,
                    })
            })
            .collect();

        // Sort by chosen metric
        match metric {
            "degree" => nodes.sort_by(|a, b| {
                b.degree
                    .partial_cmp(&a.degree)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "closeness" => nodes.sort_by(|a, b| {
                b.closeness
                    .partial_cmp(&a.closeness)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "eigenvector" => nodes.sort_by(|a, b| {
                b.eigenvector
                    .partial_cmp(&a.eigenvector)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            _ => nodes.sort_by(|a, b| {
                b.betweenness
                    .partial_cmp(&a.betweenness)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        }

        let total_nodes = nodes.len();
        nodes.truncate(top_n);

        let output = CentralityOutput {
            nodes,
            total_nodes,
            metric: metric.to_string(),
        };
        Ok(Json(TextOutput {
            text: format::format_centrality(&output),
        }))
    }

    #[tool(
        name = "get_community_metrics",
        description = "Compute quality metrics for the Leiden community partitioning: overall modularity score (higher = better separation), per-community cohesion (internal edge density), and inter-community coupling (edge counts between community pairs). Use to assess code architecture quality."
    )]
    async fn get_community_metrics(
        &self,
        Parameters(params): Parameters<GetCommunityMetricsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let metrics = compute_community_metrics(&symbols, &relationships)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let mut cohesion: Vec<CohesionEntry> = metrics
            .cohesion
            .into_iter()
            .map(|(community, density)| CohesionEntry { community, density })
            .collect();
        cohesion.sort_by(|a, b| {
            b.density
                .partial_cmp(&a.density)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let coupling: Vec<CouplingEntry> = metrics
            .coupling
            .into_iter()
            .map(|c| CouplingEntry {
                community_a: c.community_a,
                community_b: c.community_b,
                edge_count: c.edge_count,
            })
            .collect();

        let output = CommunityMetricsOutput {
            modularity: metrics.modularity,
            community_count: metrics.community_count,
            cohesion,
            coupling,
        };
        Ok(Json(TextOutput {
            text: format::format_community_metrics(&output),
        }))
    }

    #[tool(
        name = "detect_circular_dependencies",
        description = "Detect circular dependencies in the codebase using Tarjan's strongly connected components algorithm. Returns groups of symbols that form dependency cycles. Use to find architectural issues like mutual imports or call cycles."
    )]
    async fn detect_circular_dependencies(
        &self,
        Parameters(params): Parameters<DetectCyclesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let include_calls = params.include_calls.unwrap_or(true);
        let include_imports = params.include_imports.unwrap_or(true);
        let min_cycle_size = params.min_cycle_size.unwrap_or(2);

        let cycles = detect_cycles(
            &symbols,
            &relationships,
            include_calls,
            include_imports,
            min_cycle_size,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let total_count = cycles.len();
        let cycle_items: Vec<CycleItemOutput> = cycles
            .into_iter()
            .map(|c| CycleItemOutput {
                members: c.member_names,
                size: c.size,
                files: c.files,
            })
            .collect();

        let output = CyclesOutput {
            cycles: cycle_items,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_cycles(&output),
        }))
    }

    #[tool(
        name = "get_dependencies",
        description = "Compute file-level dependencies: direct imports, transitive closure (all files reachable via import chains), and reverse dependents (files that import this file). Use before refactoring to understand the full impact of moving or deleting a file."
    )]
    async fn get_dependencies(
        &self,
        Parameters(params): Parameters<GetDependenciesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let dep = compute_file_dependencies(
            &symbols,
            &relationships,
            &params.file_path,
            params.max_depth,
        );

        let output = DependenciesOutput {
            file_path: dep.file_path,
            direct_deps: dep.direct_deps,
            transitive_deps: dep.transitive_deps,
            dependents: dep.dependents,
        };
        Ok(Json(TextOutput {
            text: format::format_dependencies(&output),
        }))
    }

    #[tool(
        name = "get_module_coupling",
        description = "Compute module-level coupling metrics (afferent Ca, efferent Ce, instability I) for all files or directories. Instability ranges from 0 (maximally stable, many dependents) to 1 (maximally unstable, depends on many others). Use to find fragile modules."
    )]
    async fn get_module_coupling(
        &self,
        Parameters(params): Parameters<GetModuleCouplingParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let group_by_dir = params.group_by_directory.unwrap_or(false);
        let mut coupling = compute_module_coupling(&symbols, &relationships, group_by_dir);

        let total_count = coupling.len();
        let limit = params.limit.unwrap_or(30);
        coupling.truncate(limit);

        let modules: Vec<ModuleCouplingEntry> = coupling
            .into_iter()
            .map(|c| ModuleCouplingEntry {
                module_path: c.module_path,
                afferent: c.afferent,
                efferent: c.efferent,
                instability: c.instability,
            })
            .collect();

        let output = ModuleCouplingOutput {
            modules,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_module_coupling(&output),
        }))
    }

    #[tool(
        name = "export_mermaid",
        description = "Export the knowledge graph as a Mermaid diagram. Supports flowchart (call graph), class (class hierarchy), and graph (community-grouped) views. Returns a Mermaid-syntax string that can be rendered by any Mermaid-compatible tool."
    )]
    async fn export_mermaid_tool(
        &self,
        Parameters(params): Parameters<ExportMermaidParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let diagram_type = match params.diagram_type.as_deref() {
            Some("class") => MermaidDiagramType::ClassDiagram,
            Some("graph") => MermaidDiagramType::Graph,
            _ => MermaidDiagramType::Flowchart,
        };

        let mermaid = export_mermaid(&symbols, &relationships, diagram_type);

        Ok(Json(TextOutput { text: mermaid }))
    }

    #[tool(
        name = "quality_hotspots",
        description = "Identify refactoring hotspots by combining graph centrality, git churn, and module instability into a composite score. High-scoring symbols are architecturally critical AND frequently changed — prime candidates for refactoring."
    )]
    async fn quality_hotspots(
        &self,
        Parameters(params): Parameters<QualityHotspotsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let top_n = params.top_n.unwrap_or(20);
        let hotspots = compute_hotspot_scores(&symbols, &relationships, top_n);

        Ok(Json(TextOutput {
            text: format::format_hotspots(&hotspots),
        }))
    }

    #[tool(
        name = "architecture_lint",
        description = "Run architecture quality checks: circular dependencies, god nodes, high fan-out, unstable dependencies. Returns findings with severity levels and affected entities."
    )]
    async fn architecture_lint(
        &self,
        Parameters(params): Parameters<ArchitectureLintParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let rules: Option<Vec<&str>> = params
            .rules
            .as_ref()
            .map(|r| r.split(',').map(|s| s.trim()).collect());
        let threshold = params.god_node_threshold.unwrap_or(20);

        let report = lint_architecture(&symbols, &relationships, rules.as_deref(), threshold);

        Ok(Json(TextOutput {
            text: format::format_lint_results(&report),
        }))
    }

    #[tool(
        name = "architecture_view",
        description = "Generate a service-level architecture diagram from the knowledge graph. Communities become service nodes, cross-community edges become connections. Returns both structured JSON and a Mermaid diagram string."
    )]
    async fn architecture_view(
        &self,
        Parameters(params): Parameters<ArchitectureViewParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let communities = self.communities(&repo_id).await?;

        let diagram = generate_architecture_diagram(&symbols, &relationships, &communities);

        Ok(Json(TextOutput {
            text: format::format_architecture_view(&diagram),
        }))
    }

    #[tool(
        name = "detect_architecture_drift",
        description = "Detect architectural drift by comparing the current knowledge graph against the last saved snapshot. Returns a drift score (0-100, higher = less drift) and details on structural changes."
    )]
    async fn detect_architecture_drift(
        &self,
        Parameters(params): Parameters<DetectDriftParams2>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let baseline = load_snapshot(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "No saved snapshot found. Run analysis first to create a baseline.".to_string(),
                    None,
                )
            })?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let communities = self.communities(&repo_id).await?;

        let report = detect_drift(&baseline, &symbols, &relationships, &communities, &[]);

        Ok(Json(TextOutput {
            text: format::format_drift_report(&report),
        }))
    }

    #[tool(
        name = "get_ownership",
        description = "Resolve file ownership from CODEOWNERS rules. Parses .github/CODEOWNERS or CODEOWNERS and matches symbols to their owners."
    )]
    async fn get_ownership(
        &self,
        Parameters(params): Parameters<GetOwnershipParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo = registry.require(&params.repo_id).map_err(repo_id_error)?;

        let symbols = self.symbols(&params.repo_id).await?;

        let repo_path = std::path::Path::new(&repo.path);
        let codeowners_content = ["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"]
            .iter()
            .find_map(|p| std::fs::read_to_string(repo_path.join(p)).ok())
            .unwrap_or_default();

        let entries = parse_codeowners(&codeowners_content);
        let report = compute_ownership(&symbols, &entries);

        Ok(Json(TextOutput {
            text: format::format_ownership(&report),
        }))
    }
}
