//! One shop's feed, named by the secret its address ends with.

pub mod google;
pub mod google_reviews;
pub mod market;
pub mod meta;
pub mod tiktok;

use std::sync::Arc;

use timada_catalog::{FeedReader, feed_tenant};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::not_found,
        header::{CACHE_CONTROL, CONTENT_TYPE},
        module_param,
        response::IntoResponse,
    },
};

use crate::app::tenants::tenancy;
use crate::cluster::Executor;
use crate::db::Db;
use crate::feed::{Market, in_market, products_feed, shop_of};
use crate::media::Media;

module_param!(pub token);

/// The shop's products as `reader` takes them: the whole shop in its main
/// language, or, for a `market` (`be-fr`), what it sells in that country
/// in that language.
///
/// Answers 404 for an address no open feed has, for a suspended shop, for a
/// shop that has not said where its storefront is, and for a market the
/// shop does not sell in.
pub async fn products(
    cx: &Cx,
    reader: FeedReader,
    market: Option<&str>,
) -> Result<impl IntoResponse + use<>> {
    let db = context::<Db>(cx);
    let store = context::<Executor>(cx);
    let tenant_id = feed_tenant(db, token(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(not_found)?;
    let tenant = tenancy(cx)
        .load_tenant(&tenant_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|tenant| !tenant.is_suspended())
        .ok_or_else(not_found)?;
    let shop = shop_of(store, db, &tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(not_found)?;
    let (shop, wording) = match market {
        Some(code) => {
            let market = Market::parse(code).ok_or_else(not_found)?;
            in_market(store, db, &tenant, shop, &market)
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .ok_or_else(not_found)?
        }
        None => (shop, None),
    };
    let media = context::<Arc<Media>>(cx).clone();
    Ok((
        [
            (CONTENT_TYPE, "application/xml; charset=utf-8"),
            // The address is a secret, and the answer changes with the
            // catalog: nothing between the shop and its reader keeps it.
            (CACHE_CONTROL, "private, no-store"),
        ],
        products_feed(reader, db.clone(), media, tenant.id, shop, wording),
    ))
}
