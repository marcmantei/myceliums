//! Architecture decisions, service mappings, and contracts.

use super::prelude::*;

#[tool_router(router = decisions_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "map_service",
        description = "Assign a human-readable service name to a community. Use with architecture_view to create meaningful service labels.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn map_service(
        &self,
        Parameters(params): Parameters<MapServiceParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        self.resolve_repo_id(Some(&params.repo_id))?;
        save_service_mapping(
            &data_dir(),
            &params.repo_id,
            &params.community_label,
            &params.service_name,
        )
        .map_err(|e| ToolError::internal("cannot save the service mapping", e))?;

        text(format!(
            "Mapped community '{}' to service '{}'",
            params.community_label, params.service_name
        ))
    }

    #[tool(
        name = "get_service_map",
        description = "List all community-to-service name mappings for a repository.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_service_map(
        &self,
        Parameters(params): Parameters<GetServiceMapParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let mapping = load_service_mappings(&data_dir(), &repo_id)
            .map_err(|e| ToolError::internal("cannot read service mappings", e))?;

        if mapping.mappings.is_empty() {
            return text("No service mappings configured. Add one with map_service.");
        }

        let total = mapping.mappings.len();
        let page =
            Page::new(params.limit, params.offset, Budget::new(50, 500)).take(mapping.mappings);
        let mut out = format!("{total} service mappings for {}:\n\n", repo_id);
        for entry in &page.items {
            out.push_str(&format!(
                "  {} → {}\n",
                entry.community_label, entry.service_name
            ));
        }
        text(page.with_notice(out))
    }

    #[tool(
        name = "record_decision",
        description = "Create an Architecture Decision Record (ADR). Records architectural decisions with context, rationale, and consequences. Link to code symbols with link_decision.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(crate) async fn record_decision(
        &self,
        Parameters(params): Parameters<RecordDecisionParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        let status = adr_status(params.status.unwrap_or_default());

        let now = chrono::Utc::now().to_rfc3339();
        let adr = ArchDecisionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: params.title.clone(),
            status,
            context: params.context,
            decision: params.decision,
            consequences: params.consequences.unwrap_or_default(),
            linked_symbols: Vec::new(),
            superseded_by: None,
            created_at: now.clone(),
            updated_at: now,
        };

        self.resolve_repo_id(Some(&params.repo_id))?;
        save_decision(&data_dir(), &params.repo_id, &adr)
            .map_err(|e| ToolError::internal("cannot save the decision", e))?;

        text(format!("ADR created: {} (id: {})", params.title, adr.id))
    }

    #[tool(
        name = "get_decisions",
        description = "List Architecture Decision Records (ADRs) for a repository. Optionally filter by status.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_decisions(
        &self,
        Parameters(params): Parameters<GetDecisionsParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let decisions = load_decisions(&data_dir(), &repo_id)
            .map_err(|e| ToolError::internal("cannot read decisions", e))?;

        let filtered: Vec<_> = match params.status {
            Some(status) => {
                let wanted = adr_status(status);
                decisions
                    .into_iter()
                    .filter(|d| d.status == wanted)
                    .collect()
            }
            None => decisions,
        };

        let page = Page::new(params.limit, params.offset, Budget::LIST).take(filtered);
        text(page.with_notice(format::format_decisions(&page.items)))
    }

    #[tool(
        name = "link_decision",
        description = "Link an Architecture Decision Record to a code symbol. Creates a traceability connection between the decision and the code it affects.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn link_decision(
        &self,
        Parameters(params): Parameters<LinkDecisionParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        self.resolve_repo_id(Some(&params.repo_id))?;
        let decisions = load_decisions(&data_dir(), &params.repo_id)
            .map_err(|e| ToolError::internal("cannot read decisions", e))?;
        if !decisions.iter().any(|d| d.id == params.decision_id) {
            return Err(ToolError::recoverable(
                format!("Decision `{}` not found.", params.decision_id),
                "pass an ADR id listed by get_decisions",
            ));
        }
        link_decision_to_symbol(
            &data_dir(),
            &params.repo_id,
            &params.decision_id,
            &params.symbol_name,
        )
        .map_err(|e| ToolError::internal("cannot save the decision link", e))?;

        text(format!(
            "Linked ADR {} to symbol '{}'",
            params.decision_id, params.symbol_name
        ))
    }

    #[tool(
        name = "get_contracts",
        description = "Detect API contracts (OpenAPI, Protobuf) in the repository and match endpoints to handler symbols. Returns linked and unlinked endpoints.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_contracts(
        &self,
        Parameters(params): Parameters<GetContractsParams>,
    ) -> Result<Json<TextOutput>, ToolError> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let repo = repo_info(&repo_id)?;

        let symbols = self.symbols(&repo_id).await?;

        let mut report = detect_contracts(std::path::Path::new(&repo.path), &symbols);
        let page = Page::new(params.limit, params.offset, Budget::new(50, 500))
            .take(std::mem::take(&mut report.unlinked_endpoints));
        report.unlinked_endpoints = page.items;

        let body = with_notice(format::format_contracts(&report), page.notice.as_deref());
        self.graph_text(&repo_id, body).await
    }
}

/// The core ADR status for a requested one.
fn adr_status(status: DecisionStatus) -> AdrStatus {
    match status {
        DecisionStatus::Proposed => AdrStatus::Proposed,
        DecisionStatus::Accepted => AdrStatus::Accepted,
        DecisionStatus::Deprecated => AdrStatus::Deprecated,
        DecisionStatus::Superseded => AdrStatus::Superseded,
    }
}
