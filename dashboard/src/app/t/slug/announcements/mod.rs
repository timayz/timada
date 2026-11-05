//! `/t/{slug}/announcements`: the short messages the storefront's top bar
//! cycles through ("-10% maintenant avec le code BIENVENUE10"), for members
//! with `pages:read`; those with `pages:manage` add, edit, order, hide and
//! delete them. A message shows within its optional window, given in the
//! shop's time zone. Read from the tenant's own stream, so a change shows at
//! once here; the storefront follows a moment later.

pub mod add;
pub mod announcement_id;

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use serde::Deserialize;
use timada_pages::{
    Announcement, AnnouncementStatus, Command, MAX_ANNOUNCEMENT_LEN, MAX_ANNOUNCEMENTS,
    NewAnnouncement, PageError, render_announcement,
};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{Unescaped, View, attributes, component, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::ui::{
    Shell, Tone, document, empty_row, notice, page_header, section_card, status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct AnnouncementsQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The list, with a flash error code or message.
pub fn announcements_url(
    cx: &Cx,
    slug: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The pages gateway in the tenant's streams (403 while suspended).
pub fn pages(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The code of a refusal, for the flash.
pub fn error_code(err: &PageError) -> &'static str {
    err.code().unwrap_or("internal")
}

/// Now, in Unix seconds.
fn now() -> u64 {
    Timestamp::now().as_second().max(0) as u64
}

/// What a `datetime-local` field gives and takes.
const LOCAL_FORMAT: &str = "%Y-%m-%dT%H:%M";

fn zone(shop_zone: &str) -> TimeZone {
    TimeZone::get(shop_zone).unwrap_or(TimeZone::UTC)
}

/// A moment as a `datetime-local` field shows it, in the shop's time zone.
pub fn local_input(at: Option<u64>, shop_zone: &str) -> String {
    at.and_then(|at| Timestamp::from_second(at as i64).ok())
        .map(|at| {
            at.to_zoned(zone(shop_zone))
                .strftime(LOCAL_FORMAT)
                .to_string()
        })
        .unwrap_or_default()
}

/// A moment as the list shows it: `Tue 3 Nov 2026, 09:00`.
fn format_local(at: u64, shop_zone: &str) -> String {
    Timestamp::from_second(at as i64)
        .map(|at| {
            at.to_zoned(zone(shop_zone))
                .strftime("%a %-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// A `datetime-local` value in the shop's time zone; blank is none.
fn parse_local(value: &str, shop_zone: &str) -> std::result::Result<Option<u64>, PageError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    DateTime::strptime(LOCAL_FORMAT, value)
        .ok()
        .and_then(|local| local.to_zoned(zone(shop_zone)).ok())
        .map(|at| Some(at.timestamp().as_second().max(0) as u64))
        .ok_or(PageError::InvalidAnnouncementWindow)
}

/// A message as the merchant fills it in, to add or to change.
#[derive(Deserialize)]
pub struct AnnouncementForm {
    text: String,
    #[serde(default)]
    starts_at: String,
    #[serde(default)]
    ends_at: String,
}

impl AnnouncementForm {
    /// The message the form describes, its times read in the shop's zone.
    pub fn announcement(&self, shop_zone: &str) -> std::result::Result<NewAnnouncement, PageError> {
        Ok(NewAnnouncement {
            text: self.text.clone(),
            starts_at: parse_local(&self.starts_at, shop_zone)?,
            ends_at: parse_local(&self.ends_at, shop_zone)?,
        })
    }
}

/// When a message shows, in words.
pub fn describe_window(announcement: &Announcement, shop_zone: &str) -> String {
    match (announcement.starts_at, announcement.ends_at) {
        (None, None) => "Always".to_owned(),
        (Some(starts_at), None) => format!("From {}", format_local(starts_at, shop_zone)),
        (None, Some(ends_at)) => format!("Until {}", format_local(ends_at, shop_zone)),
        (Some(starts_at), Some(ends_at)) => format!(
            "{} to {}",
            format_local(starts_at, shop_zone),
            format_local(ends_at, shop_zone)
        ),
    }
}

/// Where a message stands now, in words, and its tone.
pub fn status_of(announcement: &Announcement) -> (&'static str, Tone) {
    match announcement.status(now()) {
        AnnouncementStatus::Live => ("Live", Tone::Success),
        AnnouncementStatus::Scheduled => ("Scheduled", Tone::Info),
        AnnouncementStatus::Ended => ("Ended", Tone::Neutral),
        AnnouncementStatus::Hidden => ("Hidden", Tone::Neutral),
    }
}

/// The text and window fields, shared by the add and edit forms.
#[component]
pub async fn announcement_fields(
    text: &str,
    starts_at: &str,
    ends_at: &str,
    shop_zone: &str,
) -> Result<impl View> {
    Ok(view! {
        field(
            field_label(attrs: attributes! { for="announcement-text" }, "Message")
            field_description(
                attrs: attributes! { id="announcement-text-hint" },
                <code>"**BIENVENUE10**"</code> " shows in bold; one link at most, to a page of the shop or an https address: "
                <code>"[Voir la boutique](/boutique)"</code> "."
            )
            input(attrs: attributes! {
                id="announcement-text" name="text" type="text" required="" maxlength=(MAX_ANNOUNCEMENT_LEN)
                aria-describedby="announcement-text-hint" placeholder="-10% maintenant avec le code **BIENVENUE10**"
                value=(text)
            })
        )
        <div class="grid gap-4 sm:grid-cols-2">
            field(
                field_label(attrs: attributes! { for="announcement-starts-at" }, "From (optional)")
                input(attrs: attributes! { id="announcement-starts-at" name="starts_at" type="datetime-local" value=(starts_at) })
            )
            field(
                field_label(attrs: attributes! { for="announcement-ends-at" }, "Until (optional)")
                input(attrs: attributes! { id="announcement-ends-at" name="ends_at" type="datetime-local" value=(ends_at) })
            )
        </div>
        <p class="text-sm text-muted-foreground">"Times are the shop's (" (shop_zone) "). Without them, the message shows until you hide it."</p>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<AnnouncementsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let announcements = pages(&access)?
        .announcements()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let shop_zone = tenant.shop_time_zone().to_owned();
    let title = format!("Announcements · {}", tenant.name);
    let last = announcements.len().saturating_sub(1);
    let full = announcements.len() >= MAX_ANNOUNCEMENTS;
    let columns = if can_manage { 5 } else { 3 };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "announcements"),
            page_header(
                title: "Announcements",
                subtitle: Some("The short messages at the top of every storefront page. Several live ones take turns, in this order."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                table(
                    table_header(
                        table_row(
                            table_head("Message")
                            table_head("When")
                            table_head("Status")
                            if can_manage {
                                table_head("Order")
                                table_head(<span class="sr-only">"Actions"</span>)
                            }
                        )
                    )
                    table_body(
                        if announcements.is_empty() {
                            empty_row(colspan: columns, text: "No announcement: the storefront shows no bar.")
                        }
                        for (position, announcement) in announcements.iter().enumerate() {
                            table_row(
                                attrs: attributes! { data-announcement=(&announcement.id) data-announcement-status=(status_of(announcement).0) },
                                table_cell(attrs: attributes! { class="whitespace-normal" data-announcement-text="" }, (Unescaped::new_unchecked(render_announcement(&announcement.text))))
                                table_cell(attrs: attributes! { class="whitespace-normal text-sm" }, (describe_window(announcement, &shop_zone)))
                                table_cell(status_badge(tone: status_of(announcement).1, (status_of(announcement).0)))
                                if can_manage {
                                    table_cell(
                                        <div class="flex gap-1">
                                            if position > 0 {
                                                order_button(slug: &slug, id: &announcement.id, position: position - 1, label: "Up")
                                            }
                                            if position < last {
                                                order_button(slug: &slug, id: &announcement.id, position: position + 1, label: "Down")
                                            }
                                        </div>
                                    )
                                    table_cell(
                                        <div class="flex justify-end gap-1">
                                            <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(announcement_id::index, Slug(slug.clone()), announcement_id::AnnouncementId(announcement.id.clone())))>"Edit"</a>
                                            <form method="post" action=(href!(announcement_id::show::show, Slug(slug.clone()), announcement_id::AnnouncementId(announcement.id.clone())))>
                                                <input type="hidden" name="shown" value=(if announcement.shown { "false" } else { "true" })>
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                                    if announcement.shown { "Hide" } else { "Show" })
                                            </form>
                                            <form method="post" action=(href!(announcement_id::remove::remove, Slug(slug.clone()), announcement_id::AnnouncementId(announcement.id.clone())))>
                                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete")
                                            </form>
                                        </div>
                                    )
                                }
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Add an announcement"),
                    if full {
                        <p class="text-sm text-muted-foreground">"The bar holds " (MAX_ANNOUNCEMENTS) " messages. Delete one to add another."</p>
                    } else {
                        <form method="post" action=(href!(add::add, Slug(slug.clone()))) class="flex max-w-2xl flex-col gap-4" data-announcement-form="">
                            announcement_fields(text: "", starts_at: "", ends_at: "", shop_zone: &shop_zone)
                            <div>button(attrs: attributes! { type="submit" }, "Add announcement")</div>
                        </form>
                    }
                )
            }
        )
    })
}

/// A button that moves a message to `position`.
#[component]
async fn order_button(slug: &str, id: &str, position: usize, label: &str) -> Result<impl View> {
    Ok(view! {
        <form method="post" action=(href!(announcement_id::move_to::move_to, Slug(slug.to_owned()), announcement_id::AnnouncementId(id.to_owned())))>
            <input type="hidden" name="position" value=(position)>
            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, (label))
        </form>
    })
}
