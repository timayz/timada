//! `GET /feeds/{token}/{market}/google`: a shop's products for one country
//! and language as Google Merchant Center reads them.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::market;
use crate::app::feeds::token::products;

#[route(GET)]
pub async fn google(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Google, Some(market(cx))).await
}
