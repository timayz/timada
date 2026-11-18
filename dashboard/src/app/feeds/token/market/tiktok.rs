//! `GET /feeds/{token}/{market}/tiktok`: a shop's products for one country
//! and language as a TikTok catalogue reads them.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::market;
use crate::app::feeds::token::products;

#[route(GET)]
pub async fn tiktok(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Tiktok, Some(market(cx))).await
}
