//! Static assets: the single-page dashboard and its favicon.

use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse},
};

const VISUALIZATION_HTML: &str = include_str!("../visualization.html");

pub(crate) async fn index_handler() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-cache, no-store, must-revalidate"),
            ("pragma", "no-cache"),
            ("expires", "0"),
        ],
        Html(VISUALIZATION_HTML),
    )
}

pub(crate) async fn favicon_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" fill="#e94560"/></svg>"##,
    )
}
