//! `/newsletter/confirm/{token}`: the link the shop mails an address asking
//! for its news. The page asks for one click rather than confirming on
//! sight, so a mail scanner opening the link confirms nothing; the button
//! hands the token to the dashboard and lands on `/newsletter`.

use timada_storefront::{Confirmation, confirm_subscription, menu};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, module_param, page, route,
    },
    view::{View, view},
};

use crate::app::newsletter::newsletter_url;
use crate::app::{document, words::t};

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = token(cx).to_owned();
    let nav = menu(cx).await?;
    let title = t!(newsletter_confirm_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(newsletter_confirm_heading))</h1>
                <div class="card panel">
                    <p>(t!(newsletter_confirm_text))</p>
                    <form method="post" action=(href!(submit, token: token)) data-newsletter-confirm="">
                        <button class="btn">(t!(newsletter_confirm_button))</button>
                    </form>
                </div>
            </div>
        )
    })
}

#[route(POST)]
pub async fn submit(cx: &Cx) -> Result<SeeOther> {
    let token = token(cx);
    let to = match confirm_subscription(cx, token).await? {
        Ok(Confirmation::Confirmed | Confirmation::AlreadyConfirmed) => {
            // Confirmed on a device that may not be the one they signed up on.
            crate::shopper::note_subscribed(cx);
            newsletter_url(cx, None, Some("confirmed"))
        }
        Err(refusal) => newsletter_url(cx, Some(refusal.code()), None),
    };
    Ok(see_other(to))
}
