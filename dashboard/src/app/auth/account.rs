use timada_identity::{SignInKind, active_sessions, session_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    session,
    view::{View, view},
};

use crate::auth::{now, oidc::Providers, require_user, webauthn::authenticator_name};
use crate::db::ReadPool;
use crate::ui::{Shell, document, format_time, notice, session_row};

#[topcoat::router::query_params(error = bad_request)]
struct AccountQuery {
    error: Option<String>,
    message: Option<String>,
}

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
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Your account"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Profile"</h2>
                <dl class="grid grid-cols-[8rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Account"</dt><dd>(&user.account)</dd>
                    <dt class="text-neutral-500">"Name"</dt><dd>(&user.label)</dd>
                    <dt class="text-neutral-500">"Role"</dt><dd>(if user.is_admin { "Administrator" } else { "Member" })</dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"E-mail and password"</h2>
                if let Some(password) = &user.password {
                    <p class="text-sm">
                        <strong>(&password.email)</strong>
                        " · password changed " <time datetime=(password.changed_at)>(format_time(password.changed_at))</time>
                    </p>
                    <p class="mt-1 text-xs text-neutral-500">"The address is set once and cannot be changed."</p>
                    <form method="post" action=(href!(super::password::change)) class="mt-4 space-y-3">
                        <input type="hidden" name="user_id" value=(user_id)>
                        <h3 class="text-sm font-medium">"Change the password"</h3>
                        <label class="block text-sm" for="current-password">"Current password"</label>
                        <input id="current-password" name="current_password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="password" autocomplete="current-password" required="">
                        <label class="block text-sm" for="new-password">"New password"</label>
                        <input id="new-password" name="password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                        <label class="block text-sm" for="new-password-confirm">"Confirm the new password"</label>
                        <input id="new-password-confirm" name="password_confirm" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                        <p class="text-xs text-neutral-500">"8 characters or more. Your other browsers will be signed out."</p>
                        <button class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700">"Change password"</button>
                    </form>
                } else {
                    <p class="text-sm text-neutral-500">"Not set up: this account signs in with a provider or a passkey."</p>
                    <form method="post" action=(href!(super::email::add::add)) class="mt-4 space-y-3">
                        <input type="hidden" name="user_id" value=(user_id)>
                        <h3 class="text-sm font-medium">"Add an e-mail address and a password"</h3>
                        <label class="block text-sm" for="new-email">"E-mail address"</label>
                        <input id="new-email" name="email" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="email" autocomplete="email" maxlength="254" required="">
                        <label class="block text-sm" for="new-email-password">"Password"</label>
                        <input id="new-email-password" name="password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                        <p class="text-xs text-neutral-500">"8 characters or more. We will e-mail you a code to confirm the address; it cannot be changed afterwards."</p>
                        <button class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700">"Add e-mail address"</button>
                    </form>
                }
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Sign-in providers"</h2>
                if user.oauth2_accounts.is_empty() {
                    <p class="text-sm text-neutral-500">"No provider linked."</p>
                }
                <ul class="divide-y divide-neutral-100">
                    for account in &user.oauth2_accounts {
                        <li class="flex items-center justify-between py-2 text-sm">
                            <span>
                                <strong>(provider_label(providers, &account.provider))</strong>
                                " · " (account.email.as_deref().unwrap_or(&account.provider_user_id))
                            </span>
                            <form method="post" action=(href!(super::oauth2::unlink::unlink))>
                                <input type="hidden" name="provider" value=(&account.provider)>
                                <input type="hidden" name="provider_user_id" value=(&account.provider_user_id)>
                                <input type="hidden" name="user_id" value=(user_id)>
                                <button class="text-red-700 underline">"Unlink"</button>
                            </form>
                        </li>
                    }
                </ul>
                <div class="mt-3 flex flex-wrap gap-2">
                    for provider in providers.iter() {
                        <form method="post" action=(href!(super::oauth2::provider::start, super::oauth2::provider::Provider(provider.name.clone())))>
                            <input type="hidden" name="mode" value="add_to_user">
                            <input type="hidden" name="user_id" value=(user_id)>
                            <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">
                                "Link " (&provider.label)
                            </button>
                        </form>
                    }
                </div>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Passkeys"</h2>
                if user.passkeys.is_empty() {
                    <p class="text-sm text-neutral-500">"No passkey yet."</p>
                }
                <ul class="divide-y divide-neutral-100">
                    for passkey in &user.passkeys {
                        <li class="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
                            <form method="post" action=(href!(super::passkey::rename::rename)) class="flex items-center gap-2">
                                <input type="hidden" name="credential_id" value=(&passkey.credential_id)>
                                <input type="hidden" name="user_id" value=(user_id)>
                                <label class="sr-only" for=(format!("passkey-{}", passkey.credential_id))>"Passkey name"</label>
                                <input id=(format!("passkey-{}", passkey.credential_id)) class="rounded border border-neutral-300 px-2 py-1" type="text" name="name" value=(&passkey.name) maxlength="64" required="">
                                <button class="underline">"Rename"</button>
                                if let Some(authenticator) = authenticator_name(&passkey.aaguid) {
                                    <span class="text-neutral-500">(authenticator)</span>
                                }
                            </form>
                            <form method="post" action=(href!(super::passkey::remove::remove))>
                                <input type="hidden" name="credential_id" value=(&passkey.credential_id)>
                                <input type="hidden" name="user_id" value=(user_id)>
                                <button class="text-red-700 underline">"Remove"</button>
                            </form>
                        </li>
                    }
                </ul>
                <div class="mt-3">
                    <label class="block text-sm" for="new-passkey-name">"Name for the new passkey"</label>
                    <div class="mt-1 flex gap-2">
                        <input id="new-passkey-name" class="rounded border border-neutral-300 px-3 py-1" type="text" value="My passkey" maxlength="64">
                        <button type="button" disabled=""
                            class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700"
                            data-passkey-register=""
                            data-user-id=(user_id)
                            data-name-input="#new-passkey-name"
                            data-start=(href!(super::passkey::register::start::start))
                            data-finish=(href!(super::passkey::register::finish::finish))>
                            "Add a passkey"
                        </button>
                    </div>
                </div>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Sessions"</h2>
                <ul class="mb-3 divide-y divide-neutral-100">
                    for s in &sessions {
                        session_row(
                            started_at: s.started_at as u64,
                            ip: s.ip.as_deref(),
                            user_agent: s.user_agent.as_deref(),
                            current: current_session.as_deref() == Some(s.id.as_str()),
                        )
                    }
                </ul>
                <p class="mb-3 text-sm text-neutral-600">"Lost a device, or signed in on a shared computer? Every other browser is signed out; this one stays."</p>
                <form method="post" action=(href!(super::sessions::revoke::revoke))>
                    <input type="hidden" name="user_id" value=(user_id)>
                    <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Sign out everywhere else"</button>
                </form>
            </section>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Recent sign-ins"</h2>
                <ul class="divide-y divide-neutral-100 text-sm">
                    for entry in user.sign_ins.iter().rev().take(20) {
                        <li class="py-2">
                            <time datetime=(entry.at)>(format_time(entry.at))</time>
                            " · "
                            (match entry.method {
                                SignInKind::OAuth2 => entry.provider.clone().unwrap_or_else(|| "provider".to_owned()),
                                SignInKind::Passkey => "passkey".to_owned(),
                                SignInKind::Password => "password".to_owned(),
                            })
                            if let Some(ip) = &entry.ip { " · " (ip) }
                            if let Some(ua) = &entry.user_agent { <span class="block truncate text-neutral-500">(ua)</span> }
                        </li>
                    }
                </ul>
            </section>
        )
    })
}

fn provider_label<'a>(providers: &'a Providers, name: &'a str) -> &'a str {
    providers
        .get(name)
        .map(|p| p.label.as_str())
        .unwrap_or(name)
}
