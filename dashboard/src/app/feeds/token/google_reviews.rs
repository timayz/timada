//! `GET /feeds/{token}/google-reviews`: a shop's published reviews as
//! Google's product reviews feed reads them, so its Shopping listings show
//! their stars. Whole each time; answers 404 as the shopping feed does.

use timada_catalog::feed_tenant;
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
use crate::feed::{reviews_feed, shop_of};

#[route(GET)]
pub async fn google_reviews(cx: &Cx) -> Result<impl IntoResponse> {
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
    Ok((
        [
            (CONTENT_TYPE, "application/xml; charset=utf-8"),
            // The address is a secret, and the answer changes with the
            // catalog: nothing between the shop and its reader keeps it.
            (CACHE_CONTROL, "private, no-store"),
        ],
        reviews_feed(db.clone(), tenant.id, shop),
    ))
}
