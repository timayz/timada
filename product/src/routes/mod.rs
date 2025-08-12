use askama::Template;
use axum::{
    Router,
    response::IntoResponse,
    routing::{get, post},
};

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
        .route("/", get(index))
        .route("/create", post(create))
        .route("/create/status", get(status))
}
