use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::redirect, href, page, query_params},
    view::{View, view},
};

use crate::auth::{current_user, oidc::Providers};
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct LoginQuery {
    error: Option<String>,
    message: Option<String>,
}

#[page]
pub async fn login(cx: &Cx) -> Result<impl View> {
    if current_user(cx).await.is_some() {
        return Err(redirect(super::home_url(cx)).into());
    }
    let query = query_params::<LoginQuery>(cx)?;
    let providers = app_context::<Providers>(cx);

    Ok(view! {
        document(
            title: "Sign in",
            <h1 class="mb-6 text-2xl font-semibold">"Sign in to Timada"</h1>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <form method="post" action=(href!(super::email::login::login)) class="mb-4 space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="email">"E-mail address"</label>
                <input id="email" name="email" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="email" autocomplete="username" maxlength="254" required="">
                <label class="block text-sm" for="password">"Password"</label>
                <input id="password" name="password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="password" autocomplete="current-password" required="">
                <button class="w-full rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Sign in"</button>
                <p class="text-sm text-neutral-600">
                    <a class="underline" href=(href!(super::forgot_password::forgot_password))>"Forgot password?"</a>
                    " · New here? "
                    <a class="underline" href=(href!(super::register::register))>"Create an account"</a>
                </p>
            </form>

            <section class="mb-8 space-y-3">
                for provider in providers.iter() {
                    <form method="post" action=(href!(super::oauth2::provider::start, super::oauth2::provider::Provider(provider.name.clone())))>
                        <input type="hidden" name="mode" value="create_user_or_login">
                        <button class="w-full rounded border border-neutral-300 bg-white px-4 py-2 font-medium hover:bg-neutral-100">
                            "Continue with " (&provider.label)
                        </button>
                    </form>
                }
            </section>

            <section class="space-y-3 border-t border-neutral-200 pt-6">
                <button type="button"
                    class="w-full rounded border border-neutral-900 px-4 py-2 font-medium hover:bg-neutral-100"
                    data-passkey-login=""
                    data-start=(href!(super::passkey::login::start::start))
                    data-finish=(href!(super::passkey::login::finish::finish))>
                    "Sign in with a passkey"
                </button>
                <p class="text-xs text-neutral-500">"Passkeys are added from your account page once you are signed in."</p>
            </section>
        )
    })
}
