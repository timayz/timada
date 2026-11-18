//! `GET /feeds/{token}/google`: a shop's products as Google Merchant Center
//! reads them, for a scheduled fetch. Any node answers, from its own copy
//! of the catalog (a moment behind the back office).
//!
//! Answers 404 for an address no open feed has, for a suspended shop, and
//! for a shop that has not said where its storefront is.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::products;

#[route(GET)]
pub async fn google(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Google, None).await
}
