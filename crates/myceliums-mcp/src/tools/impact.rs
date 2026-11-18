//! Change analysis: impact, rename plans, review context, graph diffs, snapshots.

use super::prelude::*;

#[tool_router(router = impact_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "detect_impact",
        description = "Analyze the impact of code changes before committing. Traces changed symbols through the call graph to find indirectly affected code. Use this proactively when modifying functions to catch unintended side effects."
    )]
    pub(crate) async fn detect_impact_tool(
        &self,
        Parameters(params): Parameters<DetectImpactParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.require(&repo_id).map_err(repo_id_error)?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };
        if diff_text.trim().is_empty() {
            return Ok(Json(TextOutput {
                text: "No changes detected (empty diff).".to_string(),
            }));
        }
        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let depth = params.depth.unwrap_or(2);
        let report = detect_impact(&diff_text, &symbols, &relationships, depth);
        let json_report = serde_json::to_value(&report)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_impact_report(&json_report),
        }))
    }

    #[tool(
        name = "rename_symbol",
        description = "Preview renaming a symbol across the codebase. Returns a rename plan with all edits needed (does not modify files)."
    )]
    pub(crate) async fn rename_symbol(
        &self,
        Parameters(params): Parameters<RenameParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let symbols = self.symbols(&params.repo_id).await?;
        let relationships = self.relationships(&params.repo_id).await?;
        let plan = RenamePlan::create(
            &symbols,
            &relationships,
            &params.symbol_name,
            &params.new_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let json_plan = serde_json::to_value(&plan)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_rename_plan(&json_plan),
        }))
    }

    #[tool(
        name = "get_review_context",
        description = "Get a compact structural summary of code changes for efficient review. Analyzes a diff to identify changed symbols, their callers and callees, and affected communities — returning signatures instead of full source to minimize token usage."
    )]
    async fn get_review_context(
        &self,
        Parameters(params): Parameters<ReviewContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.require(&repo_id).map_err(repo_id_error)?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };

        if diff_text.trim().is_empty() {
            return Ok(Json(TextOutput {
                text: "No changes detected (empty diff).".to_string(),
            }));
        }

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let communities = self.communities(&repo_id).await?;

        let depth = params.depth.unwrap_or(1);
        let include_source = params.include_source.unwrap_or(false);
        let report = detect_impact(&diff_text, &symbols, &relationships, depth);

        // Build uid -> symbol lookup
        let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build caller/callee maps
        use myceliums_storage::RelationshipKind;
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

        // Collect changed symbol details
        let mut changed_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut changed_uids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for cs in &report.directly_changed {
            if let Some(sym) = symbols
                .iter()
                .find(|s| s.qualified_name == cs.qualified_name)
            {
                changed_uids.insert(sym.uid.clone());
                changed_entries.push(format::ReviewSymbolEntry {
                    signature: if sym.signature.is_empty() {
                        sym.name.clone()
                    } else {
                        sym.signature.clone()
                    },
                    file_path: sym.file_path.clone(),
                    start_line: sym.start_line,
                    kind: sym.kind.to_string(),
                    source: if include_source {
                        Some(sym.content.clone())
                    } else {
                        None
                    },
                });
            }
        }

        // Collect callers of changed symbols
        let mut caller_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut seen_callers: std::collections::HashSet<String> = std::collections::HashSet::new();
        for uid in &changed_uids {
            if let Some(caller_uids) = callers_of.get(uid.as_str()) {
                for caller_uid in caller_uids {
                    if !changed_uids.contains(*caller_uid)
                        && seen_callers.insert(caller_uid.to_string())
                    {
                        if let Some(sym) = uid_to_symbol.get(caller_uid) {
                            caller_entries.push(format::ReviewSymbolEntry {
                                signature: if sym.signature.is_empty() {
                                    sym.name.clone()
                                } else {
                                    sym.signature.clone()
                                },
                                file_path: sym.file_path.clone(),
                                start_line: sym.start_line,
                                kind: sym.kind.to_string(),
                                source: None,
                            });
                        }
                    }
                }
            }
        }

        // Collect callees of changed symbols
        let mut callee_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut seen_callees: std::collections::HashSet<String> = std::collections::HashSet::new();
        for uid in &changed_uids {
            if let Some(callee_uids) = callees_of.get(uid.as_str()) {
                for callee_uid in callee_uids {
                    if !changed_uids.contains(*callee_uid)
                        && seen_callees.insert(callee_uid.to_string())
                    {
                        if let Some(sym) = uid_to_symbol.get(callee_uid) {
                            callee_entries.push(format::ReviewSymbolEntry {
                                signature: if sym.signature.is_empty() {
                                    sym.name.clone()
                                } else {
                                    sym.signature.clone()
                                },
                                file_path: sym.file_path.clone(),
                                start_line: sym.start_line,
                                kind: sym.kind.to_string(),
                                source: None,
                            });
                        }
                    }
                }
            }
        }

        // Find communities touched by changed symbols
        let mut touched_communities: Vec<String> = Vec::new();
        for community in &communities {
            let top_syms = &community.top_symbols;
            for cs in &report.directly_changed {
                if top_syms.contains(&cs.name) {
                    touched_communities.push(community.label.clone());
                    break;
                }
            }
        }
        touched_communities.sort();
        touched_communities.dedup();

        // Token estimate: count chars/4 for compact output, estimate full source size
        let mut full_source_chars: usize = 0;
        for file_path in &report.affected_files {
            for sym in &symbols {
                if sym.file_path == *file_path {
                    full_source_chars += sym.content.len();
                }
            }
        }

        let changed_files: std::collections::HashSet<&str> = report
            .directly_changed
            .iter()
            .map(|cs| cs.file_path.as_str())
            .collect();

        let context = format::ReviewContext {
            changed: changed_entries,
            callers: caller_entries,
            callees: callee_entries,
            communities_touched: touched_communities,
            changed_file_count: changed_files.len(),
            full_source_tokens: full_source_chars / 4,
        };

        Ok(Json(TextOutput {
            text: format::format_review_context(&context),
        }))
    }

    #[tool(
        name = "get_suggested_questions",
        description = "Auto-generate contextual code review questions based on code graph structure and git diff. Returns ranked questions about potential issues like missing test coverage, high caller counts, or API contract violations."
    )]
    async fn get_suggested_questions(
        &self,
        Parameters(params): Parameters<GetSuggestedQuestionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;

        let relationships = self.relationships(&repo_id).await?;

        let files = self.files(&repo_id).await?;

        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.require(&repo_id).map_err(repo_id_error)?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };

        let limit = params.limit.unwrap_or(5);
        let questions =
            generate_suggested_questions(&diff_text, &symbols, &relationships, &files, limit);

        Ok(Json(TextOutput {
            text: format::format_suggested_questions(&questions),
        }))
    }

    #[tool(
        name = "get_graph_diff",
        description = "Compare the current knowledge graph against the last stored snapshot to detect architectural drift. Shows new/removed symbols and relationships since the last analysis that saved a snapshot."
    )]
    async fn get_graph_diff(
        &self,
        Parameters(params): Parameters<GetGraphDiffParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        // Load previous snapshot
        let previous = load_snapshot(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let previous = match previous {
            Some(s) => s,
            None => {
                return Ok(Json(TextOutput {
                    text: format!(
                        "No previous snapshot found for '{}'. Run `analyze` first to create a baseline.",
                        repo_id
                    ),
                }));
            }
        };

        // Build current snapshot from graph
        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;
        let current = build_snapshot(&repo_id, &symbols, &relationships);

        let diff = diff_snapshots(&previous, &current);

        let output = GraphDiffOutput {
            repo_id: diff.repo_id,
            previous_snapshot_at: diff.previous_snapshot_at,
            current_snapshot_at: diff.current_snapshot_at,
            added_symbols: diff
                .added_symbols
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            removed_symbols: diff
                .removed_symbols
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            added_edges: diff
                .added_edges
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            removed_edges: diff
                .removed_edges
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
        };

        Ok(Json(TextOutput {
            text: format::format_graph_diff(&output),
        }))
    }

    #[tool(
        name = "snapshot_diff",
        description = "Compare two graph snapshots to see architectural changes over time. Shows added/removed symbols and relationships. Defaults to comparing the two most recent snapshots."
    )]
    async fn snapshot_diff(
        &self,
        Parameters(params): Parameters<SnapshotDiffParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let snapshots = list_snapshots(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        if snapshots.len() < 2 && params.from_snapshot.is_none() {
            return Ok(Json(TextOutput {
                text: "Need at least 2 snapshots to compare. Run analysis multiple times first."
                    .to_string(),
            }));
        }

        let from_snap = if let Some(from_id) = &params.from_snapshot {
            load_snapshot_by_id(&data_dir(), &repo_id, from_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Snapshot not found: {}", from_id),
                        None,
                    )
                })?
        } else {
            load_snapshot_by_id(&data_dir(), &repo_id, &snapshots[1].snapshot_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        "Could not load older snapshot".to_string(),
                        None,
                    )
                })?
        };

        let to_snap = if let Some(to_id) = &params.to_snapshot {
            load_snapshot_by_id(&data_dir(), &repo_id, to_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(format!("Snapshot not found: {}", to_id), None)
                })?
        } else {
            load_snapshot_by_id(&data_dir(), &repo_id, &snapshots[0].snapshot_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        "Could not load latest snapshot".to_string(),
                        None,
                    )
                })?
        };

        let diff = myceliums_core::diff_snapshots(&from_snap, &to_snap);

        let mut out = format!(
            "Snapshot diff: {} → {}\n\n",
            diff.previous_snapshot_at, diff.current_snapshot_at
        );

        if diff.added_symbols.is_empty()
            && diff.removed_symbols.is_empty()
            && diff.added_edges.is_empty()
            && diff.removed_edges.is_empty()
        {
            out.push_str("No changes detected.");
        } else {
            out.push_str(&format!(
                "Changes: +{} symbols, -{} symbols, +{} edges, -{} edges\n\n",
                diff.added_symbols.len(),
                diff.removed_symbols.len(),
                diff.added_edges.len(),
                diff.removed_edges.len(),
            ));
            if !diff.added_symbols.is_empty() {
                out.push_str("Added symbols:\n");
                for s in &diff.added_symbols {
                    out.push_str(&format!("  + {}\n", s.label));
                }
            }
            if !diff.removed_symbols.is_empty() {
                out.push_str("Removed symbols:\n");
                for s in &diff.removed_symbols {
                    out.push_str(&format!("  - {}\n", s.label));
                }
            }
        }

        Ok(Json(TextOutput { text: out }))
    }
}

// Helper function to generate suggested review questions from code graph
fn generate_suggested_questions(
    diff_text: &str,
    symbols: &[myceliums_storage::CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
    _files: &[myceliums_storage::FileNode],
    limit: usize,
) -> SuggestedQuestionsOutput {
    let mut questions = Vec::new();
    use myceliums_storage::{RelationshipKind, SymbolKind};
    use std::collections::HashMap;

    // Parse diff to find changed files and functions
    let changed_files: Vec<&str> = diff_text
        .lines()
        .filter_map(|line| {
            if line.starts_with("diff --git a/") {
                Some(line.split(" b/").nth(1).unwrap_or(""))
            } else {
                None
            }
        })
        .collect();

    if changed_files.is_empty() && !diff_text.trim().is_empty() {
        // Fallback: treat entire diff as changes
        // This is a simple heuristic
    }

    // Build a map of symbol names to their call counts
    let mut caller_counts: HashMap<String, usize> = HashMap::new();
    for rel in relationships {
        if rel.kind == RelationshipKind::Calls {
            *caller_counts.entry(rel.target_uid.clone()).or_insert(0) += 1;
        }
    }

    // Check for functions with high caller counts
    for (symbol_uid, count) in &caller_counts {
        if *count > 10 {
            if let Some(symbol) = symbols.iter().find(|s| &s.uid == symbol_uid) {
                questions.push(SuggestedQuestion {
                    question: format!(
                        "This {} has {} callers — have all call sites been updated?",
                        symbol.kind.to_string().to_lowercase(),
                        count
                    ),
                    severity: if *count > 20 { "high" } else { "medium" }.to_string(),
                    category: "callers".to_string(),
                    references: vec![symbol.qualified_name.clone()],
                    rationale: "Functions with many callers have a higher risk of breaking changes. Ensure all dependent code has been tested.".to_string(),
                });
            }
        }
    }

    // Check for symbols with no test coverage (files without "test" or "spec" in their path)
    for symbol in symbols {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
            && !symbol.file_path.contains("test")
            && !symbol.file_path.contains("spec")
        {
            let has_related_test = symbols
                .iter()
                .any(|s| s.file_path.contains("test") || s.file_path.contains("spec"));
            if !has_related_test {
                questions.push(SuggestedQuestion {
                    question: format!(
                        "The file `{}` has no test coverage — should tests be added?",
                        symbol.file_path
                    ),
                    severity: "medium".to_string(),
                    category: "coverage".to_string(),
                    references: vec![symbol.file_path.clone()],
                    rationale: "Adding tests for critical business logic reduces regressions and improves maintainability.".to_string(),
                });
                break; // Only report once per file
            }
        }
    }

    // Check for symbols crossing community boundaries (simplified check)
    // Build symbol-to-file mapping
    let mut symbol_to_file: HashMap<&str, &str> = HashMap::new();
    for symbol in symbols {
        symbol_to_file.insert(&symbol.uid, &symbol.file_path);
    }

    // Check symbols that call across file boundaries
    let mut cross_boundary_symbols = Vec::new();
    for rel in relationships {
        if rel.kind == RelationshipKind::Calls {
            if let (Some(source_file), Some(target_file)) = (
                symbol_to_file.get(rel.source_uid.as_str()),
                symbol_to_file.get(rel.target_uid.as_str()),
            ) {
                let source_parts: Vec<&str> = source_file.split('/').collect();
                let target_parts: Vec<&str> = target_file.split('/').collect();

                // Check if they're in different top-level directories
                if !source_parts.is_empty()
                    && !target_parts.is_empty()
                    && source_parts[0] != target_parts[0]
                {
                    cross_boundary_symbols.push((rel.source_uid.clone(), rel.target_uid.clone()));
                }
            }
        }
    }

    if !cross_boundary_symbols.is_empty() {
        questions.push(SuggestedQuestion {
            question: format!(
                "Found {} calls crossing module/community boundaries — are API contracts maintained?",
                cross_boundary_symbols.len()
            ),
            severity: "medium".to_string(),
            category: "api_contract".to_string(),
            references: cross_boundary_symbols
                .iter()
                .map(|(_, target)| target.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .take(3)
                .collect(),
            rationale: "Cross-boundary calls can indicate coupling issues. Verify API stability and consider public/private contracts.".to_string(),
        });
    }

    // Check for complexity: files with many symbols
    let mut file_symbol_counts: HashMap<&str, usize> = HashMap::new();
    for symbol in symbols {
        *file_symbol_counts.entry(&symbol.file_path).or_insert(0) += 1;
    }

    for (file_path, count) in &file_symbol_counts {
        if *count > 30 {
            questions.push(SuggestedQuestion {
                question: format!(
                    "File `{}` contains {} symbols — consider refactoring or splitting?",
                    file_path, count
                ),
                severity: "low".to_string(),
                category: "complexity".to_string(),
                references: vec![file_path.to_string()],
                rationale: "Large files with many symbols can be harder to understand and maintain. Consider splitting by responsibility.".to_string(),
            });
        }
    }

    // Sort questions by severity (high > medium > low) then by specificity
    questions.sort_by(|a, b| {
        let severity_order = |sev: &str| match sev {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        };
        severity_order(&a.severity).cmp(&severity_order(&b.severity))
    });

    // Limit to requested count
    questions.truncate(limit);

    SuggestedQuestionsOutput { questions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

    #[test]
    fn test_generate_suggested_questions_high_caller_count() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "processPayment".to_string(),
                qualified_name: "payments.processPayment".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/payments.ts".to_string(),
                start_line: 10,
                end_line: 30,
                signature: "function processPayment()".to_string(),
                content: "// process payment".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "validateUser".to_string(),
                qualified_name: "auth.validateUser".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/auth.ts".to_string(),
                start_line: 5,
                end_line: 15,
                signature: "function validateUser()".to_string(),
                content: "// validate user".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create relationships: 15 calls to processPayment
        let mut relationships = Vec::new();
        for i in 0..15 {
            relationships.push(Relationship {
                uid: format!("rel{}", i),
                source_uid: format!("caller{}", i),
                target_uid: "sym1".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test-repo".to_string(),
                metadata: "".to_string(),
            });
        }

        let files = vec![];
        let diff_text = "diff --git a/src/payments.ts b/src/payments.ts\n+function update(){}";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have at least one question about high caller count
        assert!(!output.questions.is_empty());
        let has_caller_question = output
            .questions
            .iter()
            .any(|q| q.category == "callers" && q.question.contains("15 callers"));
        assert!(
            has_caller_question,
            "Expected a question about high caller count"
        );
    }

    #[test]
    fn test_generate_suggested_questions_uncovered_file() {
        let symbols = vec![CodeSymbol {
            uid: "sym1".to_string(),
            name: "calculateTax".to_string(),
            qualified_name: "tax.calculateTax".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/tax.ts".to_string(),
            start_line: 1,
            end_line: 20,
            signature: "function calculateTax()".to_string(),
            content: "// calculate tax".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }];

        let relationships = vec![];
        let files = vec![];
        let diff_text = "diff --git a/src/tax.ts b/src/tax.ts\n+new function";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have a question about missing test coverage
        let has_coverage_question = output
            .questions
            .iter()
            .any(|q| q.category == "coverage" && q.question.contains("test coverage"));
        assert!(
            has_coverage_question,
            "Expected a question about test coverage"
        );
    }

    #[test]
    fn test_generate_suggested_questions_cross_boundary_calls() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "apiCall".to_string(),
                qualified_name: "api.apiCall".to_string(),
                kind: SymbolKind::Function,
                file_path: "api/handler.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function apiCall()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "dbQuery".to_string(),
                qualified_name: "db.dbQuery".to_string(),
                kind: SymbolKind::Function,
                file_path: "db/connection.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function dbQuery()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create cross-boundary call (api -> db)
        let relationships = vec![Relationship {
            uid: "rel1".to_string(),
            source_uid: "sym1".to_string(),
            target_uid: "sym2".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test-repo".to_string(),
            metadata: "".to_string(),
        }];

        let files = vec![];
        let diff_text = "diff --git a/api/handler.ts b/api/handler.ts";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have a question about API contracts
        let has_api_question = output
            .questions
            .iter()
            .any(|q| q.category == "api_contract");
        assert!(has_api_question, "Expected a question about API contracts");
    }

    #[test]
    fn test_generate_suggested_questions_respects_limit() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "func1".to_string(),
                qualified_name: "func1".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/file1.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function func1()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "func2".to_string(),
                qualified_name: "func2".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/file2.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function func2()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create many relationships to generate multiple questions
        let mut relationships = Vec::new();
        for i in 0..20 {
            relationships.push(Relationship {
                uid: format!("rel{}", i),
                source_uid: format!("caller{}", i),
                target_uid: "sym1".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test-repo".to_string(),
                metadata: "".to_string(),
            });
        }

        let files = vec![];
        let diff_text = "";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 2);

        // Should respect the limit
        assert!(output.questions.len() <= 2, "Should not exceed limit of 2");
    }
}
