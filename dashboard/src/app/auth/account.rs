use timada_identity::SignInMethod;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use crate::auth::{oidc::Providers, require_user};
use crate::ui::{document, notice};

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

    Ok(view! {
        document(
            title: "Your account",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Your account"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                    <form method="post" action=(href!(super::logout::logout))>
                        <button class="underline">"Sign out"</button>
                    </form>
                </nav>
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
                        <button type="button"
                            class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700"
                            data-passkey-register=""
                            data-mode="add_to_user"
                            data-user-id=(user_id)
                            data-name-input="#new-passkey-name"
                            data-start=(href!(super::passkey::register::start::start))
                            data-finish=(href!(super::passkey::register::finish::finish))>
                            "Add a passkey"
                        </button>
                    </div>
                </div>
            </section>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Recent sign-ins"</h2>
                <ul class="divide-y divide-neutral-100 text-sm">
                    for entry in user.sign_ins.iter().rev().take(20) {
                        <li class="py-2">
                            <time datetime=(entry.at)>(format_time(entry.at))</time>
                            " · "
                            (match entry.method {
                                SignInMethod::OAuth2 => entry.provider.clone().unwrap_or_else(|| "provider".to_owned()),
                                SignInMethod::Passkey => "passkey".to_owned(),
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

/// Unix seconds as an ISO-8601 UTC timestamp (no calendar crate needed).
fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_unix_time() {
        assert_eq!(super::format_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(super::format_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }
}
