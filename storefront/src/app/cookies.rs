//! The cookie banner and `/cookies`: what the visitor allows the shop to
//! keep on their device beyond what selling needs (the cart, the sign-in).
//!
//! The banner sits at the top of every page until it is answered: accept,
//! refuse, or choose by purpose on `/cookies`, which the footer links to so
//! an answer can be changed at any time. Each is a plain form posted here:
//! nothing needs a script. The answer is kept for
//! [`CONSENT_MAX_AGE_DAYS`](timada_storefront::CONSENT_MAX_AGE_DAYS), and
//! the next page tells Google's Consent Mode of it
//! ([`consent_script`](timada_storefront::consent_script)).
//!
//! What brought the visitor ([`crate::shopper::source`]) follows the answer:
//! the banner's form carries the address it sat on and the site the visitor
//! came from, so a campaign's click is kept once it is allowed, and an
//! answer taken back drops what it no longer covers.

use std::collections::HashMap;

use timada_storefront::{Consent, click_at, menu, request_referrer};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params,
        request::original_uri,
        route,
    },
    view::{View, view},
};

use crate::app::cart::site_path;
use crate::app::{document, words::t};
use crate::shopper::{consent, keep_source_within, remember_arrival, set_consent};

#[topcoat::router::query_params(error = bad_request)]
struct CookiesQuery {
    /// Back from saving a choice.
    done: Option<String>,
}

/// The banner: shown by the layout while the visitor has not answered.
/// Refusing takes one click, as accepting does.
#[topcoat::view::component]
pub async fn consent_banner(cx: &Cx) -> Result<impl View> {
    // Back to the page the banner sat on, with its query: what brought the
    // visitor (a campaign's parameters) is still in it.
    let here = original_uri(cx)
        .path_and_query()
        .map(|here| here.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned());
    // And the site they came from, which the page after the answer no
    // longer knows.
    let referrer = request_referrer(cx);
    Ok(view! {
        <section class="consent" aria-label=(t!(cookies_label)) data-consent="">
            <div class="wrap">
                <p>(t!(cookies_banner_text))</p>
                <form method="post" action=(href!(answer))>
                    <input type="hidden" name="from" value=(here)>
                    if let Some(referrer) = &referrer {
                        <input type="hidden" name="referrer" value=(referrer)>
                    }
                    <button class="btn" name="answer" value="none">(t!(cookies_refuse_all))</button>
                    <button class="btn" name="answer" value="all">(t!(cookies_accept_all))</button>
                    <a href=(href!(index))>(t!(cookies_choose))</a>
                </form>
            </div>
        </section>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CookiesQuery>(cx)?;
    let nav = menu(cx).await?;
    let given = consent(cx);
    let allowed = given.unwrap_or_default();
    let title = t!(footer_cookies).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            no_banner: true,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs))>
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a> " / " (&*title)
                </nav>
                <h1>(&*title)</h1>
                <div class="card panel" data-cookies=(given.map(|consent| consent.cookie_value()).unwrap_or("unanswered"))>
                    if query.done.is_some() {
                        <p class="notice" role="status">(t!(cookies_saved))</p>
                    }
                    <p>(t!(cookies_intro))</p>
                    <form class="cookies" method="post" action=(href!(answer))>
                        <label class="check">
                            <input type="checkbox" name="analytics" value="1" checked=(allowed.analytics)>
                            <span><strong>(t!(cookies_analytics))</strong>(t!(cookies_analytics.purpose))</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="ads" value="1" checked=(allowed.ads)>
                            <span><strong>(t!(cookies_ads))</strong>(t!(cookies_ads.purpose))</span>
                        </label>
                        <p class="cookies-act">
                            <button class="btn" name="answer" value="choice">(t!(cookies_save))</button>
                            <button class="btn" name="answer" value="none">(t!(cookies_refuse_all))</button>
                            <button class="btn" name="answer" value="all">(t!(cookies_accept_all))</button>
                        </p>
                    </form>
                </div>
            </div>
        )
    })
}

/// Keeps the answer, then back to the page the banner sat on (`from`, a path
/// on this site), or to this page saying so.
#[route(POST)]
pub async fn answer(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let field = |name: &str| form.get(name).map(String::as_str).unwrap_or_default();
    let consent = match field("answer") {
        "all" => Consent::ALL,
        "choice" => Consent {
            analytics: !field("analytics").is_empty(),
            ads: !field("ads").is_empty(),
        },
        // Anything else refuses: nothing is allowed by accident.
        _ => Consent::NONE,
    };
    set_consent(cx, consent);
    // What brought the visitor follows the answer: what it no longer covers
    // goes, and the arrival the banner sat on is kept now that it may be.
    keep_source_within(cx, consent)?;
    if let Some(from) = form.get("from")
        && let Some(click) = click_at(
            cx,
            site_path(from),
            form.get("referrer").map(String::as_str),
        )
    {
        remember_arrival(cx, click, consent)?;
    }
    let to = match form.get("from") {
        Some(from) => site_path(from).to_owned(),
        None => {
            #[derive(serde::Serialize)]
            struct Link {
                done: &'static str,
            }
            href!(index).query(Link { done: "1" }).resolve(cx)
        }
    };
    Ok(see_other(to))
}
