//! The MCP server: `ServerHandler` (info + resources) and the stdio and
//! Streamable HTTP transports.

use crate::repo::registry_path;
use crate::{http_security, McpHttpOptions, MyceliumsMcp};
use anyhow::Result;
use myceliums_storage::RepoRegistry;
use rmcp::model::*;
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt};
use tracing::info;

impl ServerHandler for MyceliumsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(
            "Myceliums is a code knowledge graph that indexes the current codebase.\n\n\
             WHEN TO USE MYCELIUMS (always try first for these tasks):\n\
             - Understanding the project: 'What does this codebase do?' → context_search + get_processes\n\
             - Finding code: 'Where is authentication handled?' → context_search (not grep)\n\
             - Understanding dependencies: 'What calls this function?' → symbol_context\n\
             - Before modifying code: 'What will break if I change X?' → detect_impact\n\
             - Reviewing changes: 'Summarize what this diff affects' → get_review_context\n\
             - Architecture questions: 'How does data flow from API to DB?' → get_processes\n\
             - Structural queries: 'Find all classes that implement Y' → cypher_query\n\n\
             WHEN TO USE grep/read INSTEAD:\n\
             - Reading specific file contents or line ranges\n\
             - Searching inside string literals, comments, or config files\n\
             - Editing or writing files (myceliums is read-only)\n\n\
             RULE: For any question about code structure, symbols, or architecture — use myceliums first. \
             It returns structured results with exact locations, types, and relationships in one call \
             instead of multiple grep+read rounds."
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let mut resources = vec![Annotated::new(
            RawResource::new("repository://list", "Repository List")
                .with_description("List all analyzed repositories")
                .with_mime_type("application/json"),
            None,
        )];
        for repo in registry.list() {
            resources.push(Annotated::new(
                RawResource::new(
                    format!("repository://{}/schema", repo.id),
                    format!("{} - Schema", repo.name),
                )
                .with_description(format!("Schema and stats for {}", repo.name))
                .with_mime_type("application/json"),
                None,
            ));
            resources.push(Annotated::new(
                RawResource::new(
                    format!("repository://{}/map", repo.id),
                    format!("{} - Map", repo.name),
                )
                .with_description(format!("High-level community map for {}", repo.name))
                .with_mime_type("application/json"),
                None,
            ));
        }
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        let templates = vec![
            Annotated::new(
                RawResourceTemplate::new("repository://{repo_id}/schema", "Repository Schema")
                    .with_description("Schema and statistics for a specific repository")
                    .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new("repository://{repo_id}/map", "Repository Map")
                    .with_description("High-level community map of a repository")
                    .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new(
                    "repository://{repo_id}/community/{community_id}",
                    "Community Detail",
                )
                .with_description("Details of a specific community within a repository")
                .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new(
                    "repository://{repo_id}/process/{process_id}",
                    "Process Detail",
                )
                .with_description("Details of a specific execution process within a repository")
                .with_mime_type("application/json"),
                None,
            ),
        ];
        Ok(ListResourceTemplatesResult {
            resource_templates: templates,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let text = self.read_resource_inner(&request.uri).await?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            text,
            &request.uri,
        )
        .with_mime_type("application/json")]))
    }
}

pub async fn run_mcp_server() -> Result<()> {
    info!("Starting Myceliums MCP server");
    let server = MyceliumsMcp::new();

    // Build router with tool routes from the #[tool_router] macro
    let router =
        rmcp::handler::server::router::Router::new(server).with_tools(MyceliumsMcp::tool_router());

    let transport = rmcp::transport::io::stdio();
    let running = router
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve error: {}", e))?;
    running.waiting().await?;
    Ok(())
}

/// Start the MCP server over Streamable HTTP transport.
///
/// This uses rmcp's native `StreamableHttpService` which implements the
/// MCP Streamable HTTP transport specification (SSE-based).
/// Clients connect via `POST /mcp` with JSON-RPC messages.
///
/// Access control follows [`http_security`]: loopback unless
/// `opts.allow_remote`, a bearer token for remote use, Host and Origin
/// validation, and CORS for loopback origins only. A refused configuration
/// is returned as an error before anything binds.
pub async fn run_mcp_http_server(opts: McpHttpOptions) -> Result<()> {
    use http_security::HttpSecurity;

    let security = HttpSecurity::plan(&opts)?;
    let addr = opts.addr.as_str();
    info!("Starting Myceliums MCP HTTP server on {}", addr);
    if !security.loopback_only {
        eprintln!(
            "WARNING: MCP HTTP server accepts remote clients on {addr}. Every request needs \
             `Authorization: Bearer <token>`; the transport is plain HTTP, so put TLS in front \
             of it on untrusted networks."
        );
    }
    if security.token_generated {
        if let Some(token) = security.token.as_deref() {
            eprintln!(
                "Generated bearer token (set {} to choose your own):\n  {token}",
                http_security::TOKEN_ENV
            );
        }
    } else if security.token.is_some() {
        eprintln!("Bearer token required (from {}).", http_security::TOKEN_ENV);
    }

    let app = mcp_http_app(security);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("MCP server listening on http://{}/mcp", addr);
    eprintln!("MCP server listening on http://{}/mcp", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

/// The complete HTTP app served by [`run_mcp_http_server`]: rmcp's
/// Streamable HTTP service at `/mcp`, wrapped by the access-control layers.
fn mcp_http_app(security: http_security::HttpSecurity) -> axum::Router {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let ct = CancellationToken::new();

    let mut config = StreamableHttpServerConfig::default();
    config.stateful_mode = true;
    config.json_response = false;
    config.sse_keep_alive = Some(std::time::Duration::from_secs(15));
    config.cancellation_token = ct.child_token();
    if !security.loopback_only {
        // Remote clients reach us under names we can not know in advance; the
        // bearer token protects this mode instead of the Host allow-list.
        config = config.disable_allowed_hosts();
    }

    // One graph service for all sessions: each session gets its own server
    // value, but they share the store handles and the cached graphs.
    let graphs = Arc::new(myceliums_core::GraphService::new(crate::repo::data_dir()));
    let service: StreamableHttpService<
        rmcp::handler::server::router::Router<MyceliumsMcp>,
        LocalSessionManager,
    > = StreamableHttpService::new(
        move || {
            let server = MyceliumsMcp::with_service(graphs.clone());
            let router = rmcp::handler::server::router::Router::new(server)
                .with_tools(MyceliumsMcp::tool_router());
            Ok(router)
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    http_security::secure(
        axum::Router::new().nest_service("/mcp", service),
        Arc::new(security),
    )
}

#[cfg(test)]
mod tests {
    // ── HTTP transport: the real app, not a stub router ───────────────

    mod http_app {
        use super::super::mcp_http_app;
        use crate::http_security::HttpSecurity;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        const TOKEN: &str = "0123456789abcdef0123456789abcdef";

        fn app(loopback_only: bool, token: Option<&str>) -> axum::Router {
            mcp_http_app(HttpSecurity {
                loopback_only,
                token: token.map(str::to_string),
                token_generated: false,
            })
        }

        async fn send(app: &axum::Router, method: &str, headers: &[(&str, &str)]) -> StatusCode {
            let mut req = Request::builder().method(method).uri("/mcp");
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            let body = if method == "POST" {
                Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
                )
            } else {
                Body::empty()
            };
            app.clone()
                .oneshot(req.body(body).unwrap())
                .await
                .unwrap()
                .status()
        }

        const JSON: [(&str, &str); 2] = [
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ];

        #[tokio::test]
        async fn every_method_is_guarded_against_rebinding_and_foreign_origins() {
            let app = app(true, None);
            for method in ["GET", "POST", "DELETE"] {
                let mut h = vec![("host", "evil.example:9999")];
                h.extend(JSON);
                assert_eq!(
                    send(&app, method, &h).await,
                    StatusCode::FORBIDDEN,
                    "{method}"
                );

                let mut h = vec![
                    ("host", "127.0.0.1:9999"),
                    ("origin", "https://evil.example"),
                ];
                h.extend(JSON);
                assert_eq!(
                    send(&app, method, &h).await,
                    StatusCode::FORBIDDEN,
                    "{method}"
                );
            }
        }

        #[tokio::test]
        async fn every_method_needs_the_token_when_one_is_set() {
            let app = app(false, Some(TOKEN));
            for method in ["GET", "POST", "DELETE"] {
                let mut h = vec![("host", "mcp.example:9999")];
                h.extend(JSON);
                assert_eq!(
                    send(&app, method, &h).await,
                    StatusCode::UNAUTHORIZED,
                    "{method}"
                );
            }
        }

        #[tokio::test]
        async fn guarded_requests_reach_rmcp() {
            let app = app(false, Some(TOKEN));
            let auth = format!("Bearer {TOKEN}");
            let mut h = vec![
                ("host", "mcp.example:9999"),
                ("authorization", auth.as_str()),
            ];
            h.extend(JSON);
            // `initialize` succeeds end to end through the guard.
            assert_eq!(send(&app, "POST", &h).await, StatusCode::OK);
            // GET/DELETE without a session pass the guard; rmcp answers them.
            for method in ["GET", "DELETE"] {
                let status = send(&app, method, &h).await;
                assert!(
                    status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                    "{method}: {status}"
                );
            }
        }
    }
}
