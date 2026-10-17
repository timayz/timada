//! `/account/sign-in`: the shopper gives their e-mail address and the shop
//! mails them a code (`/account/sign-in/code` takes it). There is no
//! sign-up: the first code entered creates the account.

pub mod code;

use timada_storefront::{pages::sign_in_page, sign_in_form};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, attributes, view},
};

use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = sign_in_page(cx).await?;

    Ok(view! {
        document(
            title: "Se connecter",
            <h1>"Se connecter"</h1>
            <p>"Saisissez votre adresse e-mail : nous vous enverrons un code pour vous connecter. Pas de mot de passe ni de compte à créer : vos commandes passées avec cette adresse sont déjà là."</p>
            if let Some(error) = &page.error {
                <p class="notice" role="alert">(&error.text)</p>
            }
            sign_in_form(page: &page, attrs: attributes! { class="sign-in" },
                <label for="sign-in-email">"E-mail"</label>
                <input id="sign-in-email" name="email" type="email" required="" autocomplete="email" value=(&page.email)>
                <button class="primary">"Recevoir un code"</button>
            )
        )
    })
}
