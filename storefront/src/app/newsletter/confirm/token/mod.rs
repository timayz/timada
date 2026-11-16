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

use crate::app::document;
use crate::app::newsletter::newsletter_url;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = token(cx).to_owned();
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Confirmer l'inscription",
            nav: &nav,
            <div class="wrap">
                <h1>"Confirmer votre inscription"</h1>
                <div class="card panel">
                    <p>"Un clic encore et vous recevrez notre newsletter."</p>
                    <form method="post" action=(href!(submit, token: token)) data-newsletter-confirm="">
                        <button class="btn">"Confirmer mon inscription"</button>
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
