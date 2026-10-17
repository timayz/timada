//! `/account`: the signed-in shopper's orders (every order placed with
//! their address, on any device) and saved addresses, a way to sign out
//! every other browser, and the way to delete the account. Signed out, it
//! sends to the sign-in page.

pub mod delete;
pub mod sign_in;

use timada_storefront::{
    pages::account_page, remove_address_form, sign_out_everywhere_form, sign_out_form,
};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = account_page(cx).await?;

    Ok(view! {
        document(
            title: "Mon compte",
            <h1>"Mon compte"</h1>
            <p>"Connecté avec l'adresse " <strong data-account-email="">(&page.shopper.email)</strong></p>
            sign_out_form(
                <button>"Se déconnecter"</button>
            )
            if let Some(error) = &page.error {
                <p class="notice" role="alert">(&error.text)</p>
            }
            if let Some(done) = &page.done {
                <p class="notice" role="status">(done)</p>
            }
            <section data-account-orders="">
                <h2>"Mes commandes"</h2>
                if page.orders.is_empty() {
                    <p>"Aucune commande pour le moment."</p>
                } else {
                    <table class="history">
                        <thead>
                            <tr><th>"Commande"</th><th>"Articles"</th><th>"Total"</th><th>"Statut"</th></tr>
                        </thead>
                        <tbody>
                            for row in &page.orders {
                                <tr data-account-order=(&row.order.reference)>
                                    <td><a href=(&row.url)>(&row.order.reference)</a></td>
                                    <td>(row.order.item_count)</td>
                                    <td>(&row.total)</td>
                                    <td>(&row.status_text)</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
            </section>
            <section data-account-addresses="">
                <h2>"Mes adresses"</h2>
                if page.shopper.addresses.is_empty() {
                    <p>"Les adresses de livraison que vous saisissez en passant commande sont enregistrées ici."</p>
                } else {
                    <ul class="saved">
                        for saved in &page.shopper.addresses {
                            <li data-saved-address=(&saved.address.city)>
                                <strong>(&saved.address.name)</strong> ", "
                                (&saved.address.line1) ", "
                                (&saved.address.postal_code) " " (&saved.address.city) ", "
                                (&saved.address.country) " "
                                remove_address_form(saved: saved,
                                    <button>"Retirer"</button>
                                )
                            </li>
                        }
                    </ul>
                }
            </section>
            <section data-account-sessions="">
                <h2>"Mes connexions"</h2>
                <p>"Connecté sur un ordinateur que vous n'utilisez plus ? Déconnectez-vous de tous les autres navigateurs ; celui-ci reste connecté."</p>
                sign_out_everywhere_form(
                    <button>"Se déconnecter partout ailleurs"</button>
                )
            </section>
            <section data-account-delete="">
                <h2>"Supprimer mon compte"</h2>
                <p>"Efface vos adresses et vous déconnecte partout. Vos commandes restent chez la boutique."</p>
                <a href=(&page.delete_url)>"Supprimer mon compte…"</a>
            </section>
        )
    })
}
