//! `/checkout`: delivery address first — it decides the tax zone, hence the
//! prices and the delivery methods on offer — then delivery method and payment
//! mode, and the cart is checked out. The order itself is placed by the order context's
//! process manager; its id derives from the cart id, so the confirmation
//! page knows where to look before the order exists.

use serde::Deserialize;
use timada_cart::{CartError, Checkout, DeliveryChoice, PaymentMode};
use timada_customer::{AddressBookView, load_address_book};
use timada_order::{
    INSTALLMENT_HANDLING_FEE_MINOR, lines_charged_in_zone, load_order_details, order_id,
};
use timada_shipping::delivery_offers;
use timada_tax::TaxTreatment;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param, path_param as param, query_params, query_params as query,
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
    cart_session::{current_cart, forget_cart, fresh_cart},
    db::tax_zones,
};

/// The demo shop's only pickup point for "retrait en boutique".
const PICKUP_STORE_ID: &str = "ldlc-toulouse";
const INSTALLMENT_COUNT: u8 = 3;

/// `?address=<id>` picks the delivery address the page is priced for; the
/// preferred one otherwise.
#[query_params(error = bad_request)]
struct CheckoutQuery {
    address: Option<String>,
}

#[page("/checkout")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    require_account(cx).await?;
    let address = query::<CheckoutQuery>(cx)?.address.clone();
    Ok(view! { checkout_view(error: None, address_id: address) })
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
        Err((message, address_id)) => {
            Ok(view! { checkout_view(error: Some(message), address_id: Some(address_id)) })
        }
    }
}

/// Checks the cart out; `Ok(Err((message, address id)))` is a refusal to show
/// on the form, still priced for the address that was posted.
async fn check_out(
    cx: &Cx,
    form: CheckoutForm,
) -> Result<std::result::Result<String, (String, String)>> {
    let address_id = form.delivery_address_id.clone();
    let refuse = |message: &str| Ok(Err((message.to_owned(), address_id.clone())));
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let back_to_cart = || see_other(href!(cart::show).resolve(cx));
    let cart_id = match current_cart(cx).await {
        Ok(Some(cart)) => cart.id.clone(),
        Ok(None) => return Err(back_to_cart().into()),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    // Never confirm a total the shopper has not seen: if a price moved since
    // the page was shown, show it again first.
    if let Some((_, notices)) = fresh_cart(cx).await?
        && !notices.is_empty()
    {
        return refuse(&format!(
            "{} Vérifiez le nouveau total avant de valider.",
            notices.join(" ")
        ));
    }
    let book = load_address_book(&store.executor, &account.customer_id)
        .await?
        .ok_or_not_found()?;

    let Some(delivery_address) = book
        .deliveries
        .iter()
        .find(|d| d.id == form.delivery_address_id)
        .map(|d| d.address.clone())
    else {
        return refuse("Choisissez une adresse de livraison.");
    };
    // Where it goes decides what can be ordered: the shop delivers the
    // countries of its tax zones, each with its own delivery methods.
    let zones = tax_zones();
    let Some(zone) = zones.zone_of(&delivery_address.country_code) else {
        return refuse("Nous ne livrons pas encore ce pays. Choisissez une autre adresse.");
    };
    let Some(offer) = delivery_offers()
        .into_iter()
        .find(|o| o.code == form.delivery_method)
    else {
        return refuse("Choisissez un mode de livraison.");
    };
    if !zone.offers(offer.code) {
        return refuse("Ce mode de livraison ne dessert pas cette adresse.");
    }
    let payment_mode = match form.payment_mode.as_str() {
        "card" => PaymentMode::Card,
        "installments" => PaymentMode::Installments {
            count: INSTALLMENT_COUNT,
        },
        _ => return refuse("Choisissez un mode de paiement."),
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
        Err(CartError::Address(err)) => refuse(&format!("Adresse incomplète : {err}.")),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

/// A delivery method of the chosen zone, priced for it.
struct OfferLine {
    code: &'static str,
    label: &'static str,
    fee: String,
}

fn offer_label(code: &str) -> &'static str {
    match code {
        "chronopost-dom" => "Chronopost (DOM-TOM)",
        "colissimo" => "Colissimo à domicile",
        "store-pickup" => "Retrait en boutique LDLC Toulouse",
        _ => "Livraison",
    }
}

#[component]
async fn checkout_view(
    cx: &Cx,
    error: Option<String>,
    address_id: Option<String>,
) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let (cart, price_notices) = match fresh_cart(cx).await? {
        Some((cart, notices)) if !cart.lines.is_empty() => (cart, notices),
        _ => return Err(see_other(href!(cart::show).resolve(cx)).into()),
    };
    let book = load_address_book(&store.executor, &account.customer_id)
        .await?
        .ok_or_not_found()?;
    let new_address = href!(account::new_address)
        .query([("next", href!(show).resolve(cx))])
        .resolve(cx);

    // The address the page is priced for: the one asked for, else the
    // preferred one, else the first.
    let chosen = address_id
        .as_ref()
        .and_then(|id| book.deliveries.iter().find(|d| d.id == *id))
        .or_else(|| book.deliveries.iter().find(|d| d.preferred))
        .or_else(|| book.deliveries.first())
        .cloned();
    let chosen_id = chosen.as_ref().map(|d| d.id.clone()).unwrap_or_default();
    let zones = tax_zones();
    let zone = chosen
        .as_ref()
        .and_then(|d| zones.zone_of(&d.address.country_code));
    // Without a deliverable address the cart is shown as listed.
    let pricing_zone = zone.unwrap_or_else(|| zones.default_zone());

    let charged =
        lines_charged_in_zone(&store.executor, &zones, pricing_zone, cart.lines.clone()).await?;
    let mut subtotal = timada_core::Money::zero(&cart.subtotal.currency);
    let mut lines = Vec::with_capacity(charged.len());
    for item in &charged {
        subtotal = subtotal.checked_add(&item.line.total()?)?;
        lines.push((
            item.line.name.clone(),
            item.line.quantity.to_string(),
            money(&item.line.unit_price),
        ));
    }
    let promo = cart::promo_line_on(store, cart.promo_code.as_ref(), &subtotal).await?;
    let zone_notice = match pricing_zone.treatment {
        TaxTreatment::Export => Some(format!(
            "Livraison {} : vente hors TVA française. Les prix ci-dessous sont hors taxes ; \
             d'éventuelles taxes locales sont à régler à la réception.",
            pricing_zone.label
        )),
        TaxTreatment::Domestic | TaxTreatment::DestinationVat => None,
    };
    let offers: Vec<OfferLine> = delivery_offers()
        .into_iter()
        .filter(|offer| pricing_zone.offers(offer.code))
        .map(|offer| {
            let fee = pricing_zone.charged(&offer.fee, zones.fee_vat_rate_bp);
            OfferLine {
                code: offer.code,
                label: offer_label(offer.code),
                fee: if fee.is_positive() {
                    money(&fee)
                } else {
                    "gratuit".to_owned()
                },
            }
        })
        .collect();
    let handling_fee =
        timada_core::Money::new(INSTALLMENT_HANDLING_FEE_MINOR, &cart.subtotal.currency);
    let deliverable = zone.is_some();

    Ok(view! {
        document(
            title: "Passer commande",
            <h1>"Passer commande"</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            for notice in &price_notices { <p role="status" class="notice">(notice.clone())</p> }

            if book.deliveries.is_empty() {
                <p class="notice">"Ajoutez une adresse de livraison pour continuer : " <a href=(new_address.clone())>"nouvelle adresse"</a></p>
            } else {
                <form method="get" action=(href!(show))>
                    delivery_addresses(book: &book, chosen_id: &chosen_id, new_address: &new_address)
                </form>
            }

            if let Some(notice) = &zone_notice { <p role="status" class="notice">(notice.clone())</p> }
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
                    for (name, quantity, unit_price) in &lines {
                        <tr>
                            <th scope="row">(name.clone())</th>
                            <td class="num">(quantity.clone())</td>
                            <td class="num">(unit_price.clone())</td>
                        </tr>
                    }
                </tbody>
            </table>
            <table class="totals">
                <tbody>
                    cart::promo_totals(subtotal: money(&subtotal), promo: &promo)
                </tbody>
            </table>
            cart::promo_notice(promo: &promo)
            <p class="muted">"Les frais de livraison et de dossier s'ajoutent au sous-total. " <a href=(href!(cart::show))>"Modifier le panier"</a></p>

            if !book.deliveries.is_empty() && !deliverable {
                <p role="alert" class="error">"Nous ne livrons pas encore ce pays. Choisissez une autre adresse de livraison."</p>
            }
            if deliverable {
                <form method="post" action=(href!(submit))>
                    <input type="hidden" name="delivery_address_id" value=(chosen_id.clone())>
                    <fieldset>
                        <legend>"Mode de livraison"</legend>
                        for (index, offer) in offers.iter().enumerate() {
                            <label class="choice">
                                <input type="radio" name="delivery_method" value=(offer.code) checked=(index == 0) required=(true)>
                                <span>(offer.label) " — " (offer.fee.clone())</span>
                            </label>
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

/// The address the order is priced for. Changing it reloads the page: the
/// destination decides the prices and the delivery methods.
#[component]
async fn delivery_addresses(
    book: &AddressBookView,
    chosen_id: &str,
    new_address: &str,
) -> Result<impl View> {
    let several = book.deliveries.len() > 1;
    Ok(view! {
        <fieldset>
            <legend>"Adresse de livraison"</legend>
            for delivery in &book.deliveries {
                <label class="choice">
                    <input type="radio" name="address" value=(delivery.id.clone()) checked=(delivery.id == chosen_id) required=(true)>
                    <span>(address_lines(&delivery.address).join(", "))</span>
                </label>
            }
            if several {
                <p><button type="submit">"Livrer à cette adresse"</button></p>
            }
            <p><a href=(new_address)>"Livrer à une autre adresse"</a></p>
            match &book.billing {
                Some(billing) => <p class="muted">"Facturation : " (address_lines(billing).join(", "))</p>,
                None => <p class="muted">"Sans adresse de facturation, l'adresse de livraison est utilisée."</p>,
            }
        </fieldset>
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
