//! Email and conversation tools.

use super::prelude::*;

#[tool_router(router = conversation_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "search_emails",
        description = "Search indexed emails by keyword, with optional person and date filters. Returns matching Email symbols with subject, sender, date, and body snippet."
    )]
    async fn search_emails(
        &self,
        Parameters(params): Parameters<SearchEmailsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let limit = params.limit.unwrap_or(20);
        let query_lower = params.query.to_lowercase();

        // Filter email symbols by keyword
        let mut results: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| s.kind == myceliums_storage::SymbolKind::Email)
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.content.to_lowercase().contains(&query_lower)
            })
            .collect();

        // Apply person filter: check if email has SentBy or ReceivedBy relationship to a matching person
        if let Some(ref person_filter) = params.person {
            let person_lower = person_filter.to_lowercase();
            let person_uids: std::collections::HashSet<&str> = symbols
                .iter()
                .filter(|s| {
                    s.kind == myceliums_storage::SymbolKind::Person
                        && (s.signature.to_lowercase().contains(&person_lower)
                            || s.name.to_lowercase().contains(&person_lower))
                })
                .map(|s| s.uid.as_str())
                .collect();

            let email_uids_for_person: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| {
                    (r.kind == myceliums_storage::RelationshipKind::SentBy
                        || r.kind == myceliums_storage::RelationshipKind::ReceivedBy)
                        && person_uids.contains(r.target_uid.as_str())
                })
                .map(|r| r.source_uid.as_str())
                .collect();

            results.retain(|s| email_uids_for_person.contains(s.uid.as_str()));
        }

        // Apply date filter (matches on signature which contains the date)
        if let Some(ref date_filter) = params.date {
            results.retain(|s| s.signature.contains(date_filter));
        }

        results.truncate(limit);

        if results.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No emails found matching '{}'.", params.query),
            }));
        }

        let mut output = format!(
            "Found {} email(s) matching '{}':\n\n",
            results.len(),
            params.query
        );
        for s in &results {
            let snippet = myceliums_core::text::snippet(&s.content, 200);
            output.push_str(&format!(
                "- **{}**\n  {}\n  Body: {}\n  UID: {}\n\n",
                s.name, s.signature, snippet, s.uid
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    #[tool(
        name = "get_conversation",
        description = "Get a full email thread by conversation symbol UID. Returns all emails in the thread with their relationships."
    )]
    async fn get_conversation(
        &self,
        Parameters(params): Parameters<GetConversationParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        // Find the conversation symbol
        let conv = symbols
            .iter()
            .find(|s| {
                s.uid == params.conversation_uid
                    && s.kind == myceliums_storage::SymbolKind::Conversation
            })
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!("Conversation not found: {}", params.conversation_uid),
                    None,
                )
            })?;

        // Find all emails in this conversation via PartOfConversation relationships
        let email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::PartOfConversation
                    && r.target_uid == conv.uid
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let email_uid_set: std::collections::HashSet<&str> = email_uids.iter().copied().collect();

        let emails: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| email_uid_set.contains(s.uid.as_str()))
            .collect();

        let mut output = format!(
            "Conversation: {}\n{}\n{} emails in thread:\n\n",
            conv.name,
            conv.signature,
            emails.len()
        );

        for email in &emails {
            // Find sender
            let sender: Option<&str> = relationships
                .iter()
                .find(|r| {
                    r.source_uid == email.uid
                        && r.kind == myceliums_storage::RelationshipKind::SentBy
                })
                .and_then(|r| symbols.iter().find(|s| s.uid == r.target_uid))
                .map(|s| s.name.as_str());

            let snippet = myceliums_core::text::snippet(&email.content, 300);

            output.push_str(&format!(
                "---\nFrom: {}\nSubject: {}\n{}\n\n{}\n\n",
                sender.unwrap_or("unknown"),
                email.name,
                email.signature,
                snippet,
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    #[tool(
        name = "get_person_context",
        description = "Get all emails involving a specific person (sent by or received by). Returns the person's email activity summary."
    )]
    async fn get_person_context(
        &self,
        Parameters(params): Parameters<GetPersonContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;

        let symbols = self.symbols(&repo_id).await?;
        let relationships = self.relationships(&repo_id).await?;

        let limit = params.limit.unwrap_or(50);
        let person_lower = params.person.to_lowercase();

        // Find matching person symbols
        let person_syms: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| {
                s.kind == myceliums_storage::SymbolKind::Person
                    && (s.signature.to_lowercase().contains(&person_lower)
                        || s.name.to_lowercase().contains(&person_lower))
            })
            .collect();

        if person_syms.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No person found matching '{}'.", params.person),
            }));
        }

        let person_uid_set: std::collections::HashSet<&str> =
            person_syms.iter().map(|s| s.uid.as_str()).collect();

        // Find all emails linked to these persons
        let sent_email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::SentBy
                    && person_uid_set.contains(r.target_uid.as_str())
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let received_email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::ReceivedBy
                    && person_uid_set.contains(r.target_uid.as_str())
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let mut all_email_uids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        all_email_uids.extend(sent_email_uids.iter());
        all_email_uids.extend(received_email_uids.iter());

        let mut emails: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| all_email_uids.contains(s.uid.as_str()))
            .collect();
        emails.truncate(limit);

        let person_name = person_syms
            .first()
            .map(|s| s.name.as_str())
            .unwrap_or(&params.person);
        let person_email = person_syms
            .first()
            .map(|s| s.signature.as_str())
            .unwrap_or("");

        let mut output = format!(
            "Person: {} ({})\nSent: {} emails, Received: {} emails\nShowing {} of {} total:\n\n",
            person_name,
            person_email,
            sent_email_uids.len(),
            received_email_uids.len(),
            emails.len(),
            all_email_uids.len(),
        );

        for email in &emails {
            let role = if sent_email_uids.contains(&email.uid.as_str()) {
                "sent"
            } else {
                "received"
            };
            output.push_str(&format!(
                "- [{}] **{}**\n  {}\n  UID: {}\n\n",
                role, email.name, email.signature, email.uid,
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    // ── Cross-repo comparison tools (premium) ────────────────────────────
}
