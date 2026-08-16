//! Timada shared web plumbing.
//!
//! - Base storefront and admin layouts (`layout.html`, `admin/layout.html`),
//!   resolvable from service crates via their `askama.toml`
//!   (`dirs = ["templates", "../web/templates"]`).
//! - Embedded static assets (Tailwind-built CSS, vendored TwinSpark) served by
//!   [`asset_router`].
//! - [`HtmlTemplate`], an axum responder for any Askama template.

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// Render an Askama template as a `text/html` response.
pub struct HtmlTemplate<T>(pub T);

impl<T> IntoResponse for HtmlTemplate<T>
where
    T: askama::Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(body) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                body,
            )
                .into_response(),
            Err(source) => {
                tracing::error!(error = ?source, "template rendering failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

static APP_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css"));
static TWINSPARK_JS: &str = include_str!("../assets/twinspark.min.js");

/// Serves the embedded framework assets under `/assets/*`.
pub fn asset_router() -> Router {
    Router::new()
        .route(
            "/assets/app.css",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                        (header::CACHE_CONTROL, "public, max-age=3600"),
                    ],
                    APP_CSS,
                )
            }),
        )
        .route(
            "/assets/twinspark.js",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, "public, max-age=3600"),
                    ],
                    TWINSPARK_JS,
                )
            }),
        )
}
