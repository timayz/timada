use askama::Template;
use axum::{
    Router,
    response::IntoResponse,
    routing::{get, post},
};

use crate::filters;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    pub log: Option<String>,
}

async fn index(html: shared::Template<IndexTemplate>) -> impl IntoResponse {
    html.template(IndexTemplate { log: None })
}

async fn create(html: shared::Template<IndexTemplate>) -> impl IntoResponse {
    html.template(IndexTemplate {
        log: Some("creating...".to_owned()),
    })
}

async fn status(html: shared::Template<IndexTemplate>) -> impl IntoResponse {
    html.template(IndexTemplate { log: None })
}

pub fn create_router() -> Router {
    Router::new()
        .route(INDEX, get(index))
        .route(S_CREATE, post(create))
        .route(&s_create_status(None), get(status))
}

pub const INDEX: &str = "/product";
pub const S_CREATE: &str = "/product/-/create";

pub fn s_create_status(id: Option<String>) -> String {
    format!(
        "/product/-/create-status/{}",
        id.unwrap_or("{id}".to_owned())
    )
}
