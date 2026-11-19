//! The tool error contract: how a failed tool call reaches the agent.
//!
//! A tool call can fail in three ways, and each has one channel:
//!
//! * **Recoverable** — the call was well-formed, but the agent has to do
//!   something different: the symbol does not exist, a name is ambiguous, no
//!   index exists yet, a snapshot is missing. These are returned as a normal
//!   tool result with `isError: true`, so the model reads them, and the text
//!   always ends with a `Next:` line that names the concrete next step.
//! * **Invalid params** — the arguments themselves are wrong (malformed
//!   `repo_id`, unparsable Cypher, mutually exclusive options). JSON-RPC
//!   `invalid_params`.
//! * **Internal** — a genuine server fault (I/O, storage, serialization).
//!   JSON-RPC `internal_error`; the agent cannot fix these.
//!
//! Every handler returns `Result<_, ToolError>` (text tools via
//! `tools::TextResult`; structured tools spell out
//! `Result<Json<T>, ToolError>`, because rmcp derives the `outputSchema` from
//! that literal return type and does not see through an alias), so the
//! choice is made once per failure site by picking a constructor — never by
//! string matching later.

use myceliums_core::error::MyceliumError;
use myceliums_storage::RepoIdError;
use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::{CallToolResult, Content};
use std::fmt::Display;

/// A failed tool call, already classified (see the module docs).
#[derive(Debug)]
pub(crate) enum ToolError {
    /// Sent as an `isError: true` result: `{problem}\nNext: {next}`.
    Recoverable { problem: String, next: String },
    /// Sent as a JSON-RPC error (`invalid_params` or `internal_error`).
    Protocol(rmcp::ErrorData),
}

impl ToolError {
    /// A failure the agent can act on; `next` is the concrete next step.
    pub(crate) fn recoverable(problem: impl Into<String>, next: impl Into<String>) -> Self {
        ToolError::Recoverable {
            problem: problem.into(),
            next: next.into(),
        }
    }

    /// The arguments are wrong: JSON-RPC `invalid_params`.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        ToolError::Protocol(rmcp::ErrorData::invalid_params(message.into(), None))
    }

    /// A server fault: JSON-RPC `internal_error`, `"{context}: {error}"`.
    pub(crate) fn internal(context: &str, error: impl Display) -> Self {
        ToolError::Protocol(rmcp::ErrorData::internal_error(
            format!("{context}: {error}"),
            None,
        ))
    }

    /// The symbol does not exist in the index.
    pub(crate) fn symbol_not_found(name: &str) -> Self {
        Self::recoverable(
            format!("Symbol `{name}` not found in the index."),
            "check the spelling, find the symbol with search_code, or re-run analyze if it is new",
        )
    }

    /// The symbol does not exist, but names like it do.
    pub(crate) fn symbol_not_found_did_you_mean(name: &str, similar: &[String]) -> Self {
        let similar: Vec<String> = similar.iter().map(|s| format!("`{s}`")).collect();
        Self::recoverable(
            format!("Symbol `{name}` not found in the index."),
            format!(
                "did you mean {}? Otherwise find it with search_code, or re-run analyze if it is new",
                similar.join(", ")
            ),
        )
    }

    /// The agent-visible text, whichever channel carries it.
    pub(crate) fn message(&self) -> String {
        match self {
            ToolError::Recoverable { problem, next } => format!("{problem}\nNext: {next}"),
            ToolError::Protocol(e) => e.message.to_string(),
        }
    }

    /// For channels without `isError` results (resource reads): a
    /// recoverable failure becomes `invalid_params` carrying the same text.
    pub(crate) fn into_error_data(self) -> rmcp::ErrorData {
        match self {
            ToolError::Protocol(e) => e,
            recoverable => rmcp::ErrorData::invalid_params(recoverable.message(), None),
        }
    }
}

impl From<rmcp::ErrorData> for ToolError {
    fn from(e: rmcp::ErrorData) -> Self {
        ToolError::Protocol(e)
    }
}

impl IntoCallToolResult for ToolError {
    fn into_call_tool_result(self) -> Result<CallToolResult, rmcp::ErrorData> {
        match self {
            ToolError::Protocol(e) => Err(e),
            recoverable => Ok(CallToolResult::error(vec![Content::text(
                recoverable.message(),
            )])),
        }
    }
}

/// A malformed repo id is bad input; a well-formed but unknown one is
/// recoverable (pick a known id, or index the project first).
pub(crate) fn repo_id_error(e: RepoIdError) -> ToolError {
    match e {
        RepoIdError::Invalid { .. } => ToolError::invalid(e.to_string()),
        RepoIdError::Unknown { id, known } => {
            let next = if known.is_empty() {
                "run analyze with the project path to index it".to_string()
            } else {
                format!(
                    "use one of: {} — or run analyze with the project path",
                    known.join(", ")
                )
            };
            ToolError::recoverable(format!("unknown repo_id {id:?}: no index for it."), next)
        }
    }
}

/// Maps a [`myceliums_core::GraphService`] error onto the contract.
pub(crate) fn service_error(e: MyceliumError) -> ToolError {
    match e {
        MyceliumError::Repo(e) => repo_id_error(e),
        MyceliumError::SymbolNotFound(name) => ToolError::symbol_not_found(&name),
        MyceliumError::IndexChanged(repo_id) => ToolError::recoverable(
            format!("The index of {repo_id} was replaced while this call was reading it."),
            "retry the call; it reads the new index",
        ),
        other => ToolError::internal("graph read failed", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_becomes_an_is_error_result_with_a_next_step() {
        let result = ToolError::recoverable("Symbol `x` not found.", "use context_search")
            .into_call_tool_result()
            .expect("recoverable errors are tool results, not protocol errors");
        assert_eq!(result.is_error, Some(true));
        let text = result.content[0]
            .as_text()
            .expect("text content")
            .text
            .clone();
        assert_eq!(text, "Symbol `x` not found.\nNext: use context_search");
    }

    #[test]
    fn invalid_and_internal_stay_protocol_errors() {
        let invalid = ToolError::invalid("bad")
            .into_call_tool_result()
            .unwrap_err();
        assert_eq!(invalid.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        let internal = ToolError::internal("store", "disk on fire")
            .into_call_tool_result()
            .unwrap_err();
        assert_eq!(internal.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(internal.message, "store: disk on fire");
    }

    #[test]
    fn unknown_repo_id_is_recoverable_and_lists_known_ids() {
        let err = repo_id_error(RepoIdError::Unknown {
            id: "nope".into(),
            known: vec!["a".into(), "b".into()],
        });
        let msg = err.message();
        assert!(matches!(err, ToolError::Recoverable { .. }));
        assert!(msg.contains("unknown repo_id \"nope\""), "{msg}");
        assert!(msg.contains("Next: use one of: a, b"), "{msg}");
    }

    #[test]
    fn malformed_repo_id_is_invalid_params() {
        let err = repo_id_error(RepoIdError::Invalid {
            id: "../x".into(),
            reason: "path separator",
        });
        let ToolError::Protocol(e) = err else {
            panic!("malformed ids are invalid_params");
        };
        assert_eq!(e.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
