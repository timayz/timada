use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::redirect, href, page, query_params},
    view::{View, attributes, class, view},
};

use crate::auth::{current_user, oidc::Providers};
use crate::components::button::{ButtonVariant, button};
use crate::components::card::CARD;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::ui::{LINK, auth_document, notice};

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
        auth_document(
            title: "Sign in",
            <div class=(class!(CARD, "gap-5 px-6"))>
                <div class="text-center">
                    <h1 class="text-2xl font-bold tracking-tight">"Sign in to Timada"</h1>
                    <p class="mt-1 text-sm text-muted-foreground">"Welcome back. Sign in to continue."</p>
                </div>
                notice(error: query.error.as_deref(), message: query.message.as_deref())

                <form method="post" action=(href!(super::email::login::login)) class="flex flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="email" }, "E-mail address")
                        input(attrs: attributes! {
                            id="email" name="email" type="email" autocomplete="username" maxlength="254" required=""
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="password" }, "Password")
                        input(attrs: attributes! {
                            id="password" name="password" type="password" autocomplete="current-password" required=""
                        })
                    )
                    button(attrs: attributes! { class="w-full" }, "Sign in")
                    <p class="text-center text-sm text-muted-foreground">
                        <a class=(LINK) href=(href!(super::forgot_password::forgot_password))>"Forgot password?"</a>
                        " · New here? "
                        <a class=(LINK) href=(href!(super::register::register))>"Create an account"</a>
                    </p>
                </form>

                if providers.iter().next().is_some() {
                    <p class="flex items-center gap-3 text-xs text-muted-foreground before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border">"Or"</p>
                    <div class="flex flex-col gap-2">
                        for provider in providers.iter() {
                            <form method="post" action=(href!(super::oauth2::provider::start, super::oauth2::provider::Provider(provider.name.clone())))>
                                <input type="hidden" name="mode" value="create_user_or_login">
                                button(variant: ButtonVariant::Secondary, attrs: attributes! { class="w-full" },
                                    "Continue with " (&provider.label)
                                )
                            </form>
                        }
                    </div>
                }

                <div class="flex flex-col gap-2 border-t border-border pt-5">
                    button(
                        variant: ButtonVariant::Outline,
                        attrs: attributes! {
                            type="button" disabled="" class="w-full"
                            data-passkey-login=""
                            data-start=(href!(super::passkey::login::start::start))
                            data-finish=(href!(super::passkey::login::finish::finish))
                        },
                        "Sign in with a passkey"
                    )
                    <p class="text-xs text-muted-foreground">"Passkeys are added from your account page once you are signed in."</p>
                </div>
            </div>
        )
    })
}
