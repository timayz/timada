//! `/auth/account`: the signed-in user's own account, on the settings
//! layout: profile, e-mail and password, providers, passkeys, sessions and
//! the recent sign-ins.

use timada_identity::{SignInKind, active_sessions, session_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    session,
    view::{View, attributes, view},
};

use crate::auth::{now, oidc::Providers, require_user, webauthn::authenticator_name};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::db::ReadPool;
use crate::ui::{
    Shell, document, fact, facts, format_time, icons, notice, page_header, section_card,
    session_row, settings_layout, sign_in_row,
};

#[topcoat::router::query_params(error = bad_request)]
struct AccountQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The sections of the page, for the links on the left.
const SECTIONS: &[(&str, &str)] = &[
    ("profile", "Profile"),
    ("email", "E-mail and password"),
    ("providers", "Sign-in providers"),
    ("passkeys", "Passkeys"),
    ("sessions", "Sessions"),
    ("sign-ins", "Recent sign-ins"),
];

#[page]
pub async fn account(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<AccountQuery>(cx)?;
    let providers = app_context::<Providers>(cx);
    let user_id = user.id.as_str();
    let current_session = session::token_hash(cx).await?.map(|hash| session_id(&hash));
    let sessions = active_sessions(app_context::<ReadPool>(cx), user_id, now() as i64).await?;

    Ok(view! {
        document(
            title: "Your account",
            shell: Shell::page("account"),
            page_header(title: "Your account", subtitle: Some("How you sign in, and where you are signed in."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            settings_layout(
                label: "Account sections",
                sections: SECTIONS,
                section_card(
                    title: Some("Profile"),
                    attrs: attributes! { id="profile" class="scroll-mt-20" },
                    facts(
                        fact(term: "Account", (&user.account))
                        fact(term: "Name", (&user.label))
                        fact(term: "Role", (if user.is_admin { "Administrator" } else { "Member" }))
                    )
                )

                section_card(
                    title: Some("E-mail and password"),
                    attrs: attributes! { id="email" class="scroll-mt-20" },
                    if let Some(password) = &user.password {
                        <p class="text-sm">
                            <strong>(&password.email)</strong>
                            " · password changed " <time datetime=(password.changed_at)>(format_time(password.changed_at))</time>
                        </p>
                        <p class="mt-1 text-xs text-muted-foreground">"The address is set once and cannot be changed."</p>
                        <form method="post" action=(href!(super::password::change)) class="mt-5 flex max-w-md flex-col gap-4">
                            <input type="hidden" name="user_id" value=(user_id)>
                            <h3 class="text-sm font-semibold">"Change the password"</h3>
                            field(
                                field_label(attrs: attributes! { for="current-password" }, "Current password")
                                input(attrs: attributes! {
                                    id="current-password" name="current_password" type="password"
                                    autocomplete="current-password" required=""
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-password" }, "New password")
                                input(attrs: attributes! {
                                    id="new-password" name="password" type="password"
                                    autocomplete="new-password" minlength="8" maxlength="128" required=""
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-password-confirm" }, "Confirm the new password")
                                input(attrs: attributes! {
                                    id="new-password-confirm" name="password_confirm" type="password"
                                    autocomplete="new-password" minlength="8" maxlength="128" required=""
                                })
                                field_description("8 characters or more. Your other browsers will be signed out.")
                            )
                            <div>
                                button(attrs: attributes! { type="submit" }, "Change password")
                            </div>
                        </form>
                    } else {
                        <p class="text-sm text-muted-foreground">"Not set up: this account signs in with a provider or a passkey."</p>
                        <form method="post" action=(href!(super::email::add::add)) class="mt-5 flex max-w-md flex-col gap-4">
                            <input type="hidden" name="user_id" value=(user_id)>
                            <h3 class="text-sm font-semibold">"Add an e-mail address and a password"</h3>
                            field(
                                field_label(attrs: attributes! { for="new-email" }, "E-mail address")
                                input(attrs: attributes! {
                                    id="new-email" name="email" type="email" autocomplete="email" maxlength="254" required=""
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-email-password" }, "Password")
                                input(attrs: attributes! {
                                    id="new-email-password" name="password" type="password"
                                    autocomplete="new-password" minlength="8" maxlength="128" required=""
                                })
                                field_description("8 characters or more. We will e-mail you a code to confirm the address; it cannot be changed afterwards.")
                            )
                            <div>
                                button(attrs: attributes! { type="submit" }, "Add e-mail address")
                            </div>
                        </form>
                    }
                )

                section_card(
                    title: Some("Sign-in providers"),
                    attrs: attributes! { id="providers" class="scroll-mt-20" },
                    if user.oauth2_accounts.is_empty() {
                        <p class="text-sm text-muted-foreground">"No provider linked."</p>
                    }
                    <ul class="divide-y divide-border">
                        for account in &user.oauth2_accounts {
                            <li class="flex flex-wrap items-center justify-between gap-2 py-2.5 text-sm">
                                <span>
                                    <strong>(provider_label(providers, &account.provider))</strong>
                                    " · " (account.email.as_deref().unwrap_or(&account.provider_user_id))
                                </span>
                                <form method="post" action=(href!(super::oauth2::unlink::unlink))>
                                    <input type="hidden" name="provider" value=(&account.provider)>
                                    <input type="hidden" name="provider_user_id" value=(&account.provider_user_id)>
                                    <input type="hidden" name="user_id" value=(user_id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Unlink")
                                </form>
                            </li>
                        }
                    </ul>
                    if providers.iter().next().is_some() {
                        <div class="mt-4 flex flex-wrap gap-2">
                            for provider in providers.iter() {
                                <form method="post" action=(href!(super::oauth2::provider::start, super::oauth2::provider::Provider(provider.name.clone())))>
                                    <input type="hidden" name="mode" value="add_to_user">
                                    <input type="hidden" name="user_id" value=(user_id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                        icon(data: icons::LOG_IN)
                                        "Link " (&provider.label)
                                    )
                                </form>
                            }
                        </div>
                    }
                )

                section_card(
                    title: Some("Passkeys"),
                    attrs: attributes! { id="passkeys" class="scroll-mt-20" },
                    if user.passkeys.is_empty() {
                        <p class="text-sm text-muted-foreground">"No passkey yet."</p>
                    }
                    <ul class="divide-y divide-border">
                        for passkey in &user.passkeys {
                            <li class="flex flex-wrap items-center justify-between gap-2 py-2.5 text-sm">
                                <form method="post" action=(href!(super::passkey::rename::rename)) class="flex flex-wrap items-center gap-2">
                                    <input type="hidden" name="credential_id" value=(&passkey.credential_id)>
                                    <input type="hidden" name="user_id" value=(user_id)>
                                    <label class="sr-only" for=(format!("passkey-{}", passkey.credential_id))>"Passkey name"</label>
                                    input(attrs: attributes! {
                                        id=(format!("passkey-{}", passkey.credential_id)) name="name" type="text"
                                        value=(&passkey.name) maxlength="64" required="" class="w-48"
                                    })
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Rename")
                                    if let Some(authenticator) = authenticator_name(&passkey.aaguid) {
                                        <span class="text-muted-foreground">(authenticator)</span>
                                    }
                                </form>
                                <form method="post" action=(href!(super::passkey::remove::remove))>
                                    <input type="hidden" name="credential_id" value=(&passkey.credential_id)>
                                    <input type="hidden" name="user_id" value=(user_id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Remove")
                                </form>
                            </li>
                        }
                    </ul>
                    <div class="mt-4 flex max-w-md flex-col gap-2">
                        field_label(attrs: attributes! { for="new-passkey-name" }, "Name for the new passkey")
                        <div class="flex flex-wrap gap-2">
                            input(attrs: attributes! { id="new-passkey-name" type="text" value="My passkey" maxlength="64" class="w-48" })
                            button(attrs: attributes! {
                                type="button" disabled=""
                                data-passkey-register=""
                                data-user-id=(user_id)
                                data-name-input="#new-passkey-name"
                                data-start=(href!(super::passkey::register::start::start))
                                data-finish=(href!(super::passkey::register::finish::finish))
                            }, icon(data: icons::API_KEYS) "Add a passkey")
                        </div>
                    </div>
                )

                section_card(
                    title: Some("Sessions"),
                    description: Some("Lost a device, or signed in on a shared computer? Every other browser is signed out; this one stays."),
                    attrs: attributes! { id="sessions" class="scroll-mt-20" },
                    <ul class="divide-y divide-border">
                        for s in &sessions {
                            session_row(
                                started_at: s.started_at as u64,
                                ip: s.ip.as_deref(),
                                user_agent: s.user_agent.as_deref(),
                                current: current_session.as_deref() == Some(s.id.as_str()),
                            )
                        }
                    </ul>
                    <form method="post" action=(href!(super::sessions::revoke::revoke)) class="mt-4">
                        <input type="hidden" name="user_id" value=(user_id)>
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" class="text-destructive-text" }, icon(data: icons::LOG_OUT) "Sign out everywhere else")
                    </form>
                )

                section_card(
                    title: Some("Recent sign-ins"),
                    attrs: attributes! { id="sign-ins" class="scroll-mt-20" },
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
            )
        )
    })
}

fn provider_label<'a>(providers: &'a Providers, name: &'a str) -> &'a str {
    providers
        .get(name)
        .map(|p| p.label.as_str())
        .unwrap_or(name)
}
