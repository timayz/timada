use askama::Template;
use axum::{response::IntoResponse, routing::get, Router};

use crate::{assets, html_template::HtmlTemplate};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index() -> impl IntoResponse {
    let template = IndexTemplate;
    HtmlTemplate(template)
}

pub fn create_router() -> Router {
    Router::new()
        .fallback(get(assets::static_handler))
        .route("/", get(index))
        .nest(
            "/product",
            product::create_router().fallback(get(not_found)),
        )
}

#[derive(Template)]
#[template(path = "404.html")]
struct NotFoundTemplate;

async fn not_found() -> impl IntoResponse {
    let template = NotFoundTemplate;
    HtmlTemplate(template)
}
