//! `/t/{slug}/messages`: what visitors sent through the shop's contact form,
//! for members with `support:read`; those with `support:manage` mark a
//! message handled once they answered it (by mail, from the address it
//! carries), reopen one marked too soon, and erase one whose sender asks to
//! be forgotten (its key is destroyed, its row dropped). The list comes from
//! the shared read model (a moment behind) and opens on the messages waiting.

pub mod forget;
pub mod handle;
pub mod reopen;

use timada_support::{
    Command, MessageFilter, MessageStatus, SupportError, count_messages, messages_of_tenant,
};
use timada_tenancy::{PERMISSION_SUPPORT_MANAGE, PERMISSION_SUPPORT_READ};
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
use crate::components::button::{ButtonSize, ButtonVariant, button};
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
/// The `status` of the list showing every message.
const ALL: &str = "all";

#[topcoat::router::query_params(error = bad_request)]
struct MessagesQuery {
    /// `open` (the default), `handled` or `all`.
    status: Option<String>,
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct MessagesLink<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

/// The messages page, with a flash error code or message.
pub fn messages_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The support gateway in the tenant's streams (403 while suspended).
pub fn support(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The word for where a message stands.
pub fn status_label(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Open => "Waiting",
        MessageStatus::Handled => "Handled",
    }
}

fn status_of(row_status: &str) -> &'static str {
    MessageStatus::parse(row_status).map_or("Unknown", status_label)
}

/// What a message of the list is acted on with.
#[derive(serde::Deserialize)]
pub struct MessageForm {
    message_id: String,
}

/// What every action on a message shares: the member may manage messages,
/// and the outcome goes back to the list.
async fn act<F>(cx: &Cx, form: MessageForm, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &Command<Executor>,
        &TenantAccess,
        &str,
    ) -> std::result::Result<(), SupportError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SUPPORT_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let gateway = support(&access)?;
    match run(&gateway, &access, &form.message_id).await {
        Ok(()) => Ok(see_other(messages_url(cx, &slug, None, Some(message)))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(messages_url(cx, &slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SUPPORT_READ)
        .await?;
    let query = query_params::<MessagesQuery>(cx)?;
    let status = match query.status.as_deref() {
        Some(ALL) => None,
        Some(value) => Some(MessageStatus::parse(value).unwrap_or_default()),
        None => Some(MessageStatus::Open),
    };
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_SUPPORT_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = MessageFilter { status };
    let total = count_messages(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = messages_of_tenant(
        pool,
        &tenant.id,
        &filter,
        PAGE_SIZE,
        (page_number - 1) * PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let status_value = status.map_or(ALL, MessageStatus::as_str);
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(MessagesLink {
                    status: status_value,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Messages · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "messages"),
            page_header(title: "Messages", subtitle: Some("What visitors sent through the shop's contact form."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                description: Some("What visitors wrote in the shop's contact form, with the address to answer at. Answer from your own mailbox, then mark the message handled here. Erase a message when its sender asks to be forgotten: what they wrote is gone for good."),
                filter_bar(
                    <label class="sr-only" for="messages-status">"Status"</label>
                    select(
                        attrs: attributes! { id="messages-status" name="status" class="w-44" },
                        for option in MessageStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                        <option value=(ALL) selected=(status.is_none())>"Every status"</option>
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Filter")
                )
                <p class="mb-4 text-sm text-muted-foreground" data-message-count=(total)>(total) " message(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("From")
                            table_head("Message")
                            table_head("Language")
                            table_head("Received")
                            table_head("Status")
                            if can_manage {
                                table_head(attrs: attributes! { class="text-right" }, "Actions")
                            }
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: if can_manage { 6 } else { 5 }, text: "No message here.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-message=(&row.id) data-message-status=(&row.status) },
                                table_cell(
                                    attrs: attributes! { class="align-top" },
                                    <span class="block font-medium">(row.full_name())</span>
                                    <a class=(LINK) href=(format!("mailto:{}", row.email))>(&row.email)</a>
                                )
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal align-top" },
                                    <details>
                                        <summary class="cursor-pointer">(excerpt(&row.body))</summary>
                                        <p class="mt-2 whitespace-pre-line" data-message-body="">(&row.body)</p>
                                    </details>
                                )
                                table_cell(attrs: attributes! { class="align-top" data-message-language=(row.language.as_deref().unwrap_or_default()) }, (row.language.as_deref().unwrap_or("—")))
                                table_cell(attrs: attributes! { class="align-top" }, <time datetime=(row.received_at)>(format_time(row.received_at as u64))</time>)
                                table_cell(attrs: attributes! { class="align-top" }, status_badge(tone: message_tone(&row.status), (status_of(&row.status))))
                                if can_manage {
                                    table_cell(
                                        attrs: attributes! { class="align-top" },
                                        <div class="flex justify-end gap-2">
                                            if row.status == MessageStatus::Handled.as_str() {
                                                <form method="post" action=(href!(reopen::reopen, Slug(slug.clone())))>
                                                    <input type="hidden" name="message_id" value=(&row.id)>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Reopen")
                                                </form>
                                            } else {
                                                <form method="post" action=(href!(handle::handle, Slug(slug.clone())))>
                                                    <input type="hidden" name="message_id" value=(&row.id)>
                                                    button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Mark handled")
                                                </form>
                                            }
                                            <form method="post" action=(href!(forget::forget, Slug(slug.clone())))>
                                                <input type="hidden" name="message_id" value=(&row.id)>
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

/// The colour of a message's stored status.
fn message_tone(status: &str) -> Tone {
    match MessageStatus::parse(status) {
        Some(MessageStatus::Open) => Tone::Warning,
        Some(MessageStatus::Handled) => Tone::Success,
        None => Tone::Neutral,
    }
}

/// The start of a message, for its row.
fn excerpt(body: &str) -> String {
    const LEN: usize = 60;
    let first_line = body.lines().next().unwrap_or_default();
    if first_line.chars().count() <= LEN && first_line.len() == body.trim_end().len() {
        return first_line.to_owned();
    }
    let cut: String = first_line.chars().take(LEN).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::excerpt;

    #[test]
    fn an_excerpt_is_the_start_of_the_first_line() {
        assert_eq!(excerpt("Where is my order?"), "Where is my order?");
        assert_eq!(
            excerpt("Where is my order?\nThanks."),
            "Where is my order?…"
        );
        assert_eq!(excerpt(&"x".repeat(70)), format!("{}…", "x".repeat(60)));
    }
}
