//! The page of an invitation to create an account. The link was mailed to
//! the invited address, so it stands for the proof of that address: the
//! person chooses a password, or continues with a sign-in provider that
//! vouches for the same address, and the account is theirs. Creating it is
//! an explicit POST, never a side effect of opening the link.

use serde::Deserialize;
use timada_identity::{RegisterInvited, hash_password};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, module_param, page, query_params, route,
    },
    view::{View, attributes, class, view},
};

use crate::app::auth::{Flash, error_code, signed_in_url};
use crate::auth::{current_user, identity, login_context, now, oidc::Providers, sign_in_as};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::CARD;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::ui::{LINK, auth_document, icons, notice};

module_param!(pub token);

#[topcoat::router::query_params(error = bad_request)]
struct InvitationQuery {
    error: Option<String>,
}

pub fn invitation_url(cx: &Cx, token: &str, error: Option<&str>) -> String {
    href!(index, token: token.to_owned())
        .query(Flash {
            error,
            message: None,
        })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = token(cx);
    let query = query_params::<InvitationQuery>(cx)?;
    let user = current_user(cx).await;
    let providers = context::<Providers>(cx);
    let invitation = identity(cx)
        .load_account_invitation_by_token(token)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|invitation| invitation.is_open_at(now()));

    // What the page offers: nothing, signing out first, or the account.
    enum Offer {
        Invalid,
        SignedIn { label: String },
        Create { email: String },
    }
    let offer = match (invitation, user) {
        (None, _) => Offer::Invalid,
        (Some(_), Some(user)) => Offer::SignedIn {
            label: user.label.clone(),
        },
        (Some(invitation), None) => Offer::Create {
            email: invitation.email,
        },
    };
    let state = match offer {
        Offer::Invalid => "invalid",
        Offer::SignedIn { .. } => "signed-in",
        Offer::Create { .. } => "open",
    };
    let invalid = matches!(offer, Offer::Invalid);
    let (title, lead) = match &offer {
        Offer::Invalid => ("Invitation", "This invitation is no longer valid."),
        Offer::SignedIn { .. } => (
            "Create your Timada account",
            "This invitation creates a new account.",
        ),
        Offer::Create { .. } => (
            "Create your Timada account",
            "You are invited to create the account of the address below.",
        ),
    };

    Ok(view! {
        auth_document(
            title: "Invitation",
            <section class=(class!(CARD, "gap-5 px-6")) data-account-invitation=(state)>
                <div class="flex flex-col items-center gap-3 text-center">
                    <span class=(class!(
                        "flex size-12 items-center justify-center rounded-xl [&>svg]:size-6",
                        if invalid { "bg-muted text-muted-foreground" } else { "bg-primary/10 text-primary" },
                    ))>
                        icon(data: icons::INVITATIONS)
                    </span>
                    <h1 class="text-2xl font-bold tracking-tight">(title)</h1>
                    <p class="text-sm text-muted-foreground">(lead)</p>
                </div>
                notice(error: query.error.as_deref())

                match offer {
                    Offer::Invalid => {
                        <p class="text-center text-sm">"It may have been used, withdrawn, or it expired. Ask the person who invited you for a new one."</p>
                        <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Md)) href=(href!(crate::app::auth::login::login))>icon(data: icons::LOG_IN) "Sign in"</a>
                    }
                    Offer::SignedIn { label } => {
                        <p class="text-center text-sm">
                            "You are signed in as " <strong>(&label)</strong> ". Sign out, then open the link again to create the account."
                        </p>
                        <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Md)) href=(href!(crate::app::home))>"Go to the dashboard"</a>
                    }
                    Offer::Create { email } => {
                        <form method="post" action=(href!(submit, token: token.to_owned())) class="flex flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="email" }, "E-mail address")
                                input(attrs: attributes! {
                                    id="email" name="email" type="email" autocomplete="username" readonly="" value=(&email)
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="password" }, "Password")
                                input(attrs: attributes! {
                                    id="password" name="password" type="password" autocomplete="new-password" minlength="8" maxlength="128" required=""
                                })
                                field_description("8 characters or more.")
                            )
                            button(attrs: attributes! { class="w-full" }, "Create account")
                        </form>

                        if providers.iter().next().is_some() {
                            <p class="flex items-center gap-3 text-xs text-muted-foreground before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border">"Or"</p>
                            <div class="flex flex-col gap-2">
                                for provider in providers.iter() {
                                    <form method="post" action=(href!(crate::app::auth::oauth2::provider::start, crate::app::auth::oauth2::provider::provider: provider.name.clone()))>
                                        <input type="hidden" name="mode" value="create_user">
                                        <input type="hidden" name="invitation" value=(token)>
                                        button(variant: ButtonVariant::Secondary, attrs: attributes! { class="w-full" },
                                            "Continue with " (&provider.label)
                                        )
                                    </form>
                                }
                                <p class="text-xs text-muted-foreground">"With a provider, sign in there with this same address."</p>
                            </div>
                        }

                        <p class="text-center text-sm text-muted-foreground">
                            "Already have an account? "
                            <a class=(LINK) href=(href!(crate::app::auth::login::login))>"Sign in"</a>
                        </p>
                    }
                }
            </section>
        )
    })
}

#[derive(Deserialize)]
struct CreateForm {
    password: String,
}

/// Creates the account of the invited address with the password chosen, and
/// signs the browser in. The address is the invitation's, whatever the form
/// carried.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let token = token(cx);
    if current_user(cx).await.is_some() {
        return Ok(see_other(invitation_url(cx, token, Some("signed_in"))));
    }
    // Looked at before the password is hashed, which costs: a link that
    // opens nothing gets no work out of this node.
    let open = identity(cx)
        .load_account_invitation_by_token(token)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some_and(|invitation| invitation.is_open_at(now()));
    if !open {
        return Ok(see_other(invitation_url(
            cx,
            token,
            Some("invitation_invalid"),
        )));
    }
    let password_hash = match hash_password(&form.password).await {
        Ok(hash) => hash,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(invitation_url(cx, token, Some(error_code(&err)))));
        }
        Err(err) => return Err(err.into()),
    };
    match identity(cx)
        .register_invited(
            RegisterInvited {
                token: token.to_owned(),
                password_hash,
                context: login_context(cx),
            },
            now(),
        )
        .await
    {
        Ok(done) => {
            sign_in_as(cx, &done.user_id).await?;
            Ok(see_other(signed_in_url(cx, &done.label, false)))
        }
        Err(err) if err.is_user_facing() => {
            Ok(see_other(invitation_url(cx, token, Some(error_code(&err)))))
        }
        Err(err) => Err(err.into()),
    }
}
