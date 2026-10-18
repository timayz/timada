//! `/admin/users/{user_id}`: one account and the actions on it.

pub mod delete;
pub mod role;
pub mod sign_out;

use timada_identity::{SignInKind, user_sessions};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use super::role_label;
use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::auth::{identity, now, oidc::Providers, webauthn::authenticator_name};
use crate::components::button::{ButtonVariant, button};
use crate::db::ReadPool;
use crate::ui::{
    Shell, Tone, document, fact, facts, format_time, notice, page_header, section_card,
    session_row, settings_layout, sign_in_row, status_badge,
};

module_param!(pub user_id);

#[topcoat::router::query_params(error = bad_request)]
struct UserQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn user_url(cx: &Cx, user_id: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(user, UserId(user_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The sections of the page, for the links on the left.
const SECTIONS: &[(&str, &str)] = &[
    ("profile", "Profile"),
    ("methods", "Sign-in methods"),
    ("sessions", "Sessions"),
    ("sign-ins", "Recent sign-ins"),
    ("administration", "Administration"),
];

#[page]
pub async fn user(cx: &Cx) -> Result<impl View> {
    let me = require_admin(cx).await?;
    let id = path_param::<UserId>(cx);
    let query = query_params::<UserQuery>(cx)?;
    let Some(user) = identity(cx)
        .load_user_any(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Err(not_found().into());
    };
    let now = now() as i64;
    let active: Vec<_> = user_sessions(app_context::<ReadPool>(cx), &user.id)
        .await?
        .into_iter()
        .filter(|s| s.is_active_at(now))
        .collect();
    let providers = app_context::<Providers>(cx);
    let is_me = user.id == me.id;
    let title = format!("{} · Users", user.label);
    // The last section is the administration one; a deleted account has
    // nothing left to administer.
    let sections = if user.deleted {
        &SECTIONS[..SECTIONS.len() - 1]
    } else {
        SECTIONS
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("admin_users"),
            page_header(
                title: &user.label,
                subtitle: Some(&user.account),
                if user.deleted { status_badge(tone: Tone::Neutral, "deleted") }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            settings_layout(
                label: "User sections",
                sections: sections,
                section_card(
                    title: Some("Profile"),
                    attrs: attributes! { id="profile" class="scroll-mt-20" },
                    facts(
                        fact(term: "Account", (&user.account))
                        fact(term: "Role", (role_label(user.is_admin)))
                        fact(term: "Registered", <time datetime=(user.registered_at)>(format_time(user.registered_at))</time>)
                        fact(term: "Sign-ins", (user.sign_ins.len()))
                    )
                )

                section_card(
                    title: Some("Sign-in methods"),
                    attrs: attributes! { id="methods" class="scroll-mt-20" },
                    if user.sign_in_methods() == 0 {
                        <p class="text-sm text-muted-foreground">"None."</p>
                    }
                    <ul class="divide-y divide-border text-sm">
                        if let Some(password) = &user.password {
                            <li class="py-2.5">
                                <strong>"E-mail and password"</strong> " · " (&password.email)
                                <span class="block text-xs text-muted-foreground">"Password changed " <time datetime=(password.changed_at)>(format_time(password.changed_at))</time></span>
                            </li>
                        }
                        for account in &user.oauth2_accounts {
                            <li class="py-2.5">
                                <strong>(providers.get(&account.provider).map(|p| p.label.as_str()).unwrap_or(&account.provider))</strong>
                                " · " (account.email.as_deref().unwrap_or(&account.provider_user_id))
                                <span class="block text-xs text-muted-foreground">"Linked " <time datetime=(account.linked_at)>(format_time(account.linked_at))</time></span>
                            </li>
                        }
                        for passkey in &user.passkeys {
                            <li class="py-2.5">
                                <strong>"Passkey"</strong> " · " (&passkey.name)
                                if let Some(authenticator) = authenticator_name(&passkey.aaguid) { " (" (authenticator) ")" }
                                <span class="block text-xs text-muted-foreground">"Added " <time datetime=(passkey.added_at)>(format_time(passkey.added_at))</time></span>
                            </li>
                        }
                    </ul>
                )

                section_card(
                    title: Some("Sessions"),
                    attrs: attributes! { id="sessions" class="scroll-mt-20" },
                    if active.is_empty() {
                        <p class="text-sm text-muted-foreground">"No active session."</p>
                    } else {
                        <ul class="divide-y divide-border">
                            for s in &active {
                                session_row(started_at: s.started_at as u64, ip: s.ip.as_deref(), user_agent: s.user_agent.as_deref())
                            }
                        </ul>
                    }
                    if !user.deleted {
                        <form method="post" action=(href!(sign_out::sign_out, UserId(user.id.clone()))) class="mt-4">
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" class="text-destructive-text" }, "Sign out everywhere")
                        </form>
                    }
                )

                section_card(
                    title: Some("Recent sign-ins"),
                    attrs: attributes! { id="sign-ins" class="scroll-mt-20" },
                    if user.sign_ins.is_empty() {
                        <p class="text-sm text-muted-foreground">"Never signed in."</p>
                    }
                    <ul class="divide-y divide-border">
                        for entry in user.sign_ins.iter().rev().take(20) {
                            sign_in_row(
                                at: entry.at,
                                method: match entry.method {
                                    SignInKind::OAuth2 => entry.provider.clone().unwrap_or_else(|| "provider".to_owned()),
                                    SignInKind::Passkey => "passkey".to_owned(),
                                    SignInKind::Password => "password".to_owned(),
                                },
                                ip: entry.ip.as_deref(),
                                user_agent: entry.user_agent.as_deref(),
                            )
                        }
                    </ul>
                )

                if !user.deleted {
                    section_card(
                        title: Some("Administration"),
                        description: Some(if is_me {
                            "You cannot change your own role or delete your own account."
                        } else {
                            "Deleting signs the user out everywhere and frees their sign-in methods; the person can register again."
                        }),
                        attrs: attributes! { id="administration" class="scroll-mt-20 border-destructive/30" },
                        if !is_me {
                            <div class="flex flex-wrap gap-2">
                                <form method="post" action=(href!(role::role, UserId(user.id.clone())))>
                                    <input type="hidden" name="admin" value=(if user.is_admin { "false" } else { "true" })>
                                    button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" },
                                        (if user.is_admin { "Revoke administrator" } else { "Make administrator" })
                                    )
                                </form>
                                <form method="post" action=(href!(delete::delete, UserId(user.id.clone())))>
                                    button(variant: ButtonVariant::Destructive, attrs: attributes! { type="submit" }, "Delete account")
                                </form>
                            </div>
                        }
                    )
                }
            )
        )
    })
}
