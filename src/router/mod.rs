mod market;

use axum::{
    response::IntoResponse,
    routing::{get, post},
    Router,
};

use crate::{assets, axum_extra::Template, filters};

#[derive(askama::Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index(template: Template<IndexTemplate>) -> impl IntoResponse {
    template.template(IndexTemplate)
}

pub fn create_router() -> Router<crate::State> {
    Router::new()
        .fallback(get(assets::static_handler))
        .route("/", get(index))
        .route(MARKET_INDEX, get(market::index))
        .route(MARKET_S_CREATE, post(market::create))
        .route(&market_s_create_status(None), get(market::status))
}

pub const MARKET_INDEX: &str = "/market";
pub const MARKET_S_CREATE: &str = "/market/-/create";

pub fn market_s_create_status(id: Option<String>) -> String {
    format!(
        "/market/-/create-status/{}",
        id.unwrap_or("{id}".to_owned())
    )
}
