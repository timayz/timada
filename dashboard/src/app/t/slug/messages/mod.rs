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
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

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

/// The support gateway over the tenant's own store (403 while suspended).
pub fn support(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
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
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Messages"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "messages")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "What visitors wrote in the shop's contact form, with the address to answer at. Answer from your own mailbox, then mark the message handled here. Erase a message when its sender asks to be forgotten: what they wrote is gone for good."
                </p>
                <form method="get" class="mb-4 flex flex-wrap gap-2">
                    <label class="sr-only" for="messages-status">"Status"</label>
                    <select id="messages-status" name="status" class="rounded border border-neutral-300 px-3 py-2">
                        for option in MessageStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                        <option value=(ALL) selected=(status.is_none())>"Every status"</option>
                    </select>
                    <button class="rounded border border-neutral-300 px-3 py-2">"Filter"</button>
                </form>
                <p class="mb-4 text-sm" data-message-count=(total)>(total) " message(s)"</p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"From"</th>
                            <th class="py-2 font-medium">"Message"</th>
                            <th class="py-2 font-medium">"Language"</th>
                            <th class="py-2 font-medium">"Received"</th>
                            <th class="py-2 font-medium">"Status"</th>
                            if can_manage {
                                <th class="py-2 text-right font-medium">"Actions"</th>
                            }
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan=(if can_manage { 6 } else { 5 })>"No message here."</td></tr>
                        }
                        for row in &rows {
                            <tr data-message=(&row.id) data-message-status=(&row.status)>
                                <td class="py-2 align-top">
                                    (row.full_name())
                                    <br>
                                    <a class="underline" href=(format!("mailto:{}", row.email))>(&row.email)</a>
                                </td>
                                <td class="py-2 align-top">
                                    <details>
                                        <summary class="cursor-pointer">(excerpt(&row.body))</summary>
                                        <p class="mt-2 whitespace-pre-line" data-message-body="">(&row.body)</p>
                                    </details>
                                </td>
                                <td class="py-2 align-top" data-message-language=(row.language.as_deref().unwrap_or_default())>(row.language.as_deref().unwrap_or("—"))</td>
                                <td class="py-2 align-top"><time datetime=(row.received_at)>(format_time(row.received_at as u64))</time></td>
                                <td class="py-2 align-top">(status_of(&row.status))</td>
                                if can_manage {
                                    <td class="py-2 align-top">
                                      <div class="flex justify-end gap-2">
                                        if row.status == MessageStatus::Handled.as_str() {
                                            <form method="post" action=(href!(reopen::reopen, Slug(slug.clone())))>
                                                <input type="hidden" name="message_id" value=(&row.id)>
                                                <button class="rounded border border-neutral-300 px-2 py-1 text-xs hover:bg-neutral-50">"Reopen"</button>
                                            </form>
                                        } else {
                                            <form method="post" action=(href!(handle::handle, Slug(slug.clone())))>
                                                <input type="hidden" name="message_id" value=(&row.id)>
                                                <button class="rounded border border-neutral-900 px-2 py-1 text-xs hover:bg-neutral-50">"Mark handled"</button>
                                            </form>
                                        }
                                        <form method="post" action=(href!(forget::forget, Slug(slug.clone())))>
                                            <input type="hidden" name="message_id" value=(&row.id)>
                                            <button class="rounded border border-red-700 px-2 py-1 text-xs text-red-700 hover:bg-red-50">"Erase"</button>
                                        </form>
                                      </div>
                                    </td>
                                }
                            </tr>
                        }
                    </tbody>
                </table>
                if pages > 1 {
                    <p class="mt-4 flex items-center gap-4 text-sm">
                        if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                        <span>"Page " (page_number) " of " (pages)</span>
                        if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                    </p>
                }
            </section>
        )
    })
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
