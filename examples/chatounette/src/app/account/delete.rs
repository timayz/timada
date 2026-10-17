//! `/account/delete`: says what deleting the account forgets and what stays,
//! and deletes it on confirmation. The browser then forgets the shopper too.

use timada_storefront::{delete_account_form, pages::delete_account_page};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = delete_account_page(cx).await?;

    Ok(view! {
        document(
            title: "Supprimer mon compte",
            <h1>"Supprimer mon compte"</h1>
            <p>"Supprimer le compte de " <strong>(&page.shopper.email)</strong> " :"</p>
            <ul>
                <li>"efface les adresses que vous avez enregistrées ;"</li>
                <li>"vous déconnecte partout, y compris sur ce navigateur."</li>
            </ul>
            <p>"Vos commandes et leurs factures restent chez la boutique, qui doit les conserver. Si vous vous reconnectez avec cette adresse, vous repartez avec un nouveau compte et y retrouvez vos commandes."</p>
            delete_account_form(
                <button class="primary">"Supprimer mon compte"</button>
                " "
                <a href=(&page.keep_url)>"Conserver mon compte"</a>
            )
        )
    })
}
