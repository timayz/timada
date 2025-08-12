use askama::Template;
use axum::{response::IntoResponse, routing::get, Router};

use crate::{assets, filters};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index(template: shared::Template<IndexTemplate>) -> impl IntoResponse {
    template.template(IndexTemplate)
}

pub fn create_router() -> Router {
    Router::new()
        .fallback(get(assets::static_handler))
        .route("/", get(index))
        .merge(product::create_router())
}
