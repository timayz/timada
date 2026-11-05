//! `POST /t/{slug}/announcements/{announcement_id}/change`: rewrites the
//! message and its window. A refusal stays on the message's page.

use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::{AnnouncementForm, announcements_url, error_code, pages};
use super::{AnnouncementId, announcement_url};
use crate::app::t::slug::require_tenant;

#[route(POST)]
pub async fn change(cx: &Cx, Form(form): Form<AnnouncementForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let announcement_id = path_param::<AnnouncementId>(cx).to_owned();
    let slug = access.tenant.slug.as_str();
    let changed = match form.announcement(access.tenant.shop_time_zone()) {
        Ok(announcement) => {
            pages(&access)?
                .change_announcement(&access.user_id, &announcement_id, announcement)
                .await
        }
        Err(err) => Err(err),
    };
    match changed {
        Ok(()) => Ok(see_other(announcements_url(
            cx,
            slug,
            None,
            Some("Announcement saved: the storefront shows it in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(announcement_url(
            cx,
            slug,
            &announcement_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
