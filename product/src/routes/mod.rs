use askama::Template;
use axum::{Router, response::IntoResponse, routing::get};

use crate::html_template::HtmlTemplate;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index() -> impl IntoResponse {
    let template = IndexTemplate;
    HtmlTemplate(template)
}

pub fn create_router() -> Router {
    Router::new().route("/", get(index))
}
