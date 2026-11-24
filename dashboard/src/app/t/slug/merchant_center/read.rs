use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::{SeeOther, see_other},
        href, route,
    },
};

use crate::app::auth::Flash;
use crate::app::t::slug::google::google;
use crate::app::t::slug::{require_tenant, slug};
use crate::cluster::Executor;
use crate::config::Config;
use crate::google::Google;
use crate::merchant_passes::{Read, read_shop};

/// Reads Merchant Center for the shop now, for a member who does not want
/// to wait for the daily pass: after a product was put right, say. Turned
/// down when it was read a moment ago: a read takes its questions from the
/// account's daily allowance at Google.
#[route(POST)]
pub async fn read(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(
            href!(super::index, slug: access.tenant.slug.clone())
                .query(Flash { error, message })
                .resolve(cx),
        ))
    };
    let watch = google(cx)
        .load_merchant_watch(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let wait = context::<Config>(cx).google.merchant_ask_again_secs;
    let now = crate::auth::now();
    // A read that failed may be asked again at once.
    if watch.is_some_and(|watch| watch.last_error.is_none() && now < watch.checked_at + wait) {
        return back(Some("google_merchant_just_read"), None);
    }
    let done = read_shop(
        context::<Google>(cx),
        context::<Executor>(cx),
        access.tenant_id(),
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    match done {
        Read::Done { products, .. } => {
            let message = format!("Merchant Center read: {products} products.");
            back(None, Some(&message))
        }
        Read::Nothing => back(Some("google_merchant_nothing_to_read"), None),
        // The page says why, from the watch.
        Read::Failed(_) => back(None, None),
    }
}
