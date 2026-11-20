//! Request guards that keep the loopback-only dashboard out of reach of web pages.

use axum::{
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Rejects requests that a web page could have forged.
///
/// The dashboard listens on loopback, but a page in the user's browser can
/// still reach it: via DNS rebinding (the browser then sends the attacker's
/// host name in `Host`) or with a direct cross-origin request (the browser
/// sends the page's `Origin`). Both are refused here, which keeps endpoints
/// such as `POST /api/llm/config` out of reach. Requests from the dashboard
/// itself and from CLI tools pass.
pub(crate) async fn local_only(request: Request<axum::body::Body>, next: Next) -> Response {
    use myceliums_core::http_guard::{is_loopback_host, is_loopback_origin};

    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(is_loopback_host);
    if !host_ok {
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: open the dashboard via localhost or 127.0.0.1",
        )
            .into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(is_loopback_origin) {
            return (
                StatusCode::FORBIDDEN,
                "Forbidden: cross-origin requests are not allowed",
            )
                .into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::routes::build_router;
    use crate::test_support::{app_state, test_data_dir};

    async fn dashboard_status(method: &str, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
        use tower::ServiceExt;
        let dir = test_data_dir("guard");
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        build_router(app_state(dir))
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn dashboard_rejects_foreign_host_headers() {
        // DNS rebinding: the browser sends the attacker's host name.
        assert_eq!(
            dashboard_status(
                "POST",
                "/api/llm/config",
                &[
                    ("host", "evil.example:8888"),
                    ("content-type", "application/json")
                ]
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            dashboard_status("GET", "/favicon.ico", &[]).await,
            StatusCode::FORBIDDEN,
            "a request without Host is not trusted either"
        );
    }

    #[tokio::test]
    async fn dashboard_rejects_cross_origin_requests() {
        assert_eq!(
            dashboard_status(
                "POST",
                "/api/llm/config",
                &[
                    ("host", "127.0.0.1:8888"),
                    ("origin", "https://evil.example"),
                    ("content-type", "application/json")
                ]
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn dashboard_serves_loopback_requests() {
        for host in ["localhost:8888", "127.0.0.1:8888", "[::1]:8888"] {
            assert_eq!(
                dashboard_status("GET", "/favicon.ico", &[("host", host)]).await,
                StatusCode::OK,
                "{host}"
            );
        }
        assert_eq!(
            dashboard_status(
                "GET",
                "/favicon.ico",
                &[
                    ("host", "localhost:8888"),
                    ("origin", "http://localhost:8888")
                ]
            )
            .await,
            StatusCode::OK
        );
    }
}
