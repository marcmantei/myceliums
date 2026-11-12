//! Access control for the Streamable HTTP MCP transport (`myc mcp --http`).
//!
//! The HTTP transport serves every tool, including `analyze` on arbitrary
//! paths and `delete`. The rules:
//!
//! * **Loopback by default.** A non-loopback listen address is refused unless
//!   the caller opts in with `allow_remote` (`myc mcp --http ... --allow-remote`).
//! * **Remote needs a bearer token.** With `allow_remote`, every request must
//!   carry `Authorization: Bearer <token>`. The token comes from
//!   `MYC_MCP_TOKEN`; if that is unset, one is generated and printed once.
//!   On loopback, a token is enforced only when `MYC_MCP_TOKEN` is set.
//! * **Host check (loopback mode).** The `Host` header must name loopback,
//!   which defeats DNS rebinding.
//! * **Origin check (always).** As the MCP spec requires, a request whose
//!   `Origin` header is present and not a loopback origin is rejected.
//! * **No wildcard CORS.** Only loopback browser origins get CORS headers.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use myceliums_core::http_guard::{
    bearer_token, bind_addr_is_loopback, is_loopback_host, is_loopback_origin, token_matches,
};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Environment variable that holds the bearer token for the HTTP transport.
pub const TOKEN_ENV: &str = "MYC_MCP_TOKEN";

/// Shortest bearer token accepted for `--allow-remote`.
pub const MIN_REMOTE_TOKEN_CHARS: usize = 32;

/// How `myc mcp --http` was asked to listen.
#[derive(Clone, Default)]
pub struct McpHttpOptions {
    /// Listen address, e.g. `127.0.0.1:9999`.
    pub addr: String,
    /// Explicit opt-in to listen on a non-loopback address.
    pub allow_remote: bool,
    /// Bearer token (normally read from [`TOKEN_ENV`]). Empty means unset.
    pub token: Option<String>,
}

impl std::fmt::Debug for McpHttpOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHttpOptions")
            .field("addr", &self.addr)
            .field("allow_remote", &self.allow_remote)
            .field("token", &redacted(&self.token))
            .finish()
    }
}

/// The access policy derived from [`McpHttpOptions`].
#[derive(Clone)]
pub struct HttpSecurity {
    /// Listen address only reaches this machine; enforce a loopback `Host`.
    pub loopback_only: bool,
    /// Required bearer token, if any.
    pub token: Option<String>,
    /// The token was generated here and must be shown to the operator.
    pub token_generated: bool,
}

impl std::fmt::Debug for HttpSecurity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSecurity")
            .field("loopback_only", &self.loopback_only)
            .field("token", &redacted(&self.token))
            .field("token_generated", &self.token_generated)
            .finish()
    }
}

/// Debug view of a secret: whether it is set, never its value.
fn redacted(token: &Option<String>) -> &'static str {
    if token.is_some() {
        "Some(<redacted>)"
    } else {
        "None"
    }
}

impl HttpSecurity {
    /// Decides the policy for `opts`, or explains why the address is refused.
    ///
    /// The token is trimmed (a trailing newline from `$(cat file)` is not part
    /// of it); an empty token counts as unset. In remote mode a supplied token
    /// must have at least [`MIN_REMOTE_TOKEN_CHARS`] characters.
    pub fn plan(opts: &McpHttpOptions) -> anyhow::Result<Self> {
        let token = opts
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let loopback = bind_addr_is_loopback(&opts.addr).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid listen address {:?}: expected host:port, e.g. 127.0.0.1:9999",
                opts.addr
            )
        })?;
        if loopback {
            return Ok(Self {
                loopback_only: true,
                token,
                token_generated: false,
            });
        }
        if !opts.allow_remote {
            anyhow::bail!(
                "refusing to listen on non-loopback address {}: the MCP HTTP server exposes \
                 every tool (including `analyze` and `delete`). Use 127.0.0.1:<port>, or pass \
                 --allow-remote to accept remote clients with a bearer token ({} or a \
                 generated one).",
                opts.addr,
                TOKEN_ENV
            );
        }
        let (token, token_generated) = match token {
            Some(t) if t.chars().count() < MIN_REMOTE_TOKEN_CHARS => anyhow::bail!(
                "{TOKEN_ENV} is too short for --allow-remote: use at least \
                 {MIN_REMOTE_TOKEN_CHARS} characters (e.g. `openssl rand -hex 32`), or unset it \
                 to get a generated token"
            ),
            Some(t) => (t, false),
            None => (generate_token(), true),
        };
        Ok(Self {
            loopback_only: false,
            token: Some(token),
            token_generated,
        })
    }
}

/// Two v4 UUIDs from the OS RNG (244 random bits), as 64 hex characters.
fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn reject(status: StatusCode, message: &str) -> Response {
    let mut resp = (status, message.to_string()).into_response();
    if status == StatusCode::UNAUTHORIZED {
        resp.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    resp
}

/// Middleware enforcing the Host, Origin and bearer-token rules.
async fn guard(State(sec): State<Arc<HttpSecurity>>, req: Request, next: Next) -> Response {
    if sec.loopback_only {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned)
            .or_else(|| req.uri().authority().map(|a| a.to_string()));
        if !host.as_deref().is_some_and(is_loopback_host) {
            return reject(
                StatusCode::FORBIDDEN,
                "Forbidden: Host header must be localhost, 127.0.0.1 or [::1]",
            );
        }
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(is_loopback_origin) {
            return reject(
                StatusCode::FORBIDDEN,
                "Forbidden: Origin is not a loopback origin",
            );
        }
    }
    if let Some(expected) = sec.token.as_deref() {
        let provided = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(bearer_token);
        if !provided.is_some_and(|p| token_matches(p, expected)) {
            return reject(
                StatusCode::UNAUTHORIZED,
                "Unauthorized: send `Authorization: Bearer <token>` (see MYC_MCP_TOKEN)",
            );
        }
    }
    next.run(req).await
}

/// CORS for browser-based MCP clients on this machine (e.g. MCP Inspector).
fn loopback_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.to_str().is_ok_and(is_loopback_origin)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::AUTHORIZATION,
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([HeaderName::from_static("mcp-session-id")])
}

/// Wraps `inner` with the guard and the loopback-only CORS layer.
///
/// CORS is the outer layer so preflight requests (which never carry
/// credentials) are answered before the token check.
pub fn secure(inner: axum::Router, sec: Arc<HttpSecurity>) -> axum::Router {
    inner
        .layer(axum::middleware::from_fn_with_state(sec, guard))
        .layer(loopback_cors())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::post;
    use tower::ServiceExt;

    fn opts(addr: &str, allow_remote: bool, token: Option<&str>) -> McpHttpOptions {
        McpHttpOptions {
            addr: addr.to_string(),
            allow_remote,
            token: token.map(str::to_string),
        }
    }

    #[test]
    fn loopback_bind_needs_no_flag_and_token_is_optional() {
        let sec = HttpSecurity::plan(&opts("127.0.0.1:9999", false, None)).unwrap();
        assert!(sec.loopback_only);
        assert!(sec.token.is_none());
        let sec = HttpSecurity::plan(&opts("localhost:9999", false, Some("t"))).unwrap();
        assert_eq!(sec.token.as_deref(), Some("t"));
    }

    #[test]
    fn non_loopback_bind_requires_explicit_flag() {
        let err = HttpSecurity::plan(&opts("0.0.0.0:9999", false, Some("t")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--allow-remote"), "{err}");
    }

    const LONG: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn remote_bind_always_has_a_token() {
        let sec = HttpSecurity::plan(&opts("0.0.0.0:9999", true, Some(LONG))).unwrap();
        assert!(!sec.loopback_only);
        assert_eq!(sec.token.as_deref(), Some(LONG));
        assert!(!sec.token_generated);

        // Unset and empty tokens both lead to a generated one.
        for token in [None, Some(""), Some("  ")] {
            let sec = HttpSecurity::plan(&opts("[::]:9999", true, token)).unwrap();
            assert!(sec.token_generated);
            assert_eq!(sec.token.as_deref().map(str::len), Some(64));
        }
    }

    #[test]
    fn tokens_are_trimmed() {
        let padded = format!("  {LONG}\n");
        let sec = HttpSecurity::plan(&opts("0.0.0.0:9999", true, Some(&padded))).unwrap();
        assert_eq!(sec.token.as_deref(), Some(LONG));
        let sec = HttpSecurity::plan(&opts("127.0.0.1:9", false, Some(" t \n"))).unwrap();
        assert_eq!(sec.token.as_deref(), Some("t"));
    }

    #[test]
    fn remote_mode_rejects_short_tokens() {
        let err = HttpSecurity::plan(&opts("0.0.0.0:9999", true, Some("short")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("too short"), "{err}");
        assert!(
            !err.contains("short\""),
            "the token must not be echoed: {err}"
        );
    }

    #[test]
    fn debug_output_redacts_tokens() {
        let o = opts("0.0.0.0:9999", true, Some(LONG));
        let sec = HttpSecurity::plan(&o).unwrap();
        for dbg in [format!("{o:?}"), format!("{sec:?}")] {
            assert!(!dbg.contains(LONG), "{dbg}");
            assert!(dbg.contains("<redacted>"), "{dbg}");
        }
    }

    #[test]
    fn ipv4_mapped_addresses_are_classified_by_their_ipv4_part() {
        // `[::ffff:127.0.0.1]` is loopback; `[::ffff:0.0.0.0]` is not.
        assert!(
            HttpSecurity::plan(&opts("[::ffff:127.0.0.1]:9999", false, None))
                .unwrap()
                .loopback_only
        );
        assert!(HttpSecurity::plan(&opts("[::ffff:0.0.0.0]:9999", false, None)).is_err());
        assert!(HttpSecurity::plan(&opts("[::ffff:192.168.1.5]:9999", false, None)).is_err());
    }

    #[test]
    fn unparsable_address_is_an_error() {
        assert!(HttpSecurity::plan(&opts("9999", false, None)).is_err());
    }

    fn app(sec: HttpSecurity) -> axum::Router {
        let inner = axum::Router::new().route("/mcp", post(|| async { "ok" }));
        secure(inner, Arc::new(sec))
    }

    fn local(token: Option<&str>) -> HttpSecurity {
        HttpSecurity {
            loopback_only: true,
            token: token.map(str::to_string),
            token_generated: false,
        }
    }

    async fn status(app: &axum::Router, headers: &[(&str, &str)]) -> StatusCode {
        let mut req = Request::builder().method("POST").uri("/mcp");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn loopback_host_without_origin_passes() {
        let app = app(local(None));
        assert_eq!(
            status(&app, &[("host", "127.0.0.1:9999")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, &[("host", "localhost:9999")]).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn dns_rebinding_host_is_rejected() {
        let app = app(local(None));
        assert_eq!(
            status(&app, &[("host", "evil.example:9999")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn foreign_origin_is_rejected_loopback_origin_passes() {
        let app = app(local(None));
        assert_eq!(
            status(
                &app,
                &[
                    ("host", "127.0.0.1:9999"),
                    ("origin", "https://evil.example")
                ]
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(&app, &[("host", "127.0.0.1:9999"), ("origin", "null")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &app,
                &[
                    ("host", "127.0.0.1:9999"),
                    ("origin", "http://localhost:6274")
                ]
            )
            .await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn token_is_enforced_when_configured() {
        let app = app(local(Some("s3cret")));
        let host = ("host", "127.0.0.1:9999");
        assert_eq!(status(&app, &[host]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            status(&app, &[host, ("authorization", "Bearer wrong")]).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(&app, &[host, ("authorization", "Bearer s3cret")]).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn remote_mode_accepts_any_host_but_needs_token_and_checks_origin() {
        let app = app(HttpSecurity {
            loopback_only: false,
            token: Some("s3cret".into()),
            token_generated: false,
        });
        let host = ("host", "mcp.internal.example:9999");
        assert_eq!(status(&app, &[host]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            status(&app, &[host, ("authorization", "Bearer s3cret")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(
                &app,
                &[
                    host,
                    ("authorization", "Bearer s3cret"),
                    ("origin", "https://evil.example")
                ]
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn cors_is_granted_only_to_loopback_origins() {
        let app = app(local(None));
        let preflight = |origin: &'static str| {
            Request::builder()
                .method("OPTIONS")
                .uri("/mcp")
                .header("host", "127.0.0.1:9999")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap()
        };
        let resp = app
            .clone()
            .oneshot(preflight("http://localhost:6274"))
            .await
            .unwrap();
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://localhost:6274")
        );
        let resp = app
            .clone()
            .oneshot(preflight("https://evil.example"))
            .await
            .unwrap();
        assert!(resp.headers().get("access-control-allow-origin").is_none());
    }
}
