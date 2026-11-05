//! `/t/{slug}/announcements/{announcement_id}`: one message, with the form
//! that rewrites it and its window. The buttons of the list post to the
//! actions under it.

pub mod change;
pub mod move_to;
pub mod remove;
pub mod show;

use timada_pages::{Announcement, PageError, render_announcement};
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{Unescaped, View, attributes, view},
};

use super::{
    announcement_fields, announcements_url, describe_window, error_code, local_input, pages,
    status_of,
};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::ui::{Shell, document, notice, page_header, section_card, status_badge};

module_param!(pub announcement_id);

#[topcoat::router::query_params(error = bad_request)]
struct AnnouncementQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn announcement_url(
    cx: &Cx,
    slug: &str,
    announcement_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        AnnouncementId(announcement_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The message the URL names, or 404.
async fn require_announcement(cx: &Cx, access: &TenantAccess) -> Result<Announcement> {
    let id = path_param::<AnnouncementId>(cx);
    pages(access)?
        .announcements()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .into_iter()
        .find(|announcement| announcement.id == *id)
        .ok_or_else(|| not_found().into())
}

/// What every action on a message shares: the member may manage pages, and
/// it lands back on the list, with `message` or the code of the refusal.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_pages::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), PageError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let announcement_id = path_param::<AnnouncementId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = pages(&access)?;
    match run(&gateway, &access.user_id, &announcement_id).await {
        Ok(()) => Ok(see_other(announcements_url(cx, &slug, None, Some(message)))),
        Err(err) if err.is_user_facing() => Ok(see_other(announcements_url(
            cx,
            &slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let query = query_params::<AnnouncementQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let announcement = require_announcement(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let shop_zone = tenant.shop_time_zone().to_owned();
    let title = format!("Announcement · {}", tenant.name);
    let back = announcements_url(cx, &slug, None, None);
    let (status, tone) = status_of(&announcement);
    let window = describe_window(&announcement, &shop_zone);
    let starts_at = local_input(announcement.starts_at, &shop_zone);
    let ends_at = local_input(announcement.ends_at, &shop_zone);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "announcements"),
            page_header(
                title: "Announcement",
                subtitle: Some(window.as_str()),
                status_badge(tone: tone, attrs: attributes! { data-announcement-status=(status) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"All announcements"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("As the bar shows it"),
                <p class="rounded-lg border border-border px-4 py-2 text-center text-sm" data-announcement-preview="">
                    (Unescaped::new_unchecked(render_announcement(&announcement.text)))
                </p>
            )

            section_card(
                title: Some("Message"),
                <form method="post" action=(href!(change::change, Slug(slug.clone()), AnnouncementId(announcement.id.clone()))) class="flex max-w-2xl flex-col gap-4" data-announcement-form="">
                    announcement_fields(
                        text: &announcement.text,
                        starts_at: &starts_at,
                        ends_at: &ends_at,
                        shop_zone: &shop_zone,
                    )
                    <div>button(attrs: attributes! { type="submit" }, "Save")</div>
                </form>
            )
        )
    })
}
