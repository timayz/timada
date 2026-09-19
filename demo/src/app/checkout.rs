//! `/checkout`: delivery address, delivery method and payment mode, then the
//! cart is checked out. The order itself is placed by the order context's
//! process manager; its id derives from the cart id, so the confirmation
//! page knows where to look before the order exists.

use serde::Deserialize;
use timada_cart::{CartError, Checkout, DeliveryChoice, PaymentMode};
use timada_customer::{AddressBookView, load_address_book};
use timada_order::{INSTALLMENT_HANDLING_FEE_MINOR, load_order_details, order_id};
use timada_shipping::{DeliveryOffer, delivery_offers};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param, path_param as param,
    },
    view::{View, component, view},
};

use super::{
    account, cart, document,
    format::{address_lines, money},
};
use crate::{
    Store,
    auth::require_account,
    cart_session::{current_cart, forget_cart},
};

/// The demo shop's only pickup point for "retrait en boutique".
const PICKUP_STORE_ID: &str = "ldlc-toulouse";
const INSTALLMENT_COUNT: u8 = 3;

#[page("/checkout")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    require_account(cx).await?;
    Ok(view! { checkout_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct CheckoutForm {
    delivery_address_id: String,
    delivery_method: String,
    payment_mode: String,
}

#[page(POST "/checkout")]
pub async fn submit(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<impl View> {
    match check_out(cx, form).await? {
        Ok(order_id) => {
            forget_cart(cx);
            Err(see_other(href!(confirmation, OrderId(order_id)).resolve(cx)).into())
        }
        Err(message) => Ok(view! { checkout_view(error: Some(message)) }),
    }
}

/// Checks the cart out; `Ok(Err(..))` is a refusal to show on the form.
async fn check_out(cx: &Cx, form: CheckoutForm) -> Result<std::result::Result<String, String>> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let back_to_cart = || see_other(href!(cart::show).resolve(cx));
    let cart_id = match current_cart(cx).await {
        Ok(Some(cart)) => cart.id.clone(),
        Ok(None) => return Err(back_to_cart().into()),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let book = load_address_book(&store.executor, &account.customer_id)
        .await?
        .ok_or_not_found()?;

    let Some(delivery_address) = book
        .deliveries
        .iter()
        .find(|d| d.id == form.delivery_address_id)
        .map(|d| d.address.clone())
    else {
        return Ok(Err("Choisissez une adresse de livraison.".to_owned()));
    };
    let Some(offer) = delivery_offers()
        .into_iter()
        .find(|o| o.code == form.delivery_method)
    else {
        return Ok(Err("Choisissez un mode de livraison.".to_owned()));
    };
    let payment_mode = match form.payment_mode.as_str() {
        "card" => PaymentMode::Card,
        "installments" => PaymentMode::Installments {
            count: INSTALLMENT_COUNT,
        },
        _ => return Ok(Err("Choisissez un mode de paiement.".to_owned())),
    };

    let checked_out = timada_cart::Command(&store.executor)
        .checkout(
            &cart_id,
            Checkout {
                customer_id: Some(account.customer_id.clone()),
                billing_address: book.billing.unwrap_or_else(|| delivery_address.clone()),
                delivery_address,
                delivery: DeliveryChoice {
                    method_code: offer.code.to_owned(),
                    pickup_store_id: offer
                        .requires_pickup_store
                        .then(|| PICKUP_STORE_ID.to_owned()),
                },
                payment_mode,
            },
        )
        .await;
    match checked_out {
        Ok(()) => Ok(Ok(order_id(&cart_id))),
        Err(CartError::EmptyCart | CartError::CartAlreadyCheckedOut | CartError::CartNotFound) => {
            Err(back_to_cart().into())
        }
        Err(CartError::Address(err)) => Ok(Err(format!("Adresse incomplète : {err}."))),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

#[component]
async fn checkout_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let cart = match current_cart(cx).await {
        Ok(Some(cart)) if !cart.lines.is_empty() => cart.clone(),
        Ok(_) => return Err(see_other(href!(cart::show).resolve(cx)).into()),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let book = load_address_book(&store.executor, &account.customer_id)
        .await?
        .ok_or_not_found()?;
    let new_address = href!(account::new_address)
        .query([("next", href!(show).resolve(cx))])
        .resolve(cx);
    let offers = delivery_offers();
    let handling_fee =
        timada_core::Money::new(INSTALLMENT_HANDLING_FEE_MINOR, &cart.subtotal.currency);
    let promo = cart::promo_line(store, &cart).await?;

    Ok(view! {
        document(
            title: "Passer commande",
            <h1>"Passer commande"</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <table>
                <caption class="muted">"Récapitulatif du panier"</caption>
                <thead>
                    <tr>
                        <th scope="col">"Produit"</th>
                        <th scope="col" class="num">"Quantité"</th>
                        <th scope="col" class="num">"Prix unitaire"</th>
                    </tr>
                </thead>
                <tbody>
                    for line in &cart.lines {
                        <tr>
                            <th scope="row">(line.name.clone())</th>
                            <td class="num">(line.quantity.to_string())</td>
                            <td class="num">(money(&line.unit_price))</td>
                        </tr>
                    }
                </tbody>
            </table>
            <table class="totals">
                <tbody>
                    cart::promo_totals(subtotal: money(&cart.subtotal), promo: &promo)
                </tbody>
            </table>
            cart::promo_notice(promo: &promo)
            <p class="muted">"Les frais de livraison et de dossier s'ajoutent au sous-total. " <a href=(href!(cart::show))>"Modifier le panier"</a></p>

            if book.deliveries.is_empty() {
                <p class="notice">"Ajoutez une adresse de livraison pour continuer : " <a href=(new_address)>"nouvelle adresse"</a></p>
            } else {
                <form method="post" action=(href!(submit))>
                    delivery_addresses(book: &book, new_address: &new_address)
                    <fieldset>
                        <legend>"Mode de livraison"</legend>
                        for (index, offer) in offers.iter().enumerate() {
                            delivery_offer(offer: offer, checked: index == 0)
                        }
                    </fieldset>
                    <fieldset>
                        <legend>"Mode de paiement"</legend>
                        <label class="choice">
                            <input type="radio" name="payment_mode" value="card" checked=(true) required=(true)>
                            <span>"Carte bancaire"</span>
                        </label>
                        <label class="choice">
                            <input type="radio" name="payment_mode" value="installments">
                            <span>"Paiement en " (INSTALLMENT_COUNT.to_string()) " fois (frais de dossier " (money(&handling_fee)) ")"</span>
                        </label>
                    </fieldset>
                    <button type="submit">"Valider la commande"</button>
                </form>
            }
        )
    })
}

#[component]
async fn delivery_addresses(book: &AddressBookView, new_address: &str) -> Result<impl View> {
    Ok(view! {
        <fieldset>
            <legend>"Adresse de livraison"</legend>
            for delivery in &book.deliveries {
                <label class="choice">
                    <input type="radio" name="delivery_address_id" value=(delivery.id.clone()) checked=(delivery.preferred) required=(true)>
                    <span>(address_lines(&delivery.address).join(", "))</span>
                </label>
            }
            <p><a href=(new_address)>"Livrer à une autre adresse"</a></p>
            match &book.billing {
                Some(billing) => <p class="muted">"Facturation : " (address_lines(billing).join(", "))</p>,
                None => <p class="muted">"Sans adresse de facturation, l'adresse de livraison est utilisée."</p>,
            }
        </fieldset>
    })
}

#[component]
async fn delivery_offer(offer: &DeliveryOffer, checked: bool) -> Result<impl View> {
    let label = match offer.code {
        "chronopost-dom" => "Chronopost (DOM-TOM)",
        "colissimo" => "Colissimo à domicile",
        "store-pickup" => "Retrait en boutique LDLC Toulouse",
        other => other,
    };
    let fee = if offer.fee.is_positive() {
        money(&offer.fee)
    } else {
        "gratuit".to_owned()
    };
    Ok(view! {
        <label class="choice">
            <input type="radio" name="delivery_method" value=(offer.code) checked=(checked) required=(true)>
            <span>(label) " — " (fee)</span>
        </label>
    })
}

path_param!(pub order_id: String, error = not_found);

/// Shown right after checkout. Until the process manager has placed the
/// order the page says so and reloads itself.
#[page("/checkout/confirmation/{order_id}")]
pub async fn confirmation(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<OrderId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let order = load_order_details(&store.executor, &id).await?;
    if let Some(order) = &order {
        (order.customer_id == account.customer_id)
            .then_some(())
            .ok_or_not_found()?;
    }
    let details = href!(account::order_detail, OrderId(id.clone())).resolve(cx);

    Ok(view! {
        match &order {
            Some(order) => {
                document(
                    title: "Commande enregistrée",
                    <h1>"Merci, votre commande est enregistrée"</h1>
                    <p class="notice">"Commande " <strong>(order.display_number().to_owned())</strong> " — total " (money(&order.total)) "."</p>
                    <p><a href=(details)>"Suivre cette commande"</a></p>
                )
            }
            None => {
                document(
                    title: "Commande en cours d'enregistrement",
                    refresh: Some(2),
                    <h1>"Votre commande est en cours d'enregistrement"</h1>
                    <p role="status">"Cette page se recharge automatiquement."</p>
                )
            }
        }
    })
}
