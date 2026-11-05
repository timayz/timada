use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{AnnouncementForm, announcements_url, error_code, pages};
use crate::app::t::slug::require_tenant;

/// Adds a message, shown, last in the bar, and lands back on the list.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<AnnouncementForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let added = match form.announcement(access.tenant.shop_time_zone()) {
        Ok(announcement) => {
            pages(&access)?
                .add_announcement(&access.user_id, announcement)
                .await
        }
        Err(err) => Err(err),
    };
    match added {
        Ok(_) => Ok(see_other(announcements_url(
            cx,
            slug,
            None,
            Some("Announcement added: the storefront shows it in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(announcements_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
