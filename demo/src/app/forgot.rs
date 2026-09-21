//! « Mot de passe oublié » : a shopper who cannot sign in asks for a link by
//! e-mail and chooses a new password from it. The pages never say whether an
//! address has an account, and the link — one hour, one use, only the latest —
//! is kept out of the `Referer` of whatever the page links to.

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        HeaderValue, content::Form, header::REFERRER_POLICY, href, page, query_params,
        query_params as query, response::response_headers,
    },
    view::{View, component, view},
};

use super::{Head, account, document};
use crate::{
    Store,
    auth::{self, ResetPasswordError},
};

/// Nothing here is for a search engine.
fn head() -> Head {
    Head {
        robots: Some("noindex"),
        ..Head::default()
    }
}

// ------------------------------------------------------------ asking

#[derive(Debug, Deserialize)]
pub struct ForgotForm {
    email: String,
}

#[page("/password/forgot")]
pub async fn forgot() -> Result<impl View> {
    Ok(view! { forgot_view(sent_to: None) })
}

/// Answers the same whether or not the address has an account.
#[page(POST "/password/forgot")]
pub async fn send_link(cx: &Cx, Form(form): Form<ForgotForm>) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    auth::request_password_reset(store, &form.email).await?;
    Ok(view! { forgot_view(sent_to: Some(form.email.trim().to_owned())) })
}

#[component]
async fn forgot_view(sent_to: Option<String>) -> Result<impl View> {
    let head = head();
    Ok(view! {
        document(
            title: "Mot de passe oublié",
            head: Some(&head),
            <h1>"Mot de passe oublié"</h1>
            if let Some(email) = &sent_to {
                <p role="status">
                    "Si un compte existe pour " <strong>(email.clone())</strong>
                    ", un message vient de lui être envoyé avec un lien pour choisir un nouveau \
                     mot de passe. Le lien est valable une heure."
                </p>
                <p class="muted">"Rien reçu ? Vérifiez les courriers indésirables, ou redemandez un lien dans une minute."</p>
            } else {
                <p>"Indiquez l'adresse email de votre compte : nous vous envoyons un lien pour choisir un nouveau mot de passe."</p>
                <form method="post" action=(href!(send_link)) class="stack">
                    <label for="email">"Email" <input id="email" name="email" type="email" required=(true) autocomplete="username"></label>
                    <button type="submit">"Recevoir le lien"</button>
                </form>
            }
            <p><a href=(href!(account::login))>"Retour à la connexion"</a></p>
        )
    })
}

// ---------------------------------------------------------- choosing

#[query_params(error = bad_request)]
struct ResetQuery {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetForm {
    token: String,
    new: String,
    confirm: String,
}

/// Where the reset page stands.
enum ResetStep {
    /// The form, with what was wrong with the last try.
    Choose {
        token: String,
        error: Option<String>,
    },
    /// The link was used, ran out, was replaced by a later one, or never was.
    Expired,
    Done,
}

#[page("/password/reset")]
pub async fn reset(cx: &Cx) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let token = query::<ResetQuery>(cx)?.token.clone().unwrap_or_default();
    let step = if auth::reset_link_is_valid(store, &token).await? {
        ResetStep::Choose { token, error: None }
    } else {
        ResetStep::Expired
    };
    Ok(view! { reset_view(step: step) })
}

#[page(POST "/password/reset")]
pub async fn choose(cx: &Cx, Form(form): Form<ResetForm>) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let step = if form.new != form.confirm {
        if auth::reset_link_is_valid(store, &form.token).await? {
            ResetStep::Choose {
                token: form.token,
                error: Some("Les deux mots de passe ne sont pas identiques.".to_owned()),
            }
        } else {
            ResetStep::Expired
        }
    } else {
        match auth::reset_password(store, &form.token, &form.new).await {
            Ok(()) => ResetStep::Done,
            Err(ResetPasswordError::InvalidLink) => ResetStep::Expired,
            Err(ResetPasswordError::Server(err)) => return Err(err.into()),
            Err(refused) => ResetStep::Choose {
                token: form.token,
                error: Some(refused.to_string()),
            },
        }
    };
    Ok(view! { reset_view(step: step) })
}

#[component]
async fn reset_view(cx: &Cx, step: ResetStep) -> Result<impl View> {
    // The address of this page is the secret: it goes to no other site.
    response_headers(cx).append(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    let head = head();
    Ok(view! {
        document(
            title: "Nouveau mot de passe",
            head: Some(&head),
            <h1>"Nouveau mot de passe"</h1>
            match &step {
                ResetStep::Choose { token, error } => {
                    if let Some(error) = error { <p role="alert" class="error">(error.clone())</p> }
                    <form method="post" action=(href!(choose)) class="stack">
                        <input type="hidden" name="token" value=(token.clone())>
                        <label>"Nouveau mot de passe (8 caractères minimum)"
                            <input name="new" type="password" required=(true) minlength="8" autocomplete="new-password">
                        </label>
                        <label>"Confirmer le nouveau mot de passe"
                            <input name="confirm" type="password" required=(true) minlength="8" autocomplete="new-password">
                        </label>
                        <button type="submit">"Enregistrer"</button>
                    </form>
                    <p class="muted">"Tous vos appareils seront déconnectés."</p>
                }
                ResetStep::Expired => {
                    <p role="alert" class="error">"Ce lien n'est plus valable : il a déjà servi, il date de plus d'une heure, ou un lien plus récent l'a remplacé."</p>
                    <p><a href=(href!(forgot))>"Demander un nouveau lien"</a></p>
                }
                ResetStep::Done => {
                    <p role="status">"Votre mot de passe est modifié. Vous pouvez vous connecter avec le nouveau."</p>
                    <p><a href=(href!(account::login))>"Se connecter"</a></p>
                }
            }
        )
    })
}
