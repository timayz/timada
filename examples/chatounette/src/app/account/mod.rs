//! `/account`: the signed-in shopper's orders (every order placed with
//! their address, on any device) and saved addresses, a way to sign out
//! every other browser, and the way to delete the account. Signed out, it
//! sends to the sign-in page.

pub mod addresses;
pub mod delete;
pub mod sign_in;
pub mod sign_out;
pub mod sign_out_everywhere;

use timada_storefront::{OrderStatus, menu, shopper_orders};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, redirect},
        href, page, query_params,
    },
    view::{View, view},
};

use crate::app::orders::order_id::{OrderId, index as order_page};
use crate::app::{document, say};
use crate::shopper::{forget_shopper, shopper_token, signed_in};

#[topcoat::router::query_params(error = bad_request)]
struct AccountQuery {
    error: Option<String>,
    done: Option<String>,
}

pub fn account_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

/// The account page, saying what was just `done` (see [`say_done`]).
pub fn account_done_url(cx: &Cx, done: &str) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        done: &'a str,
    }
    href!(index).query(Link { done }).resolve(cx)
}

/// What the account page says once something is done.
fn say_done(code: &str) -> Option<&'static str> {
    match code {
        sign_out_everywhere::DONE => Some("Vous êtes déconnecté de tous les autres navigateurs."),
        _ => None,
    }
}

/// An order's status, as the orders table shows it.
fn status_label(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::Paid => "Payée",
        OrderStatus::Fulfilled => "Expédiée",
        OrderStatus::Cancelled => "Annulée",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let to_sign_in = sign_in::sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    let shopper = signed_in(cx)
        .await
        .cloned()
        .ok_or_redirect(to_sign_in.clone())?;
    let token = shopper_token(cx).ok_or_redirect(to_sign_in.clone())?;
    let orders = match shopper_orders(cx, &token).await? {
        Ok(orders) => orders,
        Err(refusal) if refusal.is_signed_out() => {
            forget_shopper(cx);
            return Err(redirect(to_sign_in).into());
        }
        Err(_) => Vec::new(),
    };
    let query = query_params::<AccountQuery>(cx)?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Mon compte",
            nav: &nav,
            <h1>"Mon compte"</h1>
            <p>"Connecté avec l'adresse " <strong data-account-email="">(&shopper.email)</strong></p>
            <form method="post" action=(href!(sign_out::sign_out))>
                <button>"Se déconnecter"</button>
            </form>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            if let Some(done) = query.done.as_deref().and_then(say_done) {
                <p class="notice" role="status">(done)</p>
            }
            <section data-account-orders="">
                <h2>"Mes commandes"</h2>
                if orders.is_empty() {
                    <p>"Aucune commande pour le moment."</p>
                } else {
                    <table class="history">
                        <thead>
                            <tr><th>"Commande"</th><th>"Articles"</th><th>"Total"</th><th>"Statut"</th></tr>
                        </thead>
                        <tbody>
                            for order in &orders {
                                <tr data-account-order=(&order.reference)>
                                    <td><a href=(href!(order_page, OrderId(order.id.clone())))>(&order.reference)</a></td>
                                    <td>(order.item_count)</td>
                                    <td>(order.total_label())</td>
                                    <td>(status_label(order.status()))</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
            </section>
            <section data-account-addresses="">
                <h2>"Mes adresses"</h2>
                if shopper.addresses.is_empty() {
                    <p>"Les adresses de livraison que vous saisissez en passant commande sont enregistrées ici."</p>
                } else {
                    <ul class="saved">
                        for saved in &shopper.addresses {
                            <li data-saved-address=(&saved.address.city)>
                                <strong>(&saved.address.name)</strong> ", "
                                (&saved.address.line1) ", "
                                (&saved.address.postal_code) " " (&saved.address.city) ", "
                                (&saved.address.country) " "
                                <form method="post" action=(href!(addresses::remove::remove))>
                                    <input type="hidden" name="address_id" value=(&saved.id)>
                                    <button>"Retirer"</button>
                                </form>
                            </li>
                        }
                    </ul>
                }
            </section>
            <section data-account-sessions="">
                <h2>"Mes connexions"</h2>
                <p>"Connecté sur un ordinateur que vous n'utilisez plus ? Déconnectez-vous de tous les autres navigateurs ; celui-ci reste connecté."</p>
                <form method="post" action=(href!(sign_out_everywhere::sign_out_everywhere))>
                    <button>"Se déconnecter partout ailleurs"</button>
                </form>
            </section>
            <section data-account-delete="">
                <h2>"Supprimer mon compte"</h2>
                <p>"Efface vos adresses et vous déconnecte partout. Vos commandes restent chez la boutique."</p>
                <a href=(href!(delete::index))>"Supprimer mon compte…"</a>
            </section>
        )
    })
}
