//! `/t/{slug}/newsletter`: who asked for the shop's news, for members with
//! `newsletter:read`. An address asked from the storefront (its form, the
//! checkout) waits until its owner follows the link mailed to it; a shopper
//! signed in to their account is on the list at once. Those with
//! `newsletter:manage` take a subscriber off the list, and erase one who asks
//! to be forgotten (their key is destroyed, their row dropped). The list
//! comes from the shared read model (a moment behind) and opens on the
//! confirmed subscribers.

pub mod campaigns;
pub mod erase;
pub mod unsubscribe;

use timada_newsletter::{
    Command, NewsletterError, SubscriberFilter, SubscriberStatus, count_subscribers,
    subscribers_of_tenant,
};
use timada_tenancy::{PERMISSION_NEWSLETTER_MANAGE, PERMISSION_NEWSLETTER_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        href, page, query_params,
    },
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, notice, page_header, pager,
    section_card, status_badge,
};

const PAGE_SIZE: u32 = 50;
/// The `status` of the list showing every subscriber.
const ALL: &str = "all";

#[topcoat::router::query_params(error = bad_request)]
struct NewsletterQuery {
    /// `confirmed` (the default), `pending`, `unsubscribed` or `all`.
    status: Option<String>,
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct NewsletterLink<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

/// The newsletter page, with a flash error code or message.
pub fn newsletter_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The newsletter gateway in the tenant's streams (403 while suspended).
pub fn newsletter(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The word for where a subscriber stands.
pub fn status_label(status: SubscriberStatus) -> &'static str {
    match status {
        SubscriberStatus::Pending => "Waiting for confirmation",
        SubscriberStatus::Confirmed => "Subscribed",
        SubscriberStatus::Unsubscribed => "Unsubscribed",
    }
}

/// The word for where an address asked from.
fn source_label(source: &str) -> &'static str {
    match source {
        timada_newsletter::SOURCE_FORM => "Sign-up form",
        timada_newsletter::SOURCE_CHECKOUT => "Checkout",
        timada_newsletter::SOURCE_ACCOUNT => "Account",
        _ => "—",
    }
}

/// What a subscriber of the list is acted on with.
#[derive(serde::Deserialize)]
pub struct SubscriberForm {
    subscriber_id: String,
}

/// What every action on a subscriber shares: the member may manage the
/// list, and the outcome goes back to it.
async fn act<F>(cx: &Cx, form: SubscriberForm, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &Command<Executor>,
        &TenantAccess,
        &str,
    ) -> std::result::Result<(), NewsletterError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let gateway = newsletter(&access)?;
    match run(&gateway, &access, &form.subscriber_id).await {
        Ok(()) => Ok(see_other(newsletter_url(cx, &slug, None, Some(message)))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(newsletter_url(cx, &slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_READ)
        .await?;
    let query = query_params::<NewsletterQuery>(cx)?;
    let status = match query.status.as_deref() {
        Some(ALL) => None,
        Some(value) => Some(SubscriberStatus::parse(value).unwrap_or(SubscriberStatus::Confirmed)),
        None => Some(SubscriberStatus::Confirmed),
    };
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_NEWSLETTER_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = SubscriberFilter { status };
    let total = count_subscribers(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let confirmed = count_subscribers(
        pool,
        &tenant.id,
        &SubscriberFilter {
            status: Some(SubscriberStatus::Confirmed),
        },
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let rows = subscribers_of_tenant(
        pool,
        &tenant.id,
        &filter,
        PAGE_SIZE,
        (page_number - 1) * PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let status_value = status.map_or(ALL, SubscriberStatus::as_str);
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(NewsletterLink {
                    status: status_value,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Newsletter · {}", tenant.name);
    let subtitle = format!("{confirmed} subscriber(s) get the shop's news.");

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: "Newsletter",
                subtitle: Some(subtitle.as_str()),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(campaigns::index, Slug(slug.clone())))>"Campaigns"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                description: Some("The addresses that asked for the shop's news, from the storefront's sign-up form, at checkout or from their account. One asked from the form or at checkout counts once its owner follows the link mailed to it. Erase a subscriber who asks to be forgotten: their address is gone for good."),
                filter_bar(
                    <label class="sr-only" for="newsletter-status">"Status"</label>
                    select(
                        attrs: attributes! { id="newsletter-status" name="status" class="w-56" },
                        for option in [SubscriberStatus::Confirmed, SubscriberStatus::Pending, SubscriberStatus::Unsubscribed] {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                        <option value=(ALL) selected=(status.is_none())>"Every status"</option>
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Filter")
                )
                <p class="mb-4 text-sm text-muted-foreground" data-subscriber-count=(total)>(total) " subscriber(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("Address")
                            table_head("From")
                            table_head("Language")
                            table_head("Time zone")
                            table_head("Asked")
                            table_head("Status")
                            if can_manage {
                                table_head(attrs: attributes! { class="text-right" }, "Actions")
                            }
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: if can_manage { 7 } else { 6 }, text: "No subscriber here.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-subscriber=(&row.email) data-subscriber-status=(&row.status) },
                                table_cell(<a class=(LINK) href=(format!("mailto:{}", row.email))>(&row.email)</a>)
                                table_cell(attrs: attributes! { data-subscriber-source=(&row.source) }, (source_label(&row.source)))
                                table_cell((row.language.as_deref().unwrap_or("—")))
                                table_cell(attrs: attributes! { data-subscriber-time-zone="" }, (row.time_zone.as_deref().unwrap_or("Shop's")))
                                table_cell(<time datetime=(row.requested_at)>(format_time(row.requested_at as u64))</time>)
                                table_cell(status_badge(tone: subscriber_tone(row.status()), (status_label(row.status()))))
                                if can_manage {
                                    table_cell(
                                        <div class="flex justify-end gap-2">
                                            if row.status() != SubscriberStatus::Unsubscribed {
                                                <form method="post" action=(href!(unsubscribe::unsubscribe, Slug(slug.clone())))>
                                                    <input type="hidden" name="subscriber_id" value=(&row.id)>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Unsubscribe")
                                                </form>
                                            }
                                            <form method="post" action=(href!(erase::erase, Slug(slug.clone())))>
                                                <input type="hidden" name="subscriber_id" value=(&row.id)>
                                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Erase")
                                            </form>
                                        </div>
                                    )
                                }
                            )
                        }
                    )
                )
                pager(
                    page: i64::from(page_number),
                    pages: i64::from(pages),
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}

/// The colour of where a subscriber stands.
fn subscriber_tone(status: SubscriberStatus) -> Tone {
    match status {
        SubscriberStatus::Pending => Tone::Warning,
        SubscriberStatus::Confirmed => Tone::Success,
        SubscriberStatus::Unsubscribed => Tone::Neutral,
    }
}
