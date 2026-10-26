use std::sync::Arc;

use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{Back, LinkId, back_to};
use crate::aliexpress::{Aliexpress, SyncResult};
use crate::app::t::slug::require_tenant;

/// Reads the product again from AliExpress now.
#[route(POST)]
pub async fn sync(cx: &Cx, Form(form): Form<Back>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let store = access.executor()?.clone();
    let company = aliexpress
        .company_of(&access.tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let result = match company {
        Some(company) => aliexpress
            .sync_link(&store, &access.tenant, &company, &link_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => SyncResult::NotLinked,
    };
    Ok(match result {
        SyncResult::Synced { offline: true, .. } => back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("AliExpress no longer sells it: taken off sale."),
        ),
        SyncResult::Synced { .. } | SyncResult::Skipped => back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("Read again from AliExpress."),
        ),
        SyncResult::NotLinked => back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_not_connected"),
            None,
        ),
        SyncResult::Failed(_) => back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_sync_failed"),
            None,
        ),
    })
}
