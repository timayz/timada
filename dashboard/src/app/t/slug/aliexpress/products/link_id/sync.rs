use std::sync::Arc;
use std::time::Duration;

use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{Back, LinkId, back_to};
use crate::aliexpress::{Aliexpress, SyncMode, SyncResult};
use crate::app::t::slug::require_tenant;
use crate::db::Db;

/// How long the page waits for the read; past that, the merchant is told it
/// is on its way, and it carries on.
const READ_WAIT: Duration = Duration::from_secs(10);

/// Reads the product again from AliExpress now, on the node serving the
/// page, and waits for the read so the page shows what it found. A member's
/// read is no request on the item's stream (#472): its outcome is recorded
/// in their name, whatever it is.
#[route(POST)]
pub async fn sync(cx: &Cx, Form(form): Form<Back>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let not_connected = |cx: &Cx| {
        back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_not_connected"),
            None,
        )
    };
    let aliexpress = app_context::<Arc<Aliexpress>>(cx).clone();
    let store = access.executor()?.clone();
    let db = app_context::<Db>(cx).clone();
    let Some((tenant, company)) = aliexpress
        .connected_shop(&store, &access.tenant.id, crate::auth::now())
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(not_connected(cx));
    };
    let member = access.user_id.clone();
    let read = tokio::spawn(async move {
        aliexpress
            .sync_link(
                &db,
                &store,
                &tenant,
                &company,
                &link_id,
                SyncMode::Asked { by: &member },
            )
            .await
    });
    let read = match tokio::time::timeout(READ_WAIT, read).await {
        Ok(Ok(read)) => Some(read.map_err(topcoat::Error::from_anyhow)?),
        Ok(Err(stopped)) => return Err(topcoat::Error::from_anyhow(stopped.into())),
        Err(_) => None,
    };
    let (error, message) = match read {
        None => (None, Some("Reading it again from AliExpress.")),
        Some(SyncResult::NotLinked) => return Ok(not_connected(cx)),
        Some(SyncResult::Unlinked) => (
            None,
            Some("The product is archived: no longer kept in step with AliExpress."),
        ),
        Some(SyncResult::Failed(_)) => (Some("aliexpress_sync_failed"), None),
        Some(SyncResult::Synced { offline: true, .. }) => {
            (None, Some("AliExpress no longer sells it: taken off sale."))
        }
        Some(SyncResult::Synced { .. } | SyncResult::Skipped) => {
            (None, Some("Read again from AliExpress."))
        }
    };
    Ok(back_to(cx, &access, &form.back, error, message))
}
