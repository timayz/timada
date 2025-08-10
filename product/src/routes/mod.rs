use askama::Template;
use axum::{Router, response::IntoResponse, routing::get};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index(html: shared::Template<IndexTemplate>) -> impl IntoResponse {
    html.template(IndexTemplate)
}

pub fn create_router() -> Router {
    Router::new().route("/", get(index))
}
