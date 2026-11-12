//! Mapping of domain errors onto MCP errors.

use myceliums_storage::RepoIdError;

/// A rejected repo id is a problem with the request, not with the server.
pub(crate) fn repo_id_error(e: RepoIdError) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params(e.to_string(), None)
}

/// Maps a [`GraphService`] error: a rejected repo id is `invalid_params`,
/// anything else stays an internal error.
pub(crate) fn service_error(e: myceliums_core::error::MyceliumError) -> rmcp::ErrorData {
    match e {
        myceliums_core::error::MyceliumError::Repo(e) => repo_id_error(e),
        other => rmcp::ErrorData::internal_error(other.to_string(), None),
    }
}
