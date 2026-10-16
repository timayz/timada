//! `/t/{slug}/offers/{offer_id}/…`: the actions on one offer. Each comes
//! back to the list, which shows every offer whole.

pub mod archive;
pub mod deactivate;
pub mod reactivate;

use timada_tenancy::PERMISSION_DISCOUNTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        module_param, path_param,
    },
};

use super::offers_url;
use crate::app::t::slug::discounts::{discounts, error_code};
use crate::app::t::slug::require_tenant;

module_param!(pub offer_id);

/// What every action on an offer shares: the member may manage discounts,
/// and the outcome goes back to the list.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_discounts::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_discounts::DiscountError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_MANAGE)
        .await?;
    let offer_id = path_param::<OfferId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = discounts(&access)?;
    match run(&gateway, &access.user_id, &offer_id).await {
        Ok(()) => Ok(see_other(offers_url(cx, &slug, None, Some(message)))),
        Err(err) if err.is_user_facing() => Ok(see_other(offers_url(
            cx,
            &slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
