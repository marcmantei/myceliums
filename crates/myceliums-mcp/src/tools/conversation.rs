//! Email and conversation tools.

use super::prelude::*;

#[tool_router(router = conversation_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "search_emails",
        description = "Search indexed emails by keyword, with optional person and date filters. Returns matching Email symbols with subject, sender, date, and body snippet.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_emails(
        &self,
        Parameters(params): Parameters<SearchEmailsParams>,
    ) -> TextResult {
        let graph = self.graph_for(params.repo_id.as_deref()).await?;

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;

        require_query("query", &params.query)?;
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

        if results.is_empty() {
            return self
                .graph_text(
                    &graph,
                    format!("No emails found matching '{}'.", params.query),
                )
                .await;
        }

        let total = results.len();
        let page = Page::new(params.limit, params.offset, Budget::new(20, 100)).take(results);
        // Subjects, senders and bodies are indexed email text: untrusted.
        let mut emails = String::new();
        for s in &page.items {
            let snippet = myceliums_core::text::snippet(&s.content, 200);
            emails.push_str(&format!(
                "- **{}**\n  {}\n  Body: {}\n  UID: {}\n\n",
                s.name, s.signature, snippet, s.uid
            ));
        }
        let output = format!(
            "Found {total} email(s) matching '{}':\n\n{}\n",
            params.query,
            untrusted("emails", &emails)
        );

        self.graph_text(&graph, page.with_notice(output)).await
    }

    #[tool(
        name = "get_conversation",
        description = "Get a full email thread by conversation symbol UID. Returns all emails in the thread with their relationships.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_conversation(
        &self,
        Parameters(params): Parameters<GetConversationParams>,
    ) -> TextResult {
        let graph = self.graph_for(params.repo_id.as_deref()).await?;

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;

        // Find the conversation symbol
        let conv = symbols
            .iter()
            .find(|s| {
                s.uid == params.conversation_uid
                    && s.kind == myceliums_storage::SymbolKind::Conversation
            })
            .ok_or_else(|| {
                ToolError::recoverable(
                    format!("Conversation `{}` not found.", params.conversation_uid),
                    "pass a conversation UID from search_emails or get_person_context",
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

        let total = emails.len();
        let page = Page::new(params.limit, params.offset, Budget::new(20, 100)).take(emails);
        let mut thread = format!("Conversation: {}\n{}\n\n", conv.name, conv.signature);

        for email in &page.items {
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

            thread.push_str(&format!(
                "---\nFrom: {}\nSubject: {}\n{}\n\n{}\n\n",
                sender.unwrap_or("unknown"),
                email.name,
                email.signature,
                snippet,
            ));
        }

        let output = format!(
            "{total} emails in thread:\n\n{}\n",
            untrusted(&format!("conversation {}", conv.uid), &thread)
        );
        self.graph_text(&graph, page.with_notice(output)).await
    }

    #[tool(
        name = "get_person_context",
        description = "Get all emails involving a specific person (sent by or received by). Returns the person's email activity summary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_person_context(
        &self,
        Parameters(params): Parameters<GetPersonContextParams>,
    ) -> TextResult {
        let graph = self.graph_for(params.repo_id.as_deref()).await?;

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;

        require_query("person", &params.person)?;
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
            return self
                .graph_text(
                    &graph,
                    format!("No person found matching '{}'.", params.person),
                )
                .await;
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

        let emails: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| all_email_uids.contains(s.uid.as_str()))
            .collect();
        let page = Page::new(params.limit, params.offset, Budget::new(50, 200)).take(emails);

        let person_name = person_syms
            .first()
            .map(|s| s.name.as_str())
            .unwrap_or(&params.person);
        let person_email = person_syms
            .first()
            .map(|s| s.signature.as_str())
            .unwrap_or("");

        let mut listing = format!(
            "Person: {} ({})\nSent: {} emails, Received: {} emails ({} total)\n\n",
            person_name,
            person_email,
            sent_email_uids.len(),
            received_email_uids.len(),
            all_email_uids.len(),
        );

        for email in &page.items {
            let role = if sent_email_uids.contains(&email.uid.as_str()) {
                "sent"
            } else {
                "received"
            };
            listing.push_str(&format!(
                "- [{}] **{}**\n  {}\n  UID: {}\n\n",
                role, email.name, email.signature, email.uid,
            ));
        }

        let output = format!("{}\n", untrusted("emails", &listing));
        self.graph_text(&graph, page.with_notice(output)).await
    }
}
