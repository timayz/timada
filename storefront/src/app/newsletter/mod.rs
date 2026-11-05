//! The newsletter at `/newsletter`: the footer's form posts here and hands
//! the address to the dashboard, which mails it a link to confirm; the page
//! then says to look in the mailbox, or why the address was refused. The
//! links the shop mails land under it: `confirm/{token}` and
//! `unsubscribe/{token}`, and both come back here to say what they did.

pub mod confirm;
pub mod unsubscribe;

use std::collections::HashMap;

use timada_storefront::{menu, subscribe as hand_in, takes_newsletter_signups};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, query_params, route,
    },
    view::{View, view},
};

use crate::app::{document, say};

/// What the form says when the shop has nowhere to send sign-ups (no
/// dashboard address).
const NEWSLETTER_OFF: &str = "newsletter_off";

#[topcoat::router::query_params(error = bad_request)]
struct NewsletterQuery {
    error: Option<String>,
    /// `sent`, `confirmed` or `unsubscribed`.
    done: Option<String>,
}

#[derive(serde::Serialize)]
struct Link<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    done: Option<&'a str>,
}

/// The page, carrying a refusal or what was done.
pub fn newsletter_url(cx: &Cx, error: Option<&str>, done: Option<&str>) -> String {
    href!(index).query(Link { error, done }).resolve(cx)
}

/// How a refusal reads; the codes the rest of the shop shares read as they
/// do elsewhere.
pub fn say_newsletter(code: &str) -> &'static str {
    match code {
        "newsletter_invalid_email" => "Saisissez une adresse e-mail valide.",
        "newsletter_invalid_link" => {
            "Ce lien ne fonctionne pas, ou plus : inscrivez-vous à nouveau pour en recevoir un autre."
        }
        "newsletter_link_expired" => {
            "Ce lien a expiré : inscrivez-vous à nouveau pour en recevoir un autre."
        }
        "too_many_requests" => "Trop d'inscriptions envoyées : réessayez dans quelques minutes.",
        "newsletter_unavailable" | NEWSLETTER_OFF => {
            "L'inscription à la newsletter est indisponible pour le moment."
        }
        other => say(other),
    }
}

/// What a step done reads as.
fn say_done(done: &str) -> Option<&'static str> {
    Some(match done {
        "sent" => {
            "Presque fini ! Nous venons de vous envoyer un e-mail : cliquez sur le lien qu'il contient pour confirmer votre inscription."
        }
        "confirmed" => "C'est confirmé : vous recevrez nos nouveautés et nos offres.",
        "unsubscribed" => "C'est fait : vous ne recevrez plus notre newsletter.",
        _ => return None,
    })
}

/// The form the footer and this page show.
#[topcoat::view::component]
pub async fn newsletter_form() -> Result<impl View> {
    Ok(view! {
        <form class="newsletter" method="post" action=(href!(subscribe)) data-newsletter-form="">
            <label for="newsletter-email">"Votre e-mail"</label>
            <div class="newsletter-row">
                <input id="newsletter-email" name="email" type="email" required="" autocomplete="email" placeholder="vous@exemple.fr">
                <button class="btn">"S'inscrire"</button>
            </div>
        </form>
    })
}

#[topcoat::router::page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<NewsletterQuery>(cx)?;
    let nav = menu(cx).await?;
    let done = query.done.as_deref().and_then(say_done);
    let title = "Newsletter";
    Ok(view! {
        document(
            title: title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (title)
                </nav>
                <h1>"La newsletter Chatounette"</h1>
                <div class="card panel" data-newsletter="">
                    if let Some(code) = &query.error {
                        <p class="notice" role="alert" data-newsletter-error=(code)>(say_newsletter(code))</p>
                    }
                    if let Some(done) = done {
                        <p class="notice" role="status" data-newsletter-done=(query.done.as_deref().unwrap_or_default())>(done)</p>
                    }
                    <p>"Nouveautés, conseils et offres réservées aux abonnés, une à deux fois par mois. Vous vous désinscrivez en un clic, depuis chaque e-mail."</p>
                    newsletter_form()
                </div>
            </div>
        )
    })
}

/// Hands the footer's address to the dashboard, then to the page with the
/// word to look in the mailbox, or why it was refused.
#[route(POST)]
pub async fn subscribe(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    if !takes_newsletter_signups(cx) {
        return Ok(see_other(newsletter_url(cx, Some(NEWSLETTER_OFF), None)));
    }
    let email = form.get("email").map(String::as_str).unwrap_or_default();
    match hand_in(cx, email).await? {
        Ok(()) => Ok(see_other(newsletter_url(cx, None, Some("sent")))),
        Err(refusal) => Ok(see_other(newsletter_url(cx, Some(refusal.code()), None))),
    }
}
