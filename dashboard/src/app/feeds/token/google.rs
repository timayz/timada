//! `GET /feeds/{token}/google`: a shop's products as Google Merchant Center
//! reads them, for a scheduled fetch. Any node answers, from its own copy
//! of the catalog (a moment behind the back office).
//!
//! Answers 404 for an address no open feed has, for a suspended shop, and
//! for a shop that has not said where its storefront is.
use std::sync::Arc;

use timada_catalog::{FeedReader, feed_tenant};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::not_found,
        header::{CACHE_CONTROL, CONTENT_TYPE},
        response::IntoResponse,
        route,
    },
};

use super::token;
use crate::app::tenants::tenancy;
use crate::cluster::Executor;
use crate::db::Db;
use crate::feed::{products_feed, shop_of};
use crate::media::Media;

#[route(GET)]
pub async fn google(cx: &Cx) -> Result<impl IntoResponse> {
    let db = context::<Db>(cx);
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
    let shop = shop_of(context::<Executor>(cx), db, &tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(not_found)?;
    let media = context::<Arc<Media>>(cx).clone();
    Ok((
        [
            (CONTENT_TYPE, "application/xml; charset=utf-8"),
            // The address is a secret, and the answer changes with the
            // catalog: nothing between the shop and its reader keeps it.
            (CACHE_CONTROL, "private, no-store"),
        ],
        products_feed(FeedReader::Google, db.clone(), media, tenant.id, shop),
    ))
}
