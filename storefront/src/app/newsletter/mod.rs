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

use crate::app::{
    document, say,
    words::{Worded, t},
};

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
pub fn say_newsletter(code: &str) -> Worded<'static> {
    match code {
        "newsletter_invalid_email" => t!(error_invalid_email),
        "newsletter_invalid_link" => t!(newsletter_error_invalid_link),
        "newsletter_link_expired" => t!(newsletter_error_link_expired),
        "too_many_requests" => t!(newsletter_error_too_many_requests),
        "newsletter_unavailable" | NEWSLETTER_OFF => t!(newsletter_error_unavailable),
        other => say(other),
    }
}

/// What a step done reads as.
fn say_done(done: &str) -> Option<Worded<'static>> {
    Some(match done {
        "sent" => t!(newsletter_done_sent),
        "confirmed" => t!(newsletter_done_confirmed),
        "unsubscribed" => t!(newsletter_done_unsubscribed),
        _ => return None,
    })
}

/// The form the footer and this page show.
#[topcoat::view::component]
pub async fn newsletter_form(cx: &Cx) -> Result<impl View> {
    Ok(view! {
        <form class="newsletter" method="post" action=(href!(subscribe)) data-newsletter-form="">
            <label for="newsletter-email">(t!(newsletter_email))</label>
            <div class="newsletter-row">
                <input id="newsletter-email" name="email" type="email" required="" autocomplete="email" placeholder=(t!(newsletter_email.placeholder))>
                <button class="btn">(t!(newsletter_subscribe))</button>
            </div>
        </form>
    })
}

#[topcoat::router::page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<NewsletterQuery>(cx)?;
    let nav = menu(cx).await?;
    let done = query.done.as_deref().and_then(say_done);
    let title = t!(footer_newsletter).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs))>
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a> " / " (&*title)
                </nav>
                <h1>(t!(newsletter_heading))</h1>
                <div class="card panel" data-newsletter="">
                    if let Some(code) = &query.error {
                        <p class="notice" role="alert" data-newsletter-error=(code)>(say_newsletter(code))</p>
                    }
                    if let Some(done) = done {
                        <p class="notice" role="status" data-newsletter-done=(query.done.as_deref().unwrap_or_default())>(done)</p>
                    }
                    <p>(t!(newsletter_pitch))</p>
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
        Ok(()) => {
            crate::shopper::note_subscribed(cx);
            Ok(see_other(newsletter_url(cx, None, Some("sent"))))
        }
        Err(refusal) => Ok(see_other(newsletter_url(cx, Some(refusal.code()), None))),
    }
}
