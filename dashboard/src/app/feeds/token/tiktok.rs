//! `GET /feeds/{token}/tiktok`: a shop's products as a TikTok catalogue
//! reads them, for a scheduled fetch. The same items as Merchant Center's,
//! in TikTok's words; answers 404 as that feed does.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::products;

#[route(GET)]
pub async fn tiktok(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Tiktok, None).await
}
