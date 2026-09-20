//! `/checkout`: delivery address first — it decides the tax zone, hence the
//! prices and the delivery methods on offer — then delivery method and payment
//! mode, and the cart is checked out. The order itself is placed by the order context's
//! process manager; its id derives from the cart id, so the payment step
//! knows where to look before the order exists. `/checkout/pay/{order_id}`
//! waits for the payment to be requested, hands the shopper to the shop's
//! payment provider, and leads to the confirmation once the order is paid.

use serde::Deserialize;
use timada_cart::{CartError, Checkout, DeliveryChoice, PaymentMode};
use timada_customer::{AddressBookView, load_address_book};
use timada_order::{
    INSTALLMENT_HANDLING_FEE_MINOR, OrderDetailsView, OrderStatus, ReverseChargePolicy,
    business_purchase, cancellation_reason_label, lines_charged_in_zone, load_order_details,
    order_id, refresh_vat_standing,
};
use timada_payment::{
    PaymentError, PaymentMethod, PaymentStart, PaymentStatus, ReturnUrls, load_payment, payment_id,
    start_payment,
};
use timada_shipping::delivery_offers;
use timada_tax::{TaxTreatment, reverse_charged};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::{Form, Js},
        error::{RouterErrorExt, see_other},
        header::CONTENT_SECURITY_POLICY,
        href, page, path_param, path_param as param, query_params, query_params as query,
        response::response_headers,
        route,
    },
    view::{View, component, view},
};

use super::{
    account, cart, document,
    format::{address_lines, money, vat_rate},
};
use crate::{
    Store,
    auth::require_account,
    cart_session::{current_cart, forget_cart, fresh_cart},
    db::{mailer_config, tax_zones},
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
    /// `autoliquidation` when the page priced the order without VAT for a
    /// business of another member state.
    #[serde(default)]
    regime: String,
}

/// What the form's `regime` field says of a reverse-charged order.
const REVERSE_CHARGE_REGIME: &str = "autoliquidation";

#[page(POST "/checkout")]
pub async fn submit(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<impl View> {
    match check_out(cx, form).await? {
        Ok(order_id) => {
            forget_cart(cx);
            Err(see_other(href!(pay, OrderId(order_id)).resolve(cx)).into())
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
    // A business buying without VAT: its number is asked about once more,
    // and the order goes ahead only at the total the page showed — a number
    // the registry no longer confirms (or confirms at last) changes it.
    let policy = ReverseChargePolicy::default();
    refresh_vat_standing(
        &store.executor,
        store.vat_validator.as_ref(),
        &zones,
        zone,
        &account.customer_id,
        &policy,
    )
    .await?;
    let exempt = business_purchase(&store.executor, &zones, zone, &account.customer_id, &policy)
        .await?
        .is_some_and(|business| business.reverse_charge.is_some());
    if exempt != (form.regime == REVERSE_CHARGE_REGIME) {
        return refuse(if exempt {
            "Le numéro de TVA de votre entreprise vient d'être confirmé : la commande est hors TVA. Vérifiez le nouveau total avant de valider."
        } else {
            "Le registre européen ne confirme plus le numéro de TVA de votre entreprise : la commande est soumise à la TVA. Vérifiez le nouveau total avant de valider."
        });
    }
    let payment_mode = match form.payment_mode.as_str() {
        "card" => PaymentMode::Card,
        "installments" => PaymentMode::Installments {
            count: INSTALLMENT_COUNT,
        },
        _ => return refuse("Choisissez un mode de paiement."),
    };
    if payment_mode != PaymentMode::Card && !offers_installments(store) {
        return refuse("Ce mode de paiement n'est pas proposé. Choisissez la carte bancaire.");
    }

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
        "colissimo-europe" => "Colissimo Europe",
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
    let installments = offers_installments(store);
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
    // A business of another member state with a valid VAT number is priced
    // without VAT — what the order will be placed at.
    let business = business_purchase(
        &store.executor,
        &zones,
        pricing_zone,
        &account.customer_id,
        &ReverseChargePolicy::default(),
    )
    .await?;
    let reverse_charge = business
        .as_ref()
        .filter(|business| business.reverse_charge.is_some())
        .map(|business| business.buyer.clone());
    let exempt_zone;
    let pricing_zone = match &reverse_charge {
        Some(_) => {
            exempt_zone = reverse_charged(pricing_zone);
            &exempt_zone
        }
        None => pricing_zone,
    };

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
        TaxTreatment::Export if reverse_charge.is_some() => reverse_charge.as_ref().map(|buyer| {
            format!(
                "{} ({}) — livraison intracommunautaire : les prix ci-dessous sont hors taxes, \
                     la TVA est autoliquidée par votre entreprise dans son pays.",
                buyer.company_name, buyer.vat_number
            )
        }),
        TaxTreatment::Export => Some(format!(
            "Livraison {} : vente hors TVA française. Les prix ci-dessous sont hors taxes ; \
             d'éventuelles taxes locales sont à régler à la réception.",
            pricing_zone.label
        )),
        TaxTreatment::DestinationVat => Some(format!(
            "Livraison {} : les prix ci-dessous incluent la TVA du pays de livraison \
             (taux normal {}) à la place de la TVA française.",
            pricing_zone.label,
            vat_rate(pricing_zone.applied_rate_bp(zones.fee_vat_rate_bp))
        )),
        TaxTreatment::Domestic => None,
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
    let regime = if reverse_charge.is_some() {
        REVERSE_CHARGE_REGIME
    } else {
        ""
    };

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
                    <input type="hidden" name="regime" value=(regime)>
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
                        if installments {
                            <label class="choice">
                                <input type="radio" name="payment_mode" value="installments">
                                <span>"Paiement en " (INSTALLMENT_COUNT.to_string()) " fois (frais de dossier " (money(&handling_fee)) ")"</span>
                            </label>
                        }
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
/// Only what the shop's payment provider can take is offered.
fn offers_installments(store: &Store) -> bool {
    store.provider.supports(&PaymentMethod::Installments {
        count: INSTALLMENT_COUNT,
        fee: timada_core::Money::eur(0),
    })
}

/// Where a shopper stands between checking out and having paid.
enum PayStep {
    /// The order is not placed yet.
    Registering,
    /// Placed; stock is being reserved, or the payment is being recorded.
    Processing,
    /// No provider: the shop validates the payment itself.
    Manual,
    /// The provider's own page.
    Redirect(String),
    /// The provider's embedded form, and where it sends the shopper back.
    Embedded {
        client_secret: String,
        publishable_key: String,
        return_url: String,
    },
    Cancelled(String),
}

/// The shopper's own order, if it exists yet.
async fn own_order(cx: &Cx, id: &str) -> Result<Option<OrderDetailsView>> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let order = load_order_details(&store.executor, id).await?;
    if let Some(order) = &order {
        (order.customer_id == account.customer_id)
            .then_some(())
            .ok_or_not_found()?;
    }
    Ok(order)
}

/// A paid order has nothing left to do here: it goes to its confirmation.
async fn pay_step(cx: &Cx, id: &str, order: Option<&OrderDetailsView>) -> Result<PayStep> {
    let Some(order) = order else {
        return Ok(PayStep::Registering);
    };
    match order.status {
        OrderStatus::Placed => {}
        OrderStatus::Cancelled => {
            let reason = order.cancelled_reason.as_deref().unwrap_or_default();
            return Ok(PayStep::Cancelled(
                cancellation_reason_label(reason).to_owned(),
            ));
        }
        OrderStatus::Paid | OrderStatus::Shipped => {
            let done = href!(confirmation, OrderId(id.to_owned())).resolve(cx);
            return Err(see_other(done).into());
        }
    }

    let store = app_context::<Store>(cx);
    let payment_id = payment_id(id);
    let requested = load_payment(&store.executor, &payment_id)
        .await?
        .is_some_and(|p| p.status == PaymentStatus::Requested);
    if !requested {
        return Ok(PayStep::Processing);
    }
    let urls = ReturnUrls {
        paid: format!(
            "{}{}",
            mailer_config().base_url.trim_end_matches('/'),
            href!(pay, OrderId(id.to_owned())).resolve(cx)
        ),
    };
    let started = start_payment(
        &store.executor,
        &store.db,
        store.provider.as_ref(),
        &payment_id,
        &urls,
    )
    .await;
    match started {
        Ok(PaymentStart::Manual) => Ok(PayStep::Manual),
        Ok(PaymentStart::Redirect(url)) => Ok(PayStep::Redirect(url)),
        Ok(PaymentStart::ClientSecret {
            client_secret,
            publishable_key,
        }) => Ok(PayStep::Embedded {
            client_secret,
            publishable_key,
            return_url: urls.paid,
        }),
        // Captured or declined while the page was loading: look again.
        Err(PaymentError::NotRequested) => Ok(PayStep::Processing),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

/// What the card form may load: Stripe.js, its frames (the card fields, 3-D
/// Secure), its API — and nothing else from outside the shop. The shop's own
/// stylesheet is inline, hence `style-src`.
const PAY_PAGE_CSP: &str = "default-src 'self'; \
    script-src 'self' https://js.stripe.com; \
    frame-src https://js.stripe.com https://hooks.stripe.com; \
    connect-src 'self' https://api.stripe.com; \
    img-src 'self' data: https://*.stripe.com; \
    style-src 'self' 'unsafe-inline'; \
    base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// The card form's script, served by the shop so the page needs no inline
/// script.
#[route(GET "/checkout/pay.js")]
pub async fn pay_script(_cx: &Cx) -> Result<Js<&'static str>> {
    Ok(Js(include_str!("pay.js")))
}

#[page("/checkout/pay/{order_id}")]
pub async fn pay(cx: &Cx) -> Result<impl View> {
    let id = param::<OrderId>(cx)?.clone();
    let order = own_order(cx, &id).await?;
    let step = pay_step(cx, &id, order.as_ref()).await?;
    let summary = order
        .as_ref()
        .map(|o| (o.display_number().to_owned(), money(&o.total)));
    let details = href!(account::order_detail, OrderId(id.clone())).resolve(cx);
    let pay_label = summary
        .as_ref()
        .map(|(_, total)| total.clone())
        .unwrap_or_default();
    if matches!(step, PayStep::Embedded { .. }) {
        // The only page that loads a third party's script says exactly which.
        response_headers(cx).append(
            CONTENT_SECURITY_POLICY,
            topcoat::router::HeaderValue::from_static(PAY_PAGE_CSP),
        );
    }
    let (title, refresh) = match &step {
        PayStep::Registering => ("Commande en cours d'enregistrement", Some(2)),
        PayStep::Processing => ("Commande en cours de traitement", Some(2)),
        PayStep::Manual => ("Commande enregistrée", Some(10)),
        PayStep::Redirect(_) | PayStep::Embedded { .. } => ("Paiement de votre commande", None),
        PayStep::Cancelled(_) => ("Commande annulée", None),
    };

    Ok(view! {
        document(
            title: title,
            refresh: refresh,
            match &step {
                PayStep::Registering => {
                    <h1>"Votre commande est en cours d'enregistrement"</h1>
                    <p role="status">"Cette page se recharge automatiquement."</p>
                }
                PayStep::Processing => {
                    <h1>"Nous préparons votre commande"</h1>
                    <p role="status">"Nous vérifions le stock et votre paiement. Cette page se recharge automatiquement."</p>
                }
                PayStep::Manual => {
                    <h1>"Merci, votre commande est enregistrée"</h1>
                    <p role="status">"Votre paiement est en attente de validation par la boutique."</p>
                }
                PayStep::Redirect(url) => {
                    <h1>"Il ne reste qu'à payer"</h1>
                    <p><a class="button" href=(url.clone())>"Payer ma commande"</a></p>
                }
                PayStep::Embedded { client_secret, publishable_key, return_url } => {
                    <h1>"Il ne reste qu'à payer"</h1>
                    <form id="payment-form" class="stack"
                        data-client-secret=(client_secret.clone())
                        data-publishable-key=(publishable_key.clone())
                        data-return-url=(return_url.clone())>
                        <div id="payment-element"></div>
                        <p id="payment-problem" role="alert" hidden=(true)></p>
                        <button id="payment-submit" type="submit" disabled=(true)>"Payer " (pay_label.clone())</button>
                    </form>
                    <p id="payment-progress" role="status" hidden=(true)></p>
                    <noscript><p role="alert">"Le paiement par carte a besoin de JavaScript. Activez-le, puis rechargez cette page."</p></noscript>
                    <script src="https://js.stripe.com/v3/"></script>
                    <script src=(href!(pay_script)) defer=(true)></script>
                }
                PayStep::Cancelled(reason) => {
                    <h1>"Votre commande a été annulée"</h1>
                    <p role="status">(reason.clone())</p>
                }
            }
            if let Some((number, total)) = &summary {
                <p class="notice">"Commande " <strong>(number.clone())</strong> " — total " (total.clone()) "."</p>
                <p><a href=(details)>"Suivre cette commande"</a></p>
            }
        )
    })
}

/// An order that is not paid yet belongs to the payment step.
async fn paid_order(cx: &Cx, id: &str) -> Result<OrderDetailsView> {
    match own_order(cx, id).await? {
        Some(order) if matches!(order.status, OrderStatus::Paid | OrderStatus::Shipped) => {
            Ok(order)
        }
        _ => Err(see_other(href!(pay, OrderId(id.to_owned())).resolve(cx)).into()),
    }
}

#[page("/checkout/confirmation/{order_id}")]
pub async fn confirmation(cx: &Cx) -> Result<impl View> {
    let id = param::<OrderId>(cx)?.clone();
    let order = paid_order(cx, &id).await?;
    let details = href!(account::order_detail, OrderId(id.clone())).resolve(cx);

    Ok(view! {
        document(
            title: "Commande confirmée",
            <h1>"Merci, votre commande est confirmée"</h1>
            <p class="notice">"Commande " <strong>(order.display_number().to_owned())</strong> " — total " (money(&order.total)) "."</p>
            <p><a href=(details)>"Suivre cette commande"</a></p>
        )
    })
}
