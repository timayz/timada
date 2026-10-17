//! `/reviews/{token}`: the link a review request mails the buyer of a
//! fulfilled order. A valid token sends the shopper to the order's review
//! forms; a stale or unknown token says so and offers to sign in instead.

use timada_storefront::pages::open_review_link;
use topcoat::{
    Result,
    context::Cx,
    router::{module_param, page, path_param},
    view::{View, view},
};

use crate::app::document;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let expired = open_review_link(cx, path_param::<Token>(cx)).await?;

    Ok(view! {
        document(
            title: "Lien expiré",
            <h1>"Ce lien a expiré"</h1>
            <p data-review-link-expired="">
                "Le lien pour donner votre avis sur votre commande ne fonctionne plus. Connectez-vous avec l'adresse e-mail utilisée pour commander afin de retrouver vos commandes et donner votre avis."
            </p>
            <p><a class="primary" href=(&expired.sign_in_url)>"Se connecter"</a></p>
        )
    })
}
