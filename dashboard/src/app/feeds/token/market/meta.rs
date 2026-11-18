//! `GET /feeds/{token}/{market}/meta`: a shop's products for one country
//! and language as a Meta catalogue reads them.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::market;
use crate::app::feeds::token::products;

#[route(GET)]
pub async fn meta(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Meta, Some(market(cx))).await
}
