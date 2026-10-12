//! `/admin/users/{user_id}`: one account and the actions on it.

pub mod delete;
pub mod role;
pub mod sign_out;

use timada_identity::{SignInKind, user_sessions};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, view},
};

use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::auth::{identity, now, oidc::Providers, webauthn::authenticator_name};
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice, session_row};

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
    let sessions = user_sessions(app_context::<ReadPool>(cx), &user.id).await?;
    let now = now() as i64;
    let providers = app_context::<Providers>(cx);
    let is_me = user.id == me.id;
    let title = format!("{} · Users", user.label);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">
                    (&user.label)
                    if user.deleted { " " <span class="rounded bg-neutral-200 px-2 text-sm font-normal text-neutral-700">"deleted"</span> }
                </h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::users))>"Users"</a>
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Profile"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Account"</dt><dd>(&user.account)</dd>
                    <dt class="text-neutral-500">"Role"</dt><dd>(if user.is_admin { "Administrator" } else { "Member" })</dd>
                    <dt class="text-neutral-500">"Registered"</dt><dd><time datetime=(user.registered_at)>(format_time(user.registered_at))</time></dd>
                    <dt class="text-neutral-500">"Sign-ins"</dt><dd>(user.sign_ins.len())</dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Sign-in methods"</h2>
                if user.sign_in_methods() == 0 {
                    <p class="text-sm text-neutral-500">"None."</p>
                }
                <ul class="divide-y divide-neutral-100 text-sm">
                    if let Some(password) = &user.password {
                        <li class="py-2">
                            <strong>"E-mail and password"</strong> " · " (&password.email)
                            " · password changed " <time datetime=(password.changed_at)>(format_time(password.changed_at))</time>
                        </li>
                    }
                    for account in &user.oauth2_accounts {
                        <li class="py-2">
                            <strong>(providers.get(&account.provider).map(|p| p.label.as_str()).unwrap_or(&account.provider))</strong>
                            " · " (account.email.as_deref().unwrap_or(&account.provider_user_id))
                            " · linked " <time datetime=(account.linked_at)>(format_time(account.linked_at))</time>
                        </li>
                    }
                    for passkey in &user.passkeys {
                        <li class="py-2">
                            <strong>"Passkey"</strong> " · " (&passkey.name)
                            if let Some(authenticator) = authenticator_name(&passkey.aaguid) { " (" (authenticator) ")" }
                            " · added " <time datetime=(passkey.added_at)>(format_time(passkey.added_at))</time>
                        </li>
                    }
                </ul>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Sessions"</h2>
                <ul class="divide-y divide-neutral-100">
                    for s in sessions.iter().filter(|s| s.is_active_at(now)) {
                        session_row(started_at: s.started_at as u64, ip: s.ip.as_deref(), user_agent: s.user_agent.as_deref())
                    }
                </ul>
                if !sessions.iter().any(|s| s.is_active_at(now)) {
                    <p class="text-sm text-neutral-500">"No active session."</p>
                }
                if !user.deleted {
                    <form method="post" action=(href!(sign_out::sign_out, UserId(user.id.clone()))) class="mt-3">
                        <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Sign out everywhere"</button>
                    </form>
                }
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
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

            if !user.deleted {
                <section class="rounded border border-red-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Administration"</h2>
                    if is_me {
                        <p class="text-sm text-neutral-500">"You cannot change your own role or delete your own account."</p>
                    } else {
                        <div class="flex flex-wrap gap-2">
                            <form method="post" action=(href!(role::role, UserId(user.id.clone())))>
                                <input type="hidden" name="admin" value=(if user.is_admin { "false" } else { "true" })>
                                <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">
                                    (if user.is_admin { "Revoke administrator" } else { "Make administrator" })
                                </button>
                            </form>
                            <form method="post" action=(href!(delete::delete, UserId(user.id.clone())))>
                                <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Delete account"</button>
                            </form>
                        </div>
                        <p class="mt-2 text-sm text-neutral-500">"Deleting signs the user out everywhere and frees their sign-in methods; the person can register again."</p>
                    }
                </section>
            }
        )
    })
}
