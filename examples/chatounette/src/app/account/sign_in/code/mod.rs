//! `/account/sign-in/code`: the shopper enters the code the shop mailed, and
//! is signed in on this browser.

use timada_storefront::{pages::sign_in_code_page, resend_code_form, sign_in_code_form};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, attributes, view},
};

use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = sign_in_code_page(cx).await?;

    Ok(view! {
        document(
            title: "Saisir le code",
            <h1>"Saisir le code"</h1>
            <p>"Nous avons envoyé un code à six chiffres à " <strong>(&page.flow.email)</strong> ". Il est valable 10 minutes."</p>
            if let Some(error) = &page.error {
                <p class="notice" role="alert">(&error.text)</p>
            } else if page.resent {
                <p class="notice" role="status">"Un nouveau code est en route."</p>
            }
            sign_in_code_form(attrs: attributes! { class="sign-in" },
                <label for="sign-in-code">"Code"</label>
                <input id="sign-in-code" name="code" required="" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]*" maxlength="8">
                <button class="primary">"Valider"</button>
            )
            resend_code_form(
                <button>"Recevoir un nouveau code"</button>
            )
            <p><a href=(&page.other_address_url)>"Utiliser une autre adresse"</a></p>
        )
    })
}
