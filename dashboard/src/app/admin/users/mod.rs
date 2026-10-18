//! `/admin/users`: everyone with an account.

pub mod user_id;

use timada_identity::{count_users, list_users};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::admin::require_admin;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, notice, page_header, pager,
    section_card, status_badge,
};

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

/// "Administrator" or "Member".
pub fn role_label(is_admin: bool) -> &'static str {
    if is_admin { "Administrator" } else { "Member" }
}

/// "password, 2 providers, 1 passkey": how the account signs in.
fn methods_label(has_password: bool, oauth2_count: i64, passkey_count: i64) -> String {
    format!(
        "{}{oauth2_count} provider{}, {passkey_count} passkey{}",
        if has_password { "password, " } else { "" },
        if oauth2_count == 1 { "" } else { "s" },
        if passkey_count == 1 { "" } else { "s" },
    )
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
    let summary = format!("{total} user{}", if total == 1 { "" } else { "s" });

    Ok(view! {
        document(
            title: "Users",
            shell: Shell::page("admin_users"),
            page_header(title: "Users", subtitle: Some("Everyone with an account on the platform."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                filter_bar(
                    <label class="sr-only" for="users-q">"Search users"</label>
                    input(attrs: attributes! {
                        id="users-q" name="q" type="search" value=(q.unwrap_or(""))
                        class="sm:max-w-xs" placeholder="Account or name"
                    })
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Search")
                    <span class="ml-auto text-sm text-muted-foreground">(&summary)</span>
                )
                table(
                    table_header(
                        table_row(
                            table_head("Name")
                            table_head("Role")
                            table_head("Sign-in methods")
                            table_head("Last sign-in")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 4, text: "No user matches.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { class=(if row.deleted { "text-muted-foreground" } else { "" }) },
                                table_cell(
                                    <span class="flex flex-wrap items-center gap-2">
                                        <a class=(LINK) href=(href!(user_id::user, user_id::UserId(row.id.clone())))>(&row.label)</a>
                                        if row.id == me.id { <span class="text-muted-foreground">"(you)"</span> }
                                        if row.deleted { status_badge(tone: Tone::Neutral, "deleted") }
                                    </span>
                                    <span class="block text-xs text-muted-foreground">(&row.account)</span>
                                )
                                table_cell((role_label(row.is_admin)))
                                table_cell((methods_label(row.has_password, row.oauth2_count, row.passkey_count)))
                                table_cell(
                                    if let Some(at) = row.last_sign_in_at {
                                        <time datetime=(at)>(format_time(at as u64))</time>
                                    } else {
                                        "never"
                                    }
                                )
                            )
                        }
                    )
                )
                pager(
                    page: page_number,
                    pages: pages,
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}
