//! `GET /feeds/{token}/meta`: a shop's products as a Meta catalogue reads
//! them (Facebook and Instagram catalogue ads, retargeting), for a
//! scheduled fetch. The same items as Merchant Center's, in Meta's words;
//! answers 404 as that feed does.

use timada_catalog::FeedReader;
use topcoat::{Result, context::Cx, router::response::IntoResponse, router::route};

use super::products;

#[route(GET)]
pub async fn meta(cx: &Cx) -> Result<impl IntoResponse> {
    products(cx, FeedReader::Meta, None).await
}
