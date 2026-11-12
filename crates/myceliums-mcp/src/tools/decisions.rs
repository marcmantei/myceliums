//! Architecture decisions, service mappings, and contracts.

use super::prelude::*;

#[tool_router(router = decisions_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "map_service",
        description = "Assign a human-readable service name to a community. Use with architecture_view to create meaningful service labels."
    )]
    pub(crate) async fn map_service(
        &self,
        Parameters(params): Parameters<MapServiceParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        self.resolve_repo_id(Some(&params.repo_id))?;
        save_service_mapping(
            &data_dir(),
            &params.repo_id,
            &params.community_label,
            &params.service_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!(
                "Mapped community '{}' to service '{}'",
                params.community_label, params.service_name
            ),
        }))
    }

    #[tool(
        name = "get_service_map",
        description = "List all community-to-service name mappings for a repository."
    )]
    async fn get_service_map(
        &self,
        Parameters(params): Parameters<GetServiceMapParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let mapping = load_service_mappings(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        if mapping.mappings.is_empty() {
            return Ok(Json(TextOutput {
                text: "No service mappings configured.".to_string(),
            }));
        }

        let mut out = format!("Service mappings for {}:\n\n", repo_id);
        for entry in &mapping.mappings {
            out.push_str(&format!(
                "  {} → {}\n",
                entry.community_label, entry.service_name
            ));
        }
        Ok(Json(TextOutput { text: out }))
    }

    #[tool(
        name = "record_decision",
        description = "Create an Architecture Decision Record (ADR). Records architectural decisions with context, rationale, and consequences. Link to code symbols with link_decision."
    )]
    pub(crate) async fn record_decision(
        &self,
        Parameters(params): Parameters<RecordDecisionParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let status = match params.status.as_deref() {
            Some("accepted") => AdrStatus::Accepted,
            Some("deprecated") => AdrStatus::Deprecated,
            Some("superseded") => AdrStatus::Superseded,
            _ => AdrStatus::Proposed,
        };

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
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!("ADR created: {} (id: {})", params.title, adr.id),
        }))
    }

    #[tool(
        name = "get_decisions",
        description = "List Architecture Decision Records (ADRs) for a repository. Optionally filter by status."
    )]
    async fn get_decisions(
        &self,
        Parameters(params): Parameters<GetDecisionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let decisions = load_decisions(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let filtered: Vec<_> = if let Some(status_filter) = &params.status {
            decisions
                .into_iter()
                .filter(|d| d.status.to_string().to_lowercase() == status_filter.to_lowercase())
                .collect()
        } else {
            decisions
        };

        Ok(Json(TextOutput {
            text: format::format_decisions(&filtered),
        }))
    }

    #[tool(
        name = "link_decision",
        description = "Link an Architecture Decision Record to a code symbol. Creates a traceability connection between the decision and the code it affects."
    )]
    pub(crate) async fn link_decision(
        &self,
        Parameters(params): Parameters<LinkDecisionParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        self.resolve_repo_id(Some(&params.repo_id))?;
        link_decision_to_symbol(
            &data_dir(),
            &params.repo_id,
            &params.decision_id,
            &params.symbol_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!(
                "Linked ADR {} to symbol '{}'",
                params.decision_id, params.symbol_name
            ),
        }))
    }

    #[tool(
        name = "get_contracts",
        description = "Detect API contracts (OpenAPI, Protobuf) in the repository and match endpoints to handler symbols. Returns linked and unlinked endpoints."
    )]
    async fn get_contracts(
        &self,
        Parameters(params): Parameters<GetContractsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo = registry.require(&repo_id).map_err(repo_id_error)?;

        let db_path = registered_db_path(&repo_id)?;
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let report = detect_contracts(std::path::Path::new(&repo.path), &symbols);

        Ok(Json(TextOutput {
            text: format::format_contracts(&report),
        }))
    }
}
