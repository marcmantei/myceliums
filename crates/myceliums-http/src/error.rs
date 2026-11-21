//! The dashboard API's error contract (#118): a status code and a JSON body
//! `{"error": "...", "next": "..."}` — what went wrong, and what to do about
//! it. A request that fails never gets a `200` with empty or partial data.
//!
//! | Cause | Status |
//! |-------|--------|
//! | malformed request parameter or body (`tests=bogus`, `top_k=abc`, a missing `q`, invalid JSON) | `400` |
//! | unknown `repo_id` (`next` lists the known ids) | `404` |
//! | registered repository without an index | `404` |
//! | symbol / node not in the index | `404` |
//! | index without embeddings, for semantic search | `409` |
//! | index replaced while the request read it | `503`, `Retry-After: 1` |
//! | storage, I/O or computation failure | `500` |

use crate::state::AppState;
use axum::{
    extract::{FromRequest, FromRequestParts, Query, Request},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use myceliums_core::MyceliumError;
use myceliums_storage::{RepoIdError, StorageError};
use serde::{de::DeserializeOwned, Serialize};

/// A failed API request: its status and the `{error, next}` body.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    error: String,
    next: String,
}

/// The JSON body of every API error.
#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    next: &'a str,
}

impl ApiError {
    pub(crate) fn new(
        status: StatusCode,
        error: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error: error.into(),
            next: next.into(),
        }
    }

    /// A request parameter the server cannot use.
    pub(crate) fn bad_request(error: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error, next)
    }

    /// A storage, I/O or computation failure while answering for `repo`.
    pub(crate) fn internal(what: &str, cause: impl std::fmt::Display, repo: &Repo<'_>) -> Self {
        tracing::error!("{what} for {}: {cause}", repo.id);
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{what}: {cause}"),
            format!(
                "check the `myc serve` log; if the index is damaged, rebuild it with `myc analyze {}`",
                repo.analyze_target()
            ),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: &self.error,
            next: &self.next,
        });
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            (self.status, [(header::RETRY_AFTER, "1")], body).into_response()
        } else {
            (self.status, body).into_response()
        }
    }
}

/// [`Query`] whose rejection (a malformed or missing parameter) is a `400`
/// in the error contract instead of axum's plain-text answer.
pub(crate) struct ApiQuery<T>(pub(crate) T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| ApiQuery(value))
            .map_err(|e| {
                ApiError::bad_request(
                    e.body_text(),
                    "fix the query string; the parameters are listed in the `myc serve` section of docs/reference/commands.md",
                )
            })
    }
}

/// [`Json`] whose rejection (not JSON, wrong content type, missing or
/// mistyped fields) is a `400` in the error contract.
pub(crate) struct ApiJson<T>(pub(crate) T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| ApiJson(value))
            .map_err(|e| {
                ApiError::bad_request(
                    e.body_text(),
                    "send a JSON body with Content-Type: application/json, e.g. {\"query\": \"...\", \"limit\": 20}",
                )
            })
    }
}

/// The repository a request names, for mapping its errors: the `next` step
/// names the repository's path when the registry knows it.
pub(crate) struct Repo<'a> {
    state: &'a AppState,
    id: &'a str,
}

impl<'a> Repo<'a> {
    pub(crate) fn new(state: &'a AppState, id: &'a str) -> Self {
        Self { state, id }
    }

    /// What to pass to `myc analyze`: the registered project path, else a
    /// placeholder.
    fn analyze_target(&self) -> String {
        self.state
            .graph
            .repo_info(self.id)
            .map(|info| info.path)
            .unwrap_or_else(|_| "<path>".to_string())
    }

    /// Maps a [`myceliums_core::GraphService`] (or store) error onto the
    /// contract.
    pub(crate) fn error(&self, e: impl Into<MyceliumError>) -> ApiError {
        match e.into() {
            MyceliumError::Repo(e) => repo_id_error(e),
            MyceliumError::NotAnalyzed(msg) => ApiError::new(
                StatusCode::NOT_FOUND,
                msg,
                format!("run `myc analyze {}` to index it", self.analyze_target()),
            ),
            // Registered, but its index directory is gone: never built,
            // deleted, or moved aside by a rebuild that failed.
            MyceliumError::Store(StorageError::NoIndex(_)) => ApiError::new(
                StatusCode::NOT_FOUND,
                format!(
                    "repository {:?} is registered but has no index on disk",
                    self.id
                ),
                format!("run `myc analyze {}` to build it", self.analyze_target()),
            ),
            MyceliumError::IndexChanged(id) => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("the index of {id} was replaced while this request was reading it"),
                "retry the request; it reads the new index",
            ),
            MyceliumError::SymbolNotFound(name) => ApiError::new(
                StatusCode::NOT_FOUND,
                format!("symbol {name:?} not found"),
                "search for it with GET /api/v1/graph/search/{repo_id}?q=...",
            ),
            other => ApiError::internal("cannot read the index", other, self),
        }
    }

    /// A symbol or node id that is not in this repository's index.
    pub(crate) fn symbol_not_found(&self, uid: &str) -> ApiError {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no symbol with id {uid:?} in repository {:?}", self.id),
            format!(
                "ids come from GET /api/repos/{}/graph or the search endpoints; \
                 re-run `myc analyze {}` if the code changed",
                self.id,
                self.analyze_target()
            ),
        )
    }
}

/// An unknown repository id lists the known ones; a malformed one can name
/// no repository either. Both are `404`: the URL names nothing.
fn repo_id_error(e: RepoIdError) -> ApiError {
    let next = match &e {
        RepoIdError::Unknown { known, .. } if !known.is_empty() => format!(
            "use one of the registered repo ids: {} (GET /api/repos lists them), \
             or index the project with `myc analyze <path>`",
            known.join(", ")
        ),
        RepoIdError::Unknown { .. } => {
            "no repository is indexed yet: run `myc analyze <path>`".to_string()
        }
        RepoIdError::Invalid { .. } => "use a repo id from GET /api/repos".to_string(),
    };
    ApiError::new(StatusCode::NOT_FOUND, e.to_string(), next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        app_state, corrupt_seeded_store, error_body, register_repo, request, seed_healthy_store,
        test_data_dir,
    };
    use serde_json::json;
    use tower::ServiceExt;

    /// Every repository-scoped endpoint, as `(method, path after the repo
    /// id is substituted for `{r}`, JSON body)`.
    fn endpoints() -> Vec<(&'static str, &'static str, Option<serde_json::Value>)> {
        vec![
            ("GET", "/api/repos/{r}/graph", None),
            ("GET", "/api/repos/{r}/symbol/node", None),
            ("GET", "/api/repos/{r}/stats", None),
            ("GET", "/api/repos/{r}/processes", None),
            (
                "POST",
                "/api/repos/{r}/knowledge",
                Some(json!({"query": "node"})),
            ),
            ("GET", "/api/repos/{r}/centrality", None),
            ("GET", "/api/repos/{r}/community-metrics", None),
            ("GET", "/api/repos/{r}/cycles", None),
            ("GET", "/api/repos/{r}/module-coupling", None),
            ("GET", "/api/v1/graph/overview/{r}", None),
            ("GET", "/api/v1/graph/nodes/{r}/node", None),
            ("GET", "/api/v1/graph/search/{r}?q=node", None),
        ]
    }

    /// #118: an unknown repo id is a `404` whose body says so and lists the
    /// registered ids — on every endpoint, never an empty body.
    #[tokio::test]
    async fn unknown_repo_is_404_with_the_known_ids() {
        let dir = test_data_dir("contract_unknown");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        register_repo(&dir, "known-1");
        register_repo(&dir, "known-2");
        let state = app_state(dir.clone());
        for (method, path, body) in endpoints() {
            let uri = path.replace("{r}", "made-up");
            let (status, _, json) = request(state.clone(), method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {json}");
            let (error, next) = error_body(&json);
            assert!(
                error.contains("unknown repo_id \"made-up\""),
                "{uri}: {error}"
            );
            assert!(next.contains("known-1, known-2"), "{uri}: {next}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With nothing registered, the way out is to index a project.
    #[tokio::test]
    async fn unknown_repo_without_any_registered_says_to_analyze() {
        let dir = test_data_dir("contract_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (status, _, json) =
            request(app_state(dir.clone()), "GET", "/api/repos/nope/graph", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, next) = error_body(&json);
        assert!(next.contains("myc analyze <path>"), "{next}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A registered repository whose index is missing is a `404` that names
    /// the command rebuilding it, with the registered path.
    #[tokio::test]
    async fn registered_repo_without_index_is_404_with_the_analyze_command() {
        let dir = test_data_dir("contract_no_index");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        register_repo(&dir, "known-1");
        let state = app_state(dir.clone());
        for (method, path, body) in endpoints() {
            let uri = path.replace("{r}", "known-1");
            let (status, _, json) = request(state.clone(), method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {json}");
            let (error, next) = error_body(&json);
            assert!(error.contains("no index"), "{uri}: {error}");
            assert_eq!(next, "run `myc analyze /nonexistent/known-1` to build it");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #118: a storage failure is a `500` with the message, not a `200` with
    /// the tables that happened to load (or none).
    #[tokio::test]
    async fn storage_failure_is_500_with_the_message() {
        let dir = test_data_dir("contract_corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        corrupt_seeded_store(&dir, "broken", "node").await;
        let state = app_state(dir.clone());
        for (method, path, body) in endpoints() {
            let uri = path.replace("{r}", "broken");
            let (status, _, json) = request(state.clone(), method, &uri, body).await;
            assert_eq!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{method} {uri}: {json}"
            );
            let (error, next) = error_body(&json);
            assert!(error.contains(':'), "says what failed and why: {error}");
            assert!(next.contains("myc analyze /nonexistent/broken"), "{next}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An id that is not in a healthy index is a `404` with a body.
    #[tokio::test]
    async fn unknown_symbol_is_404_with_a_body() {
        let dir = test_data_dir("contract_symbol");
        let _ = std::fs::remove_dir_all(&dir);
        seed_healthy_store(&dir, "healthy", "node").await;
        let state = app_state(dir.clone());
        for uri in [
            "/api/repos/healthy/symbol/ghost",
            "/api/v1/graph/nodes/healthy/ghost",
        ] {
            let (status, _, json) = request(state.clone(), "GET", uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {json}");
            let (error, _) = error_body(&json);
            assert!(error.contains("\"ghost\""), "{error}");
        }
        let (status, _, json) = request(state, "GET", "/api/repos/healthy/symbol/node", None).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["id"], "node");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A re-analysis that replaced the index mid-request is a `503` the
    /// client may retry at once.
    #[tokio::test]
    async fn index_changed_is_503_with_retry_after() {
        let dir = test_data_dir("contract_changed");
        let state = app_state(dir);
        let resp = Repo::new(&state, "r")
            .error(MyceliumError::IndexChanged("r".into()))
            .into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "1");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let (error, next) = error_body(&json);
        assert!(error.contains("replaced"), "{error}");
        assert!(next.starts_with("retry"), "{next}");
    }

    /// #118: malformed parameters and bodies are `400`s in the contract,
    /// not axum's plain-text rejections, and are refused before the index
    /// is read (the repository here does not even exist).
    #[tokio::test]
    async fn malformed_parameters_are_400_with_the_contract() {
        let dir = test_data_dir("contract_bad_params");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = app_state(dir.clone());
        let cases: Vec<(&str, &str, Option<serde_json::Value>)> = vec![
            ("GET", "/api/v1/graph/overview/r?page=abc", None),
            ("GET", "/api/v1/graph/overview/r?per_page=-1", None),
            ("GET", "/api/v1/graph/search/r", None),
            ("GET", "/api/v1/graph/search/r?q=x&top_k=many", None),
            ("GET", "/api/v1/graph/search/r?q=x&tests=bogus", None),
            ("GET", "/api/v1/graph/nodes/r/n?tests=bogus", None),
            (
                "GET",
                "/api/repos/r/module-coupling?group_by_dir=maybe",
                None,
            ),
            ("GET", "/api/repos/r/centrality?top_n=abc", None),
            ("GET", "/api/repos/r/centrality?metric=pagerank", None),
            ("POST", "/api/repos/r/knowledge", Some(json!({"limit": 5}))),
            ("POST", "/api/repos/r/knowledge", Some(json!({"query": 1}))),
            (
                "POST",
                "/api/repos/r/knowledge?tests=bogus",
                Some(json!({"query": "x"})),
            ),
        ];
        for (method, uri, body) in cases {
            let (status, _, json) = request(state.clone(), method, uri, body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {json}");
            error_body(&json);
        }
        // A body that is not JSON at all, or has no JSON content type.
        for content_type in ["application/json", "text/plain"] {
            let resp = crate::routes::build_router(state.clone())
                .oneshot(
                    axum::http::Request::post("/api/repos/r/knowledge")
                        .header("host", "127.0.0.1:8888")
                        .header("content-type", content_type)
                        .body(axum::body::Body::from("{ not json"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{content_type}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            error_body(&serde_json::from_slice(&bytes).unwrap());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A registry that cannot be read is a `500` with the message.
    #[tokio::test]
    async fn unreadable_registry_is_500() {
        let dir = test_data_dir("contract_registry");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("repos.json"), "{ not json").unwrap();
        let (status, _, json) = request(app_state(dir.clone()), "GET", "/api/repos", None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{json}");
        let (error, _) = error_body(&json);
        assert!(error.contains("registry"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
