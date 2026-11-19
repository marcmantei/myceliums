//! Change analysis: impact of a change (symbol, file or diff), rename plans,
//! review context, review questions; the deprecated graph-diff aliases.

use super::prelude::*;

/// The diff to analyze: the caller's, or `git diff HEAD` in the repo.
fn diff_or_git(diff: Option<String>, repo_id: &str) -> Result<String, ToolError> {
    if let Some(diff) = diff {
        return Ok(diff);
    }
    let repo = repo_info(repo_id)?;
    run_git_diff(&repo.path).map_err(|e| {
        ToolError::recoverable(
            format!("Cannot run `git diff HEAD` in {}: {e}", repo.path),
            "pass the unified diff as `diff`",
        )
    })
}

#[tool_router(router = impact_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "impact_of_change",
        description = "What could break if I change this? For a symbol or a file: every function that calls or imports it, directly and transitively, and the files to check; direct dependents must change too if a signature changes. With neither: the impact of the uncommitted diff (git diff HEAD), or of the diff you pass.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn impact_of_change(
        &self,
        Parameters(params): Parameters<ImpactOfChangeParams>,
    ) -> TextResult {
        let given = [
            params.symbol_name.is_some(),
            params.file_path.is_some(),
            params.diff.is_some(),
        ];
        if given.iter().filter(|g| **g).count() > 1 {
            return Err(ToolError::invalid(
                "give at most one of symbol_name, file_path and diff",
            ));
        }
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let repo_id = graph.repo_id().to_string();
        let depth = params.depth.unwrap_or(2).clamp(1, 10);
        let page = Page::new(params.limit, params.offset, Budget::new(50, 500));

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;
        let (subject, roots) = match (params.symbol_name, params.file_path) {
            (Some(name), _) => {
                let s = navigation::resolve(&symbols, &name)?;
                let subject = format!(
                    "{} ({}, {}:{}-{})",
                    s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line
                );
                (subject, vec![s])
            }
            (None, Some(path)) => {
                // Core's file matching (exact path, else a unique suffix), as
                // `myc impact <file>` uses it.
                let roots = navigation::symbols_in_file(&symbols, &path);
                let path = match roots.first() {
                    Some(s) => s.file_path.clone(),
                    None => {
                        let path = path.trim().trim_start_matches("./").to_string();
                        let files = graph.files().await?;
                        if !files.iter().any(|f| f.path == path) {
                            return Err(ToolError::recoverable(
                                format!("File `{path}` is not in the index."),
                                "pass the path relative to the repository root (e.g. src/main.rs), or find the code with search_code",
                            ));
                        }
                        path
                    }
                };
                (format!("{path} ({} definitions)", roots.len()), roots)
            }
            (None, None) => {
                let diff_text = diff_or_git(params.diff, &repo_id)?;
                let body = if diff_text.trim().is_empty() {
                    "No changes detected (empty diff).".to_string()
                } else {
                    let report = detect_impact(&diff_text, &symbols, &relationships, depth);
                    format::format_impact_report(&report, page)
                };
                return self.graph_text(&graph, body).await;
            }
        };
        let hits = navigation::dependents(&symbols, &relationships, &roots, depth);
        let body = format::format_dependents(&subject, &hits, depth, page);
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "detect_impact",
        description = "Analyze the impact of code changes before committing. Traces changed symbols through the call graph to find indirectly affected code. Use this proactively when modifying functions to catch unintended side effects.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn detect_impact_tool(
        &self,
        Parameters(params): Parameters<DetectImpactParams>,
    ) -> TextResult {
        self.impact_of_change(Parameters(ImpactOfChangeParams {
            diff: params.diff,
            repo_id: params.repo_id,
            depth: params.depth,
            limit: params.limit,
            offset: params.offset,
            ..Default::default()
        }))
        .await
    }

    #[tool(
        name = "rename_symbol",
        description = "Preview renaming a symbol across the codebase. Returns a rename plan with all edits needed (does not modify files).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn rename_symbol(
        &self,
        Parameters(params): Parameters<RenameParams>,
    ) -> TextResult {
        require_query("new_name", &params.new_name)?;
        let graph = self.graph(&params.repo_id).await?;
        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;
        let target = resolve_symbol(&symbols, &params.symbol_name)?;
        let plan = RenamePlan::for_symbol(&symbols, &relationships, target, &params.new_name)
            .map_err(|e| ToolError::internal("cannot build the rename plan", e))?;
        let page = Page::new(params.limit, params.offset, Budget::new(100, 500));
        let body = format::format_rename_plan(&plan, page);
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "get_review_context",
        description = "Get a compact structural summary of code changes for efficient review. Analyzes a diff to identify changed symbols, their callers and callees, and affected communities — returning signatures instead of full source to minimize token usage.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_review_context(
        &self,
        Parameters(params): Parameters<ReviewContextParams>,
    ) -> TextResult {
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let repo_id = graph.repo_id().to_string();
        let diff_text = diff_or_git(params.diff, &repo_id)?;

        if diff_text.trim().is_empty() {
            return self
                .graph_text(&graph, "No changes detected (empty diff).".to_string())
                .await;
        }

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;
        let communities = graph.communities().await?;

        let depth = params.depth.unwrap_or(1).clamp(1, 10);
        let include_source = params.include_source.unwrap_or(false);
        let limit = Page::new(params.limit, None, Budget::new(50, 200)).limit;
        let report = detect_impact(&diff_text, &symbols, &relationships, depth);

        // Build uid -> symbol lookup
        let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
        let labels = crate::repo::CallLabels::new(&symbols);

        // Build caller/callee maps (with the CALLS edge, for its resolution)
        use myceliums_storage::RelationshipKind;
        let mut callers_of: HashMap<&str, Vec<(&str, &myceliums_storage::Relationship)>> =
            HashMap::new();
        let mut callees_of: HashMap<&str, Vec<(&str, &myceliums_storage::Relationship)>> =
            HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push((rel.source_uid.as_str(), rel));
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push((rel.target_uid.as_str(), rel));
            }
        }

        let entry = |sym: &myceliums_storage::CodeSymbol, source: Option<String>| {
            format::ReviewSymbolEntry {
                signature: if sym.signature.is_empty() {
                    sym.name.clone()
                } else {
                    sym.signature.clone()
                },
                file_path: sym.file_path.clone(),
                start_line: sym.start_line,
                kind: sym.kind.to_string(),
                source,
                resolved: None,
            }
        };

        // Collect changed symbol details
        let mut changed_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        // Ordered, so the callers/callees lists (and what a limit cuts) are
        // the same on every call.
        let mut changed_uids: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();

        for cs in &report.directly_changed {
            if let Some(sym) = symbols
                .iter()
                .find(|s| s.qualified_name == cs.qualified_name)
            {
                changed_uids.insert(sym.uid.clone());
                changed_entries.push(entry(sym, include_source.then(|| sym.content.clone())));
            }
        }

        // Callers / callees of changed symbols, each once, with the
        // resolution of the edge that links them.
        let neighbours = |links: &HashMap<&str, Vec<(&str, &myceliums_storage::Relationship)>>,
                          target_is_neighbour: bool| {
            let mut entries: Vec<format::ReviewSymbolEntry> = Vec::new();
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for uid in &changed_uids {
                for &(other, rel) in links.get(uid.as_str()).map(Vec::as_slice).unwrap_or(&[]) {
                    if changed_uids.contains(other) || !seen.insert(other) {
                        continue;
                    }
                    if let Some(sym) = uid_to_symbol.get(other) {
                        let target_name = if target_is_neighbour {
                            sym.name.as_str()
                        } else {
                            uid_to_symbol
                                .get(uid.as_str())
                                .map(|s| s.name.as_str())
                                .unwrap_or("")
                        };
                        let label = labels.label(rel, target_name);
                        let mut e = entry(sym, None);
                        e.resolved = crate::repo::CallLabels::is_heuristic(label).then_some(label);
                        entries.push(e);
                    }
                }
            }
            entries
        };
        let caller_entries = neighbours(&callers_of, false);
        let callee_entries = neighbours(&callees_of, true);
        let callers_total = caller_entries.len();
        let callees_total = callee_entries.len();

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
            callers: caller_entries.into_iter().take(limit).collect(),
            callers_total,
            callees: callee_entries.into_iter().take(limit).collect(),
            callees_total,
            communities_touched: touched_communities,
            changed_file_count: changed_files.len(),
            full_source_tokens: full_source_chars / 4,
        };

        self.graph_text(&graph, format::format_review_context(&context))
            .await
    }

    #[tool(
        name = "get_suggested_questions",
        description = "Auto-generate contextual code review questions based on code graph structure and git diff. Returns ranked questions about potential issues like missing test coverage, high caller counts, or API contract violations.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_suggested_questions(
        &self,
        Parameters(params): Parameters<GetSuggestedQuestionsParams>,
    ) -> TextResult {
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let repo_id = graph.repo_id().to_string();

        let symbols = graph.symbols().await?;

        let relationships = graph.relationships().await?;

        let files = graph.files().await?;

        let diff_text = diff_or_git(params.diff, &repo_id)?;

        let page = Page::new(params.limit, params.offset, Budget::new(5, 50));
        let questions =
            generate_suggested_questions(&diff_text, &symbols, &relationships, &files, usize::MAX);
        let page = page.take(questions.questions);
        let body = with_notice(
            format::format_suggested_questions(&SuggestedQuestionsOutput {
                questions: page.items,
            }),
            page.notice.as_deref(),
        );
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "get_graph_diff",
        description = "Compare the current knowledge graph against the last stored snapshot to detect architectural drift. Shows new/removed symbols and relationships since the last analysis that saved a snapshot.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_graph_diff(
        &self,
        Parameters(params): Parameters<GetGraphDiffParams>,
    ) -> TextResult {
        self.detect_architecture_drift(Parameters(DetectDriftParams {
            repo_id: params.repo_id,
            limit: params.limit,
            offset: params.offset,
            ..Default::default()
        }))
        .await
    }

    #[tool(
        name = "snapshot_diff",
        description = "Compare two graph snapshots to see architectural changes over time. Shows added/removed symbols and relationships. Defaults to comparing the two most recent snapshots.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn snapshot_diff(
        &self,
        Parameters(params): Parameters<SnapshotDiffParams>,
    ) -> TextResult {
        self.detect_architecture_drift(Parameters(DetectDriftParams {
            repo_id: params.repo_id,
            from_snapshot: params.from_snapshot,
            to_snapshot: params.to_snapshot,
            limit: params.limit,
            offset: params.offset,
        }))
        .await
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
