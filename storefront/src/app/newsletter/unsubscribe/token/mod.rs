//! `/newsletter/unsubscribe/{token}`: the link every mail of news carries.
//! Opening it asks for one click (a mail scanner opening it unsubscribes
//! nobody); the button, and a mail client's one-click unsubscribe
//! (`List-Unsubscribe-Post`, a POST here), hand the token to the dashboard
//! and land on `/newsletter`.

use timada_storefront::{menu, unsubscribe};
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
    let title = t!(newsletter_unsubscribe_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(newsletter_unsubscribe_heading))</h1>
                <div class="card panel">
                    <p>(t!(newsletter_unsubscribe_text))</p>
                    <form method="post" action=(href!(submit, token: token)) data-newsletter-unsubscribe="">
                        <button class="btn">(t!(newsletter_unsubscribe_button))</button>
                    </form>
                </div>
            </div>
        )
    })
}

#[route(POST)]
pub async fn submit(cx: &Cx) -> Result<SeeOther> {
    let token = token(cx);
    let to = match unsubscribe(cx, token).await? {
        Ok(()) => newsletter_url(cx, None, Some("unsubscribed")),
        Err(refusal) => newsletter_url(cx, Some(refusal.code()), None),
    };
    Ok(see_other(to))
}
