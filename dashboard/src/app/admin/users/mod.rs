//! `/admin/users`: everyone with an account and, for root, the invitations
//! to create one: sending them, and those still waiting.

pub mod user_id;

use timada_core::MailStatus;
use timada_identity::{
    MAX_EMAIL_LEN, ROOT_USER_ID, count_users, list_users, open_account_invitations,
};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::admin::{invitations, require_admin};
use crate::auth::now;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, empty_state, filter_bar, format_time, icons, notice,
    page_header, pager, section_card, status_badge,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub fn users_url(cx: &Cx, message: Option<&str>) -> String {
    href!(users)
        .query(UsersLink {
            q: None,
            page: None,
            message,
            error: None,
        })
        .resolve(cx)
}

/// The page, saying why an action on it was refused.
pub fn users_error_url(cx: &Cx, error: &str) -> String {
    href!(users)
        .query(UsersLink {
            q: None,
            page: None,
            message: None,
            error: Some(error),
        })
        .resolve(cx)
}

/// How the row words where the invitation's mail stands.
fn mail_state(mail: MailStatus) -> &'static str {
    match mail {
        MailStatus::Sending => "sending…",
        MailStatus::Retrying => "not sent yet: the mail server did not answer, trying again",
        MailStatus::Sent => "mailed",
        MailStatus::Undeliverable => "could not be delivered: check the address",
        MailStatus::Failed => "could not be sent: the mail server did not answer; invite again",
    }
}

/// "Root", "Administrator" or "Member".
pub fn role_label(user_id: &str, is_admin: bool) -> &'static str {
    if user_id == ROOT_USER_ID {
        "Root"
    } else if is_admin {
        "Administrator"
    } else {
        "Member"
    }
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
    let db = context::<Db>(cx);
    let total = count_users(db, q)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = list_users(db, q, (page_number - 1) * PAGE_SIZE, PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let page_link = move |page: i64| {
        href!(users)
            .query(UsersLink {
                q,
                page: Some(page),
                message: None,
                error: None,
            })
            .resolve(cx)
    };
    let summary = format!("{total} user{}", if total == 1 { "" } else { "s" });
    // Root alone invites, so root alone sees the invitations.
    let is_root = me.is_root();
    let invitations = if is_root {
        open_account_invitations(db, now())
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        Vec::new()
    };
    let signup_open = context::<Config>(cx).auth.signup.is_open();

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
                                        <a class=(LINK) href=(href!(user_id::user, user_id::user_id: row.id.clone()))>(&row.label)</a>
                                        if row.id == me.id { <span class="text-muted-foreground">"(you)"</span> }
                                        if row.deleted { status_badge(tone: Tone::Neutral, "deleted") }
                                    </span>
                                    <span class="block text-xs text-muted-foreground">(&row.account)</span>
                                )
                                table_cell((role_label(&row.id, row.is_admin)))
                                table_cell(
                                    if row.id == ROOT_USER_ID {
                                        "password, in the configuration"
                                    } else {
                                        (methods_label(row.has_password, row.oauth2_count, row.passkey_count))
                                    }
                                )
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

            if is_root {
                section_card(
                    title: Some("Invite someone"),
                    description: Some(if signup_open {
                        "The link in the e-mail is valid for 7 days and creates the account of this address. Sign-up is open: anyone can also create an account from the login page."
                    } else {
                        "The link in the e-mail is valid for 7 days and creates the account of this address. Sign-up is closed: this is the only way to get an account."
                    }),
                    <form method="post" action=(href!(invitations::send::send)) class="flex flex-wrap items-end gap-3">
                        field(
                            attrs: attributes! { class="grow" },
                            field_label(attrs: attributes! { for="invite-email" }, "E-mail address")
                            input(attrs: attributes! {
                                id="invite-email" name="email" type="email" maxlength=(MAX_EMAIL_LEN) required=""
                            })
                        )
                        button(attrs: attributes! { type="submit" }, "Send invitation")
                    </form>
                )

                section_card(
                    title: Some("Pending invitations"),
                    if invitations.is_empty() {
                        empty_state(title: "None.", symbol: Some(icons::INVITATIONS))
                    } else {
                        <ul class="divide-y divide-border text-sm">
                            for invitation in &invitations {
                                <li class="flex flex-wrap items-center justify-between gap-2 py-3" data-invitation-email=(&invitation.email)>
                                    <span class="min-w-0">
                                        <span class="font-medium">(&invitation.email)</span>
                                        <span class="block text-xs text-muted-foreground">
                                            "expires " <time datetime=(invitation.expires_at)>(format_time(invitation.expires_at as u64))</time>
                                            " · " <span data-invitation-mail=(invitation.mail().as_str())>(mail_state(invitation.mail()))</span>
                                        </span>
                                    </span>
                                    <form method="post" action=(href!(invitations::revoke::revoke))>
                                        <input type="hidden" name="invitation_id" value=(&invitation.id)>
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Withdraw")
                                    </form>
                                </li>
                            }
                        </ul>
                    }
                )
            }
        )
    })
}
