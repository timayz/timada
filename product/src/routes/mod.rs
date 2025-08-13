use askama::Template;
use axum::{
    Form, Router,
    extract::Path,
    response::IntoResponse,
    routing::{get, post},
};
use shared::Metadata;

use crate::{
    filters,
    product::{CreateInput, Product, ProductState},
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    pub log: Option<(String, ProductState)>,
}

async fn index(html: shared::Template<IndexTemplate>) -> impl IntoResponse {
    html.template(IndexTemplate { log: None })
}

async fn create(
    html: shared::Template<IndexTemplate>,
    state: crate::State,
    metadata: Metadata,
    Form(input): Form<CreateInput>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    let id = crate::product::create(input)?
        .metadata(&metadata)?
        .routing_key(state.region)
        .commit(&state.executor)
        .await?;

    Ok(html.template(IndexTemplate {
        log: Some((id, ProductState::Checking)),
    }))
}

async fn status(
    html: shared::Template<IndexTemplate>,
    state: crate::State,
    Path((id,)): Path<(String,)>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    let product = liteventd::load::<Product, _>(&state.executor, &id).await?;

    Ok(html.template(IndexTemplate {
        log: Some((id, product.item.state)),
    }))
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
