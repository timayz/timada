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
        href, module_param, page, path_param, route,
    },
    view::{View, view},
};

use crate::app::document;
use crate::app::newsletter::newsletter_url;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx).to_owned();
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Se désinscrire",
            nav: &nav,
            <div class="wrap">
                <h1>"Se désinscrire de la newsletter"</h1>
                <div class="card panel">
                    <p>"Vous ne recevrez plus nos nouveautés ni nos offres par e-mail. Vos commandes, elles, continuent de vous être confirmées."</p>
                    <form method="post" action=(href!(submit, Token(token))) data-newsletter-unsubscribe="">
                        <button class="btn">"Me désinscrire"</button>
                    </form>
                </div>
            </div>
        )
    })
}

#[route(POST)]
pub async fn submit(cx: &Cx) -> Result<SeeOther> {
    let token = path_param::<Token>(cx);
    let to = match unsubscribe(cx, token).await? {
        Ok(()) => newsletter_url(cx, None, Some("unsubscribed")),
        Err(refusal) => newsletter_url(cx, Some(refusal.code()), None),
    };
    Ok(see_other(to))
}
