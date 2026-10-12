//! `/admin/users`: everyone with an account.

pub mod user_id;

use timada_identity::{count_users, list_users};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::admin::require_admin;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct UsersQuery {
    /// Matches the account or the name.
    q: Option<String>,
    /// 1-based.
    page: Option<i64>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct UsersLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

pub fn users_url(cx: &Cx, message: Option<&str>) -> String {
    href!(users)
        .query(UsersLink {
            q: None,
            page: None,
            message,
        })
        .resolve(cx)
}

#[page]
pub async fn users(cx: &Cx) -> Result<impl View> {
    let me = require_admin(cx).await?;
    let query = query_params::<UsersQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let page_number = query.page.unwrap_or(1).max(1);
    let pool = app_context::<ReadPool>(cx);
    let total = count_users(pool, q).await?;
    let rows = list_users(pool, q, (page_number - 1) * PAGE_SIZE, PAGE_SIZE).await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let page_link = move |page: i64| {
        href!(users)
            .query(UsersLink {
                q,
                page: Some(page),
                message: None,
            })
            .resolve(cx)
    };

    Ok(view! {
        document(
            title: "Users",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Users"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                    <a class="underline" href=(href!(crate::app::auth::account::account))>"Account"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <form method="get" class="mb-4 flex gap-2" role="search">
                <label class="sr-only" for="users-q">"Search users"</label>
                <input id="users-q" class="w-full rounded border border-neutral-300 px-3 py-1" type="search" name="q" value=(q.unwrap_or("")) placeholder="Account or name">
                <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">"Search"</button>
            </form>
            <p class="mb-2 text-sm text-neutral-500">(total) " user" (if total == 1 { "" } else { "s" })</p>

            <table class="w-full text-left text-sm">
                <thead class="border-b border-neutral-300 text-neutral-500">
                    <tr>
                        <th class="py-2 pr-3 font-medium">"Name"</th>
                        <th class="py-2 pr-3 font-medium">"Account"</th>
                        <th class="py-2 pr-3 font-medium">"Role"</th>
                        <th class="py-2 pr-3 font-medium">"Sign-in methods"</th>
                        <th class="py-2 font-medium">"Last sign-in"</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-100">
                    for row in &rows {
                        <tr class=(if row.deleted { "text-neutral-400" } else { "" })>
                            <td class="py-2 pr-3">
                                <a class="underline" href=(href!(user_id::user, user_id::UserId(row.id.clone())))>(&row.label)</a>
                                if row.id == me.id { " " <span class="text-neutral-500">"(you)"</span> }
                                if row.deleted { " " <span class="rounded bg-neutral-200 px-1 text-xs text-neutral-700">"deleted"</span> }
                            </td>
                            <td class="py-2 pr-3">(&row.account)</td>
                            <td class="py-2 pr-3">(if row.is_admin { "Administrator" } else { "Member" })</td>
                            <td class="py-2 pr-3">(if row.has_password { "password, " } else { "" }) (row.oauth2_count) " provider" (if row.oauth2_count == 1 { "" } else { "s" }) ", " (row.passkey_count) " passkey" (if row.passkey_count == 1 { "" } else { "s" })</td>
                            <td class="py-2">
                                if let Some(at) = row.last_sign_in_at {
                                    <time datetime=(at)>(format_time(at as u64))</time>
                                } else {
                                    "never"
                                }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>

            if pages > 1 {
                <nav class="mt-4 flex gap-3 text-sm" aria-label="Pages">
                    if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                    <span>"Page " (page_number) " of " (pages)</span>
                    if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                </nav>
            }
        )
    })
}
