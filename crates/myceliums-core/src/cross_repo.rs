//! Cross-repository comparison and adaptation planning.
//!
//! This module provides **Partial Context Differentiation + Adaptation Planning**:
//! isolate an "intent" (feature/concept) in two codebases, compare how each
//! implements it, and generate an actionable adaptation plan.
//!
//! Requires the `cross_repo` feature flag.

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Community, Relationship, RelationshipKind, SymbolKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[cfg(feature = "embeddings")]
use crate::embeddings::Embedder;
use crate::search::search_symbols;

// ── Data model ──────────────────────────────────────────────────────────

/// A cluster of symbols from one repo that collectively implement an "intent."
#[derive(Debug, Clone, Serialize)]
pub struct IntentSlice {
    pub repo_id: String,
    pub repo_name: String,
    pub intent_query: String,
    /// Seed symbols found by search
    pub seed_symbols: Vec<SliceSymbol>,
    /// Expanded symbol set via call graph traversal
    pub expanded_symbols: Vec<SliceSymbol>,
    /// Relationships within the expanded set
    pub internal_relationships: Vec<SliceRelationship>,
    /// Community labels that these symbols belong to
    pub community_labels: Vec<String>,
    /// Languages present in this slice
    pub languages: Vec<String>,
    /// Total symbol count (seeds + expanded)
    pub total_symbols: usize,
}

/// Lightweight symbol representation for cross-repo output.
#[derive(Debug, Clone, Serialize)]
pub struct SliceSymbol {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
}

impl From<&CodeSymbol> for SliceSymbol {
    fn from(s: &CodeSymbol) -> Self {
        Self {
            uid: s.uid.clone(),
            name: s.name.clone(),
            qualified_name: s.qualified_name.clone(),
            kind: s.kind.to_string(),
            file_path: s.file_path.clone(),
            start_line: s.start_line,
            end_line: s.end_line,
            signature: s.signature.clone(),
        }
    }
}

/// Lightweight relationship for cross-repo output.
#[derive(Debug, Clone, Serialize)]
pub struct SliceRelationship {
    pub source_name: String,
    pub target_name: String,
    pub kind: String,
}

/// A pair of matched symbols across repos.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolAlignment {
    pub source_symbol: SliceSymbol,
    pub target_symbol: SliceSymbol,
    pub similarity_score: f64,
    pub match_type: String, // "DirectMatch", "SemanticMatch"
}

/// A symbol present in one slice but not the other.
#[derive(Debug, Clone, Serialize)]
pub struct UnmatchedSymbol {
    pub symbol: SliceSymbol,
    pub side: String, // "source" or "target"
}

/// Structural difference along a specific dimension.
#[derive(Debug, Clone, Serialize)]
pub struct StructuralDiff {
    pub dimension: String,
    pub source_value: String,
    pub target_value: String,
    pub significance: String, // "critical", "notable", "minor"
}

/// Full differentiation report comparing two intent slices.
#[derive(Debug, Clone, Serialize)]
pub struct DifferentiationReport {
    pub intent_query: String,
    pub source_slice: IntentSlice,
    pub target_slice: IntentSlice,
    pub alignments: Vec<SymbolAlignment>,
    pub unmatched: Vec<UnmatchedSymbol>,
    pub structural_differences: Vec<StructuralDiff>,
}

/// A single step in an adaptation plan.
#[derive(Debug, Clone, Serialize)]
pub struct AdaptationStep {
    pub order: u32,
    pub action: String,
    pub category: String, // "create", "adapt", "remove", "refactor"
    pub symbols_affected: Vec<String>,
    pub file_paths: Vec<String>,
    pub prerequisite_steps: Vec<u32>,
}

/// Full adaptation plan for migrating between approaches.
#[derive(Debug, Clone, Serialize)]
pub struct AdaptationPlan {
    pub intent_query: String,
    pub direction: String, // "source_to_target" or "target_to_source"
    pub source_repo: String,
    pub target_repo: String,
    pub steps: Vec<AdaptationStep>,
    pub symbols_to_create: Vec<SliceSymbol>,
    pub symbols_to_adapt: Vec<SymbolAlignment>,
    pub symbols_to_remove: Vec<SliceSymbol>,
    pub effort_estimate: String, // "small", "medium", "large", "very_large"
    pub risks: Vec<String>,
}

// ── Intent isolation ────────────────────────────────────────────────────

/// Configuration for intent isolation.
pub struct IsolateConfig {
    pub max_symbols: usize,
    pub expansion_depth: u32,
}

impl Default for IsolateConfig {
    fn default() -> Self {
        Self {
            max_symbols: 50,
            expansion_depth: 2,
        }
    }
}

/// Isolate the symbols implementing an "intent" in a single repository.
///
/// 1. Seed discovery via BM25 text search
/// 2. Call graph expansion (BFS, bidirectional)
/// 3. Cohesion pruning (community-aware)
pub fn isolate_intent(
    intent: &str,
    repo_id: &str,
    repo_name: &str,
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    communities: &[Community],
    config: &IsolateConfig,
) -> IntentSlice {
    // Stage 1: Seed discovery via BM25
    let search_results = search_symbols(symbols, intent);
    let seed_limit = config.max_symbols.min(10);
    let seed_symbols: Vec<&CodeSymbol> = search_results
        .iter()
        .take(seed_limit)
        .filter(|r| is_code_symbol(&r.symbol.kind))
        .map(|r| &r.symbol)
        .collect();

    let seed_uids: HashSet<String> = seed_symbols.iter().map(|s| s.uid.clone()).collect();

    // Stage 2: Call graph expansion (BFS, bidirectional)
    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();

    for rel in relationships {
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

    let mut expanded_uids: HashSet<String> = seed_uids.clone();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();

    for uid in &seed_uids {
        if let Some(callers) = callers_of.get(uid.as_str()) {
            for caller in callers {
                if !expanded_uids.contains(*caller) {
                    queue.push_back((caller.to_string(), 1));
                }
            }
        }
        if let Some(callees) = callees_of.get(uid.as_str()) {
            for callee in callees {
                if !expanded_uids.contains(*callee) {
                    queue.push_back((callee.to_string(), 1));
                }
            }
        }
    }

    while let Some((uid, dist)) = queue.pop_front() {
        if dist > config.expansion_depth
            || expanded_uids.contains(&uid)
            || expanded_uids.len() >= config.max_symbols
        {
            continue;
        }
        expanded_uids.insert(uid.clone());

        if dist < config.expansion_depth {
            if let Some(callers) = callers_of.get(uid.as_str()) {
                for caller in callers {
                    if !expanded_uids.contains(*caller) {
                        queue.push_back((caller.to_string(), dist + 1));
                    }
                }
            }
            if let Some(callees) = callees_of.get(uid.as_str()) {
                for callee in callees {
                    if !expanded_uids.contains(*callee) {
                        queue.push_back((callee.to_string(), dist + 1));
                    }
                }
            }
        }
    }

    // Stage 3: Cohesion pruning (community-aware)
    let uid_to_community = build_uid_to_community(communities);
    let mut community_counts: HashMap<&str, usize> = HashMap::new();
    for uid in &expanded_uids {
        if let Some(label) = uid_to_community.get(uid.as_str()) {
            *community_counts.entry(label).or_default() += 1;
        }
    }

    // Keep symbols in communities with >= 2 members, or if total communities <= 3
    let total_communities = community_counts.len();
    if total_communities > 3 {
        let threshold = 2;
        let keep_communities: HashSet<&str> = community_counts
            .iter()
            .filter(|(_, count)| **count >= threshold)
            .map(|(label, _)| *label)
            .collect();

        expanded_uids.retain(|uid| {
            seed_uids.contains(uid)
                || uid_to_community
                    .get(uid.as_str())
                    .map(|l| keep_communities.contains(l.as_str()))
                    .unwrap_or(true)
        });
    }

    // Build output
    let seed_out: Vec<SliceSymbol> = seed_symbols.iter().map(|s| SliceSymbol::from(*s)).collect();

    let expanded_only: Vec<SliceSymbol> = expanded_uids
        .iter()
        .filter(|uid| !seed_uids.contains(uid.as_str()))
        .filter_map(|uid| uid_to_symbol.get(uid.as_str()))
        .map(|s| SliceSymbol::from(*s))
        .collect();

    // Internal relationships (both endpoints in the slice)
    let internal_rels: Vec<SliceRelationship> = relationships
        .iter()
        .filter(|r| expanded_uids.contains(&r.source_uid) && expanded_uids.contains(&r.target_uid))
        .filter_map(|r| {
            let source = uid_to_symbol.get(r.source_uid.as_str())?;
            let target = uid_to_symbol.get(r.target_uid.as_str())?;
            Some(SliceRelationship {
                source_name: source.qualified_name.clone(),
                target_name: target.qualified_name.clone(),
                kind: r.kind.to_string(),
            })
        })
        .collect();

    // Community labels
    let mut slice_communities: HashSet<String> = HashSet::new();
    for uid in &expanded_uids {
        if let Some(label) = uid_to_community.get(uid.as_str()) {
            slice_communities.insert(label.clone());
        }
    }

    // Languages
    let mut languages: HashSet<String> = HashSet::new();
    for uid in &expanded_uids {
        if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
            if let Some(ext) = std::path::Path::new(&sym.file_path)
                .extension()
                .and_then(|e| e.to_str())
            {
                languages.insert(ext.to_string());
            }
        }
    }

    let total = seed_out.len() + expanded_only.len();

    IntentSlice {
        repo_id: repo_id.to_string(),
        repo_name: repo_name.to_string(),
        intent_query: intent.to_string(),
        seed_symbols: seed_out,
        expanded_symbols: expanded_only,
        internal_relationships: internal_rels,
        community_labels: slice_communities.into_iter().collect(),
        languages: languages.into_iter().collect(),
        total_symbols: total,
    }
}

/// Isolate intent using hybrid search (BM25 + vector). Requires `embeddings` feature.
#[cfg(feature = "embeddings")]
pub async fn isolate_intent_hybrid(
    intent: &str,
    repo_id: &str,
    repo_name: &str,
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    communities: &[Community],
    config: &IsolateConfig,
) -> Result<IntentSlice> {
    use crate::hybrid_search::hybrid_search;

    let seed_limit = config.max_symbols.min(10);
    let search_results = hybrid_search(symbols, intent, seed_limit).await?;

    let seed_uids: HashSet<String> = search_results
        .iter()
        .filter(|r| is_code_symbol(&r.symbol.kind))
        .map(|r| r.symbol.uid.clone())
        .collect();

    let seed_symbols: Vec<SliceSymbol> = search_results
        .iter()
        .filter(|r| is_code_symbol(&r.symbol.kind))
        .map(|r| SliceSymbol::from(&r.symbol))
        .collect();

    // Reuse the expansion + pruning from the sync version
    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();

    for rel in relationships {
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

    let mut expanded_uids: HashSet<String> = seed_uids.clone();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();

    for uid in &seed_uids {
        if let Some(callers) = callers_of.get(uid.as_str()) {
            for caller in callers {
                if !expanded_uids.contains(*caller) {
                    queue.push_back((caller.to_string(), 1));
                }
            }
        }
        if let Some(callees) = callees_of.get(uid.as_str()) {
            for callee in callees {
                if !expanded_uids.contains(*callee) {
                    queue.push_back((callee.to_string(), 1));
                }
            }
        }
    }

    while let Some((uid, dist)) = queue.pop_front() {
        if dist > config.expansion_depth
            || expanded_uids.contains(&uid)
            || expanded_uids.len() >= config.max_symbols
        {
            continue;
        }
        expanded_uids.insert(uid.clone());

        if dist < config.expansion_depth {
            if let Some(callers) = callers_of.get(uid.as_str()) {
                for caller in callers {
                    if !expanded_uids.contains(*caller) {
                        queue.push_back((caller.to_string(), dist + 1));
                    }
                }
            }
            if let Some(callees) = callees_of.get(uid.as_str()) {
                for callee in callees {
                    if !expanded_uids.contains(*callee) {
                        queue.push_back((callee.to_string(), dist + 1));
                    }
                }
            }
        }
    }

    // Cohesion pruning
    let uid_to_community = build_uid_to_community(communities);
    let mut community_counts: HashMap<&str, usize> = HashMap::new();
    for uid in &expanded_uids {
        if let Some(label) = uid_to_community.get(uid.as_str()) {
            *community_counts.entry(label).or_default() += 1;
        }
    }
    if community_counts.len() > 3 {
        let keep_communities: HashSet<&str> = community_counts
            .iter()
            .filter(|(_, count)| **count >= 2)
            .map(|(label, _)| *label)
            .collect();
        expanded_uids.retain(|uid| {
            seed_uids.contains(uid)
                || uid_to_community
                    .get(uid.as_str())
                    .map(|l| keep_communities.contains(l.as_str()))
                    .unwrap_or(true)
        });
    }

    let expanded_only: Vec<SliceSymbol> = expanded_uids
        .iter()
        .filter(|uid| !seed_uids.contains(uid.as_str()))
        .filter_map(|uid| uid_to_symbol.get(uid.as_str()))
        .map(|s| SliceSymbol::from(*s))
        .collect();

    let internal_rels: Vec<SliceRelationship> = relationships
        .iter()
        .filter(|r| expanded_uids.contains(&r.source_uid) && expanded_uids.contains(&r.target_uid))
        .filter_map(|r| {
            let source = uid_to_symbol.get(r.source_uid.as_str())?;
            let target = uid_to_symbol.get(r.target_uid.as_str())?;
            Some(SliceRelationship {
                source_name: source.qualified_name.clone(),
                target_name: target.qualified_name.clone(),
                kind: r.kind.to_string(),
            })
        })
        .collect();

    let mut slice_communities: HashSet<String> = HashSet::new();
    for uid in &expanded_uids {
        if let Some(label) = uid_to_community.get(uid.as_str()) {
            slice_communities.insert(label.clone());
        }
    }

    let mut languages: HashSet<String> = HashSet::new();
    for uid in &expanded_uids {
        if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
            if let Some(ext) = std::path::Path::new(&sym.file_path)
                .extension()
                .and_then(|e| e.to_str())
            {
                languages.insert(ext.to_string());
            }
        }
    }

    let total = seed_symbols.len() + expanded_only.len();

    Ok(IntentSlice {
        repo_id: repo_id.to_string(),
        repo_name: repo_name.to_string(),
        intent_query: intent.to_string(),
        seed_symbols,
        expanded_symbols: expanded_only,
        internal_relationships: internal_rels,
        community_labels: slice_communities.into_iter().collect(),
        languages: languages.into_iter().collect(),
        total_symbols: total,
    })
}

// ── Differentiation ─────────────────────────────────────────────────────

/// Configuration for differentiation.
pub struct DifferentiateConfig {
    pub similarity_threshold: f64,
}

impl Default for DifferentiateConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.65,
        }
    }
}

/// Differentiate two intent slices by aligning symbols and comparing structure.
///
/// Uses embedding cosine similarity for symbol alignment when vectors are available,
/// falls back to name-based matching otherwise.
#[cfg(feature = "embeddings")]
pub fn differentiate_with_vectors(
    source_slice: &IntentSlice,
    target_slice: &IntentSlice,
    source_symbols_with_vectors: &[(CodeSymbol, Option<Vec<f32>>)],
    target_symbols_with_vectors: &[(CodeSymbol, Option<Vec<f32>>)],
    config: &DifferentiateConfig,
) -> DifferentiationReport {
    let all_source = all_symbols(source_slice);
    let all_target = all_symbols(target_slice);

    // Build vector lookups
    let source_vectors: HashMap<&str, &Vec<f32>> = source_symbols_with_vectors
        .iter()
        .filter_map(|(s, v)| v.as_ref().map(|vec| (s.uid.as_str(), vec)))
        .collect();
    let target_vectors: HashMap<&str, &Vec<f32>> = target_symbols_with_vectors
        .iter()
        .filter_map(|(s, v)| v.as_ref().map(|vec| (s.uid.as_str(), vec)))
        .collect();

    let mut alignments: Vec<SymbolAlignment> = Vec::new();
    let mut matched_target_uids: HashSet<String> = HashSet::new();
    let mut unmatched: Vec<UnmatchedSymbol> = Vec::new();

    for source_sym in &all_source {
        let mut best_match: Option<(&SliceSymbol, f64, &str)> = None;

        for target_sym in &all_target {
            if matched_target_uids.contains(&target_sym.uid) {
                continue;
            }

            // Check for direct name match
            if source_sym.name == target_sym.name {
                let score = if let (Some(sv), Some(tv)) = (
                    source_vectors.get(source_sym.uid.as_str()),
                    target_vectors.get(target_sym.uid.as_str()),
                ) {
                    Embedder::cosine_similarity(sv, tv)
                } else {
                    0.9 // Name match without vectors gets high score
                };
                if best_match.as_ref().is_none_or(|(_, s, _)| score > *s) {
                    best_match = Some((target_sym, score, "DirectMatch"));
                }
                continue;
            }

            // Semantic match via embeddings
            if let (Some(sv), Some(tv)) = (
                source_vectors.get(source_sym.uid.as_str()),
                target_vectors.get(target_sym.uid.as_str()),
            ) {
                let score = Embedder::cosine_similarity(sv, tv);
                if score >= config.similarity_threshold
                    && best_match.as_ref().is_none_or(|(_, s, _)| score > *s)
                {
                    best_match = Some((target_sym, score, "SemanticMatch"));
                }
            }
        }

        if let Some((target_sym, score, match_type)) = best_match {
            matched_target_uids.insert(target_sym.uid.clone());
            alignments.push(SymbolAlignment {
                source_symbol: source_sym.clone(),
                target_symbol: target_sym.clone(),
                similarity_score: score,
                match_type: match_type.to_string(),
            });
        } else {
            unmatched.push(UnmatchedSymbol {
                symbol: source_sym.clone(),
                side: "source".to_string(),
            });
        }
    }

    // Add unmatched target symbols
    for target_sym in &all_target {
        if !matched_target_uids.contains(&target_sym.uid) {
            unmatched.push(UnmatchedSymbol {
                symbol: target_sym.clone(),
                side: "target".to_string(),
            });
        }
    }

    let structural_diffs = compute_structural_diffs(source_slice, target_slice);

    DifferentiationReport {
        intent_query: source_slice.intent_query.clone(),
        source_slice: source_slice.clone(),
        target_slice: target_slice.clone(),
        alignments,
        unmatched,
        structural_differences: structural_diffs,
    }
}

/// Name-based differentiation fallback (no embeddings required).
pub fn differentiate_by_name(
    source_slice: &IntentSlice,
    target_slice: &IntentSlice,
) -> DifferentiationReport {
    let all_source = all_symbols(source_slice);
    let all_target = all_symbols(target_slice);

    let target_by_name: HashMap<&str, &SliceSymbol> =
        all_target.iter().map(|s| (s.name.as_str(), s)).collect();

    let mut alignments: Vec<SymbolAlignment> = Vec::new();
    let mut matched_names: HashSet<String> = HashSet::new();
    let mut unmatched: Vec<UnmatchedSymbol> = Vec::new();

    for source_sym in &all_source {
        if let Some(target_sym) = target_by_name.get(source_sym.name.as_str()) {
            matched_names.insert(source_sym.name.clone());
            alignments.push(SymbolAlignment {
                source_symbol: source_sym.clone(),
                target_symbol: (*target_sym).clone(),
                similarity_score: 1.0,
                match_type: "DirectMatch".to_string(),
            });
        } else {
            unmatched.push(UnmatchedSymbol {
                symbol: source_sym.clone(),
                side: "source".to_string(),
            });
        }
    }

    for target_sym in &all_target {
        if !matched_names.contains(&target_sym.name) {
            unmatched.push(UnmatchedSymbol {
                symbol: target_sym.clone(),
                side: "target".to_string(),
            });
        }
    }

    let structural_diffs = compute_structural_diffs(source_slice, target_slice);

    DifferentiationReport {
        intent_query: source_slice.intent_query.clone(),
        source_slice: source_slice.clone(),
        target_slice: target_slice.clone(),
        alignments,
        unmatched,
        structural_differences: structural_diffs,
    }
}

// ── Adaptation planning ─────────────────────────────────────────────────

/// Generate an adaptation plan from a differentiation report.
pub fn plan_adaptation(
    report: &DifferentiationReport,
    direction: &str, // "source_to_target" or "target_to_source"
) -> AdaptationPlan {
    let (from_slice, to_slice) = if direction == "target_to_source" {
        (&report.target_slice, &report.source_slice)
    } else {
        (&report.source_slice, &report.target_slice)
    };

    // Symbols to create: exist in source approach but not in target
    let symbols_to_create: Vec<SliceSymbol> = report
        .unmatched
        .iter()
        .filter(|u| {
            if direction == "target_to_source" {
                u.side == "target"
            } else {
                u.side == "source"
            }
        })
        .map(|u| u.symbol.clone())
        .collect();

    // Symbols to remove: exist in target but have no equivalent in source
    let symbols_to_remove: Vec<SliceSymbol> = report
        .unmatched
        .iter()
        .filter(|u| {
            if direction == "target_to_source" {
                u.side == "source"
            } else {
                u.side == "target"
            }
        })
        .map(|u| u.symbol.clone())
        .collect();

    // Symbols to adapt: matched but potentially different
    let symbols_to_adapt: Vec<SymbolAlignment> = report.alignments.clone();

    // Build steps with dependency ordering
    let mut steps: Vec<AdaptationStep> = Vec::new();
    let mut step_order: u32 = 1;

    // Step group 1: Understand the source approach
    steps.push(AdaptationStep {
        order: step_order,
        action: format!(
            "Analyze {} intent implementation in {} ({} symbols across {} communities)",
            from_slice.intent_query,
            from_slice.repo_name,
            from_slice.total_symbols,
            from_slice.community_labels.len()
        ),
        category: "analyze".to_string(),
        symbols_affected: all_symbols(from_slice)
            .iter()
            .map(|s| s.qualified_name.clone())
            .collect(),
        file_paths: collect_file_paths(from_slice),
        prerequisite_steps: vec![],
    });
    step_order += 1;

    // Step group 2: Create new symbols (leaf-first via reverse topo)
    if !symbols_to_create.is_empty() {
        let create_files: Vec<String> = symbols_to_create
            .iter()
            .map(|s| s.file_path.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        steps.push(AdaptationStep {
            order: step_order,
            action: format!(
                "Create {} new symbols from {} approach: {}",
                symbols_to_create.len(),
                from_slice.repo_name,
                symbols_to_create
                    .iter()
                    .take(5)
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            category: "create".to_string(),
            symbols_affected: symbols_to_create
                .iter()
                .map(|s| s.qualified_name.clone())
                .collect(),
            file_paths: create_files,
            prerequisite_steps: vec![1],
        });
        step_order += 1;
    }

    // Step group 3: Adapt matched symbols
    if !symbols_to_adapt.is_empty() {
        let adapt_files: Vec<String> = symbols_to_adapt
            .iter()
            .map(|a| {
                if direction == "target_to_source" {
                    a.source_symbol.file_path.clone()
                } else {
                    a.target_symbol.file_path.clone()
                }
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        steps.push(AdaptationStep {
            order: step_order,
            action: format!(
                "Adapt {} matched symbols to align with {} approach",
                symbols_to_adapt.len(),
                from_slice.repo_name
            ),
            category: "adapt".to_string(),
            symbols_affected: symbols_to_adapt
                .iter()
                .map(|a| {
                    if direction == "target_to_source" {
                        a.source_symbol.qualified_name.clone()
                    } else {
                        a.target_symbol.qualified_name.clone()
                    }
                })
                .collect(),
            file_paths: adapt_files,
            prerequisite_steps: if symbols_to_create.is_empty() {
                vec![1]
            } else {
                vec![1, step_order - 1]
            },
        });
        step_order += 1;
    }

    // Step group 4: Remove symbols no longer needed
    if !symbols_to_remove.is_empty() {
        let remove_files: Vec<String> = symbols_to_remove
            .iter()
            .map(|s| s.file_path.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        steps.push(AdaptationStep {
            order: step_order,
            action: format!(
                "Remove {} symbols not present in {} approach: {}",
                symbols_to_remove.len(),
                from_slice.repo_name,
                symbols_to_remove
                    .iter()
                    .take(5)
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            category: "remove".to_string(),
            symbols_affected: symbols_to_remove
                .iter()
                .map(|s| s.qualified_name.clone())
                .collect(),
            file_paths: remove_files,
            prerequisite_steps: vec![step_order - 1],
        });
        step_order += 1;
    }

    // Step group 5: Integration testing
    steps.push(AdaptationStep {
        order: step_order,
        action: "Run tests and validate the adapted implementation".to_string(),
        category: "verify".to_string(),
        symbols_affected: vec![],
        file_paths: vec![],
        prerequisite_steps: vec![step_order - 1],
    });

    // Effort estimate
    let total_changes = symbols_to_create.len() + symbols_to_adapt.len() + symbols_to_remove.len();
    let effort = match total_changes {
        0..=5 => "small",
        6..=15 => "medium",
        16..=40 => "large",
        _ => "very_large",
    };

    // Risks
    let mut risks: Vec<String> = Vec::new();
    if !symbols_to_remove.is_empty() {
        risks.push(format!(
            "Removing {} symbols may break callers outside the isolated intent slice",
            symbols_to_remove.len()
        ));
    }
    if from_slice.languages != to_slice.languages {
        risks.push(format!(
            "Language mismatch: source uses {:?}, target uses {:?}",
            from_slice.languages, to_slice.languages
        ));
    }
    if report.unmatched.len() > report.alignments.len() {
        risks.push(
            "More unmatched than matched symbols — approaches may be fundamentally different"
                .to_string(),
        );
    }
    if report
        .structural_differences
        .iter()
        .any(|d| d.significance == "critical")
    {
        risks.push("Critical structural differences detected — adaptation may require architectural changes".to_string());
    }

    AdaptationPlan {
        intent_query: report.intent_query.clone(),
        direction: direction.to_string(),
        source_repo: from_slice.repo_name.clone(),
        target_repo: to_slice.repo_name.clone(),
        steps,
        symbols_to_create,
        symbols_to_adapt,
        symbols_to_remove,
        effort_estimate: effort.to_string(),
        risks,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_code_symbol(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::TypeAlias
            | SymbolKind::Enum
            | SymbolKind::Module
            | SymbolKind::Constant
            | SymbolKind::Variable
    )
}

fn all_symbols(slice: &IntentSlice) -> Vec<SliceSymbol> {
    let mut all = slice.seed_symbols.clone();
    all.extend(slice.expanded_symbols.clone());
    all
}

fn collect_file_paths(slice: &IntentSlice) -> Vec<String> {
    let mut paths: HashSet<String> = HashSet::new();
    for s in &slice.seed_symbols {
        paths.insert(s.file_path.clone());
    }
    for s in &slice.expanded_symbols {
        paths.insert(s.file_path.clone());
    }
    paths.into_iter().collect()
}

fn build_uid_to_community(communities: &[Community]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for c in communities {
        // top_symbols is comma-separated list of symbol names — we need UIDs
        // Communities store member UIDs in the label field as "community_{label}"
        // Since we don't have direct UID->community mapping, use the summary field
        // which contains symbol names. For cohesion pruning, we use the community label.
        //
        // The community.top_symbols field contains comma-separated qualified names.
        // We map each qualified name back to the community label.
        for name in c.top_symbols.split(", ") {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                map.insert(trimmed.to_string(), c.label.clone());
            }
        }
    }
    map
}

fn compute_structural_diffs(source: &IntentSlice, target: &IntentSlice) -> Vec<StructuralDiff> {
    let mut diffs = Vec::new();

    // Complexity (symbol count)
    let src_count = source.total_symbols;
    let tgt_count = target.total_symbols;
    let complexity_ratio = if src_count.max(tgt_count) > 0 {
        (src_count as f64 - tgt_count as f64).abs() / src_count.max(tgt_count) as f64
    } else {
        0.0
    };
    diffs.push(StructuralDiff {
        dimension: "Complexity (symbol count)".to_string(),
        source_value: format!("{} symbols", src_count),
        target_value: format!("{} symbols", tgt_count),
        significance: if complexity_ratio > 0.5 {
            "critical"
        } else if complexity_ratio > 0.25 {
            "notable"
        } else {
            "minor"
        }
        .to_string(),
    });

    // Language mix
    let src_langs = &source.languages;
    let tgt_langs = &target.languages;
    let lang_match = src_langs.iter().any(|l| tgt_langs.contains(l));
    diffs.push(StructuralDiff {
        dimension: "Language mix".to_string(),
        source_value: src_langs.join(", "),
        target_value: tgt_langs.join(", "),
        significance: if !lang_match { "critical" } else { "minor" }.to_string(),
    });

    // Modularity (community spread)
    let src_comm = source.community_labels.len();
    let tgt_comm = target.community_labels.len();
    diffs.push(StructuralDiff {
        dimension: "Modularity (communities)".to_string(),
        source_value: format!("{} communities", src_comm),
        target_value: format!("{} communities", tgt_comm),
        significance: if (src_comm as i32 - tgt_comm as i32).unsigned_abs() > 2 {
            "notable"
        } else {
            "minor"
        }
        .to_string(),
    });

    // Relationship density
    let src_rels = source.internal_relationships.len();
    let tgt_rels = target.internal_relationships.len();
    let src_density = if src_count > 0 {
        src_rels as f64 / src_count as f64
    } else {
        0.0
    };
    let tgt_density = if tgt_count > 0 {
        tgt_rels as f64 / tgt_count as f64
    } else {
        0.0
    };
    diffs.push(StructuralDiff {
        dimension: "Relationship density".to_string(),
        source_value: format!("{:.1} rels/symbol ({} total)", src_density, src_rels),
        target_value: format!("{:.1} rels/symbol ({} total)", tgt_density, tgt_rels),
        significance: if (src_density - tgt_density).abs() > 1.0 {
            "notable"
        } else {
            "minor"
        }
        .to_string(),
    });

    // Architectural style (kind distribution)
    let src_kinds = kind_distribution(source);
    let tgt_kinds = kind_distribution(target);
    diffs.push(StructuralDiff {
        dimension: "Architectural style".to_string(),
        source_value: format_kind_distribution(&src_kinds),
        target_value: format_kind_distribution(&tgt_kinds),
        significance: {
            let src_has_classes = src_kinds.get("Class").copied().unwrap_or(0) > 0;
            let tgt_has_classes = tgt_kinds.get("Class").copied().unwrap_or(0) > 0;
            if src_has_classes != tgt_has_classes {
                "notable"
            } else {
                "minor"
            }
        }
        .to_string(),
    });

    diffs
}

fn kind_distribution(slice: &IntentSlice) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in &slice.seed_symbols {
        *counts.entry(s.kind.clone()).or_default() += 1;
    }
    for s in &slice.expanded_symbols {
        *counts.entry(s.kind.clone()).or_default() += 1;
    }
    counts
}

fn format_kind_distribution(dist: &HashMap<String, usize>) -> String {
    let mut parts: Vec<String> = dist
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(kind, count)| format!("{}x {}", count, kind))
        .collect();
    parts.sort();
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

    fn make_symbol(uid: &str, name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("{}::{}", file, name),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: format!("fn {}() {{}}", name),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_call(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_isolate_intent_basic() {
        let symbols = vec![
            make_symbol("s1", "parse", "parser.rs"),
            make_symbol("s2", "parse_node", "parser.rs"),
            make_symbol("s3", "unrelated", "other.rs"),
        ];
        let relationships = vec![make_call("s1", "s2")];
        let communities = vec![];

        let config = IsolateConfig {
            max_symbols: 50,
            expansion_depth: 2,
        };

        let slice = isolate_intent(
            "parse",
            "repo-1",
            "Test Repo",
            &symbols,
            &relationships,
            &communities,
            &config,
        );

        assert!(!slice.seed_symbols.is_empty());
        assert!(slice.total_symbols >= 1);
        assert_eq!(slice.repo_id, "repo-1");
    }

    #[test]
    fn test_differentiate_by_name() {
        let source = IntentSlice {
            repo_id: "a".to_string(),
            repo_name: "Repo A".to_string(),
            intent_query: "parse".to_string(),
            seed_symbols: vec![SliceSymbol {
                uid: "s1".to_string(),
                name: "parse".to_string(),
                qualified_name: "a::parse".to_string(),
                kind: "Function".to_string(),
                file_path: "parser.rs".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "fn parse()".to_string(),
            }],
            expanded_symbols: vec![],
            internal_relationships: vec![],
            community_labels: vec![],
            languages: vec!["rs".to_string()],
            total_symbols: 1,
        };

        let target = IntentSlice {
            repo_id: "b".to_string(),
            repo_name: "Repo B".to_string(),
            intent_query: "parse".to_string(),
            seed_symbols: vec![
                SliceSymbol {
                    uid: "t1".to_string(),
                    name: "parse".to_string(),
                    qualified_name: "b::parse".to_string(),
                    kind: "Function".to_string(),
                    file_path: "parser.py".to_string(),
                    start_line: 1,
                    end_line: 10,
                    signature: "def parse()".to_string(),
                },
                SliceSymbol {
                    uid: "t2".to_string(),
                    name: "tokenize".to_string(),
                    qualified_name: "b::tokenize".to_string(),
                    kind: "Function".to_string(),
                    file_path: "parser.py".to_string(),
                    start_line: 11,
                    end_line: 20,
                    signature: "def tokenize()".to_string(),
                },
            ],
            expanded_symbols: vec![],
            internal_relationships: vec![],
            community_labels: vec![],
            languages: vec!["py".to_string()],
            total_symbols: 2,
        };

        let report = differentiate_by_name(&source, &target);

        assert_eq!(report.alignments.len(), 1);
        assert_eq!(report.alignments[0].match_type, "DirectMatch");
        assert_eq!(report.unmatched.len(), 1);
        assert_eq!(report.unmatched[0].symbol.name, "tokenize");
        assert_eq!(report.unmatched[0].side, "target");
    }

    #[test]
    fn test_plan_adaptation() {
        let source = IntentSlice {
            repo_id: "a".to_string(),
            repo_name: "Repo A".to_string(),
            intent_query: "parse".to_string(),
            seed_symbols: vec![SliceSymbol {
                uid: "s1".to_string(),
                name: "parse".to_string(),
                qualified_name: "a::parse".to_string(),
                kind: "Function".to_string(),
                file_path: "parser.rs".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "fn parse()".to_string(),
            }],
            expanded_symbols: vec![],
            internal_relationships: vec![],
            community_labels: vec![],
            languages: vec!["rs".to_string()],
            total_symbols: 1,
        };

        let target = IntentSlice {
            repo_id: "b".to_string(),
            repo_name: "Repo B".to_string(),
            intent_query: "parse".to_string(),
            seed_symbols: vec![],
            expanded_symbols: vec![],
            internal_relationships: vec![],
            community_labels: vec![],
            languages: vec!["py".to_string()],
            total_symbols: 0,
        };

        let report = DifferentiationReport {
            intent_query: "parse".to_string(),
            source_slice: source,
            target_slice: target,
            alignments: vec![],
            unmatched: vec![UnmatchedSymbol {
                symbol: SliceSymbol {
                    uid: "s1".to_string(),
                    name: "parse".to_string(),
                    qualified_name: "a::parse".to_string(),
                    kind: "Function".to_string(),
                    file_path: "parser.rs".to_string(),
                    start_line: 1,
                    end_line: 10,
                    signature: "fn parse()".to_string(),
                },
                side: "source".to_string(),
            }],
            structural_differences: vec![],
        };

        let plan = plan_adaptation(&report, "source_to_target");

        assert!(!plan.steps.is_empty());
        assert_eq!(plan.symbols_to_create.len(), 1);
        assert_eq!(plan.effort_estimate, "small");
    }
}
