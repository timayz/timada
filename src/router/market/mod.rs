use crate::{axum_extra::Template, filters};
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Form,
};
use market::product::{CreateInput, Product, ProductState, QueryProduct};
use shared::Metadata;

#[derive(askama::Template)]
#[template(path = "market/index.html")]
pub struct IndexTemplate {
    pub log: Option<(String, ProductState)>,
    pub products: evento::cursor::ReadResult<QueryProduct>,
}

pub async fn index(
    html: Template<IndexTemplate>,
    State(state): State<crate::State>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    let products = market::product::query_products(&state.lmdb)?;
    Ok(html.template(IndexTemplate {
        log: None,
        products,
    }))
}

#[axum::debug_handler]
pub async fn create(
    html: Template<IndexTemplate>,
    State(state): State<crate::State>,
    metadata: Metadata,
    Form(input): Form<CreateInput>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    let id = market::product::create(input)?
        .metadata(&metadata)?
        .routing_key(state.config.region)
        .commit(&state.market_executor)
        .await?;

    Ok(html.template(IndexTemplate {
        log: Some((id, ProductState::Checking)),
        products: Default::default(),
    }))
}

pub async fn status(
    html: Template<IndexTemplate>,
    State(state): State<crate::State>,
    Path((id,)): Path<(String,)>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    let product = evento::load::<Product, _>(&state.market_executor, &id).await?;

    Ok(html.template(IndexTemplate {
        log: Some((id, product.item.state)),
        products: Default::default(),
    }))
}
