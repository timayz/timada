//! The support page at `/support-client`: the
//! contact form on one side, the questions shoppers ask most on the other,
//! the shop's four promises below. The form posts here too and hands the
//! message to the dashboard, where the shop reads it; the shopper is told
//! it was sent, or why it was refused.

use std::{borrow::Cow, collections::HashMap};

use timada_storefront::{MessageDraft, menu, send_message, takes_messages};
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
    CONTACT_EMAIL, document, reassurance, say,
    words::{Worded, t},
};

/// What the form says when the shop has nowhere to send messages (no
/// dashboard address): the visitor is told to write instead.
const MESSAGES_UNAVAILABLE: &str = "messages_unavailable";

/// The questions and their answers: placeholders for your own, which your
/// policy pages should agree with.
pub fn faq() -> [(Worded<'static>, Worded<'static>); 7] {
    [
        (t!(support_faq_secure), t!(support_faq_secure.answer)),
        (t!(support_faq_publisher), t!(support_faq_publisher.answer)),
        (
            t!(support_faq_delivery_time),
            t!(support_faq_delivery_time.answer),
        ),
        (t!(support_faq_tracking), t!(support_faq_tracking.answer)),
        (
            t!(support_faq_cancellation),
            t!(support_faq_cancellation.answer),
        ),
        (t!(support_faq_return), t!(support_faq_return.answer)),
        (
            t!(support_faq_returns_policy),
            t!(support_faq_returns_policy.answer),
        ),
    ]
}

/// What the live search matches a page of the shop's own against.
pub struct SearchEntry {
    pub slug: &'static str,
    pub title: Cow<'static, str>,
    pub paragraphs: [Cow<'static, str>; 2],
}

/// How the live search finds this page: its title and its questions, in
/// the visitor's language. The other pages are the merchant's, found in the
/// store.
pub fn search_entry(cx: &Cx) -> SearchEntry {
    SearchEntry {
        slug: SLUG,
        title: t!(shell_support).resolve(cx),
        paragraphs: [
            t!(support_search_contact).resolve(cx),
            t!(support_search_questions).resolve(cx),
        ],
    }
}

/// The name the live search knows this page by.
pub const SLUG: &str = "support-client";

#[topcoat::router::query_params(error = bad_request)]
struct SupportQuery {
    error: Option<String>,
    sent: Option<String>,
}

#[derive(serde::Serialize)]
struct Link<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent: Option<u8>,
}

/// The page, carrying a refusal or the word that the message went.
fn support_url(cx: &Cx, error: Option<&str>, sent: bool) -> String {
    href!(index)
        .query(Link {
            error,
            sent: sent.then_some(1),
        })
        .resolve(cx)
}

/// How a refusal of the form reads; the codes the rest of the shop shares
/// read as they do elsewhere.
fn say_support(code: &str) -> Worded<'static> {
    match code {
        "message_invalid_name" => t!(support_error_invalid_name),
        "message_invalid_email" => t!(error_invalid_email),
        "message_invalid_body" => t!(support_error_invalid_body),
        "too_many_requests" => t!(support_error_too_many_requests),
        MESSAGES_UNAVAILABLE => t!(support_error_unavailable),
        other => say(other),
    }
}

#[topcoat::router::page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<SupportQuery>(cx)?;
    let nav = menu(cx).await?;
    let takes = takes_messages(cx);
    let sent = query.sent.is_some();
    let title = t!(shell_support).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs)) data-crumbs="">
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a> " / " (&*title)
                </nav>
                <h1>(t!(support_heading, email: CONTACT_EMAIL))</h1>
                <section class="support" data-support="">
                    <div class="card panel">
                        <span class="eyebrow">(t!(support_contact_eyebrow))</span>
                        <h2>(t!(support_contact_title))</h2>
                        if let Some(code) = &query.error {
                            <p class="notice" role="alert">(say_support(code))</p>
                        }
                        if sent {
                            <p class="notice" role="status" data-sent="">(t!(support_sent))</p>
                        }
                        if takes {
                            <form class="checkout" method="post" action=(href!(send)) data-support-form="">
                                <div class="row">
                                    <label>(t!(support_first_name))
                                        <input name="first_name" type="text" required="" maxlength="80" autocomplete="given-name">
                                    </label>
                                    <label>(t!(support_last_name))
                                        <input name="last_name" type="text" required="" maxlength="80" autocomplete="family-name">
                                    </label>
                                </div>
                                <label>(t!(support_email))
                                    <input name="email" type="email" required="" autocomplete="email">
                                </label>
                                <label>(t!(support_message))
                                    <textarea name="body" rows="6" required="" minlength="10" maxlength="4000"></textarea>
                                </label>
                                                                <button class="btn">(t!(support_send))</button>
                            </form>
                        } else {
                            <p data-support-unavailable="">(t!(support_write_to)) " " <a href=(format!("mailto:{CONTACT_EMAIL}"))>(CONTACT_EMAIL)</a> "."</p>
                        }
                    </div>
                    <div class="card panel">
                        <span class="eyebrow">(t!(support_faq_eyebrow))</span>
                        <h2>(t!(support_faq_title))</h2>
                        <div class="faq" data-faq="">
                            for (i, (question, answer)) in faq().into_iter().enumerate() {
                                if i == 0 {
                                    <details open="">
                                        <summary>(question)</summary>
                                        <p>(answer)</p>
                                    </details>
                                } else {
                                    <details>
                                        <summary>(question)</summary>
                                        <p>(answer)</p>
                                    </details>
                                }
                            }
                        </div>
                        <p class="hint">
                            (t!(support_policies_before)) " "
                            <a href=(href!(crate::app::politique_de_remboursement::index))>(t!(support_policies_refund))</a>
                            " " (t!(support_policies_between)) " "
                            <a href=(href!(crate::app::politique_de_livraison::index))>(t!(support_policies_shipping))</a>
                            "."
                        </p>
                    </div>
                </section>
            </div>
            <section class="block">
                <div class="wrap">
                    reassurance()
                </div>
            </section>
        )
    })
}

/// Hands the form's message to the dashboard, then back to the page with
/// the word that it went, or why it was refused.
#[route(POST)]
pub async fn send(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    if !takes_messages(cx) {
        return Ok(see_other(support_url(
            cx,
            Some(MESSAGES_UNAVAILABLE),
            false,
        )));
    }
    let field = |name: &str| form.get(name).cloned().unwrap_or_default();
    let draft = MessageDraft {
        first_name: field("first_name"),
        last_name: field("last_name"),
        email: field("email"),
        body: field("body"),
    };
    match send_message(cx, draft).await? {
        Ok(_) => Ok(see_other(support_url(cx, None, true))),
        Err(refusal) => Ok(see_other(support_url(cx, Some(refusal.code()), false))),
    }
}
