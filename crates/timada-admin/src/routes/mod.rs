pub(crate) mod catalog;
pub(crate) mod providers;

use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::AdminContext;
use crate::render::{AdminError, html};

const ADMIN_CSS: &str = include_str!("../../assets/admin.css");
const TWINSPARK_JS: &str = include_str!("../../assets/twinspark.min.js");

pub(crate) async fn admin_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        ADMIN_CSS,
    )
}

pub(crate) async fn twinspark_js() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        TWINSPARK_JS,
    )
}

struct ProviderInfo {
    kind: &'static str,
    name: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    base_path: String,
    providers: Vec<ProviderInfo>,
}

pub(crate) async fn dashboard(State(ctx): State<AdminContext>) -> Result<Response, AdminError> {
    let mut providers: Vec<ProviderInfo> = ctx
        .providers
        .iter()
        .map(|p| ProviderInfo {
            kind: p.kind(),
            name: p.display_name(),
        })
        .collect();
    providers.sort_by_key(|p| p.kind);

    html(&DashboardPage {
        base_path: ctx.base_path.clone(),
        providers,
    })
}
