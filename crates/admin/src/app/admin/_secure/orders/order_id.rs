//! `/{mount}/orders/{order_id}`: the order, its payment, shipment and
//! fulfillment state, and the operator actions.

use serde::Deserialize;
use timada_core::Address;
use timada_order::{
    FulfillmentStatus, OrderDetailsView, OrderStatus, load_fulfillment, load_order_details,
};
use timada_payment::{PaymentStatus, payment_id};
use timada_shipping::{ShipmentStatus, shipment_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param,
    },
    view::{View, view},
};

use crate::{
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        separator::separator,
    },
    config::AdminServices,
    ui::{date, money, order_status_badge, page_header},
};

path_param!(pub order_id: String, error = not_found);

async fn load(cx: &Cx) -> Result<(String, OrderDetailsView)> {
    let id = param::<OrderId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let order = load_order_details(&services.executor, &id)
        .await?
        .ok_or_not_found()?;
    Ok((id, order))
}

fn back(cx: &Cx, id: &str) -> String {
    href!(show, OrderId(id.to_owned())).resolve(cx)
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let (id, order) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let payment = timada_payment::load_payment(&services.executor, payment_id(&id)).await?;
    let shipment = timada_shipping::load_shipment(&services.executor, shipment_id(&id)).await?;
    let fulfillment = load_fulfillment(&services.executor, &id).await?;

    let payment_label = match &payment {
        Some(p) => format!("{:?}", p.status),
        None if !order.total.is_positive() => "aucun paiement requis".to_owned(),
        None => "non demandé".to_owned(),
    };
    let can_capture = payment
        .as_ref()
        .is_some_and(|p| p.status == PaymentStatus::Requested);
    let can_ship = shipment
        .as_ref()
        .is_some_and(|s| s.status == ShipmentStatus::Created);
    let can_cancel = matches!(order.status, OrderStatus::Placed | OrderStatus::Paid);
    let fulfillment_label = match fulfillment.map(|f| f.status) {
        None => "non démarrée",
        Some(FulfillmentStatus::ReservingStock) => "réservation du stock",
        Some(FulfillmentStatus::AwaitingPayment) => "en attente du paiement",
        Some(FulfillmentStatus::AwaitingShipment) => "en attente d'expédition",
        Some(FulfillmentStatus::Completed) => "terminée",
        Some(FulfillmentStatus::Compensated) => "annulée",
    };

    Ok(view! {
        page_header(
            title: "Commande",
            order_status_badge(status: order.status)
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone()) " · passée le " (date(order.placed_at))</p>

        <div class="grid gap-6 lg:grid-cols-3">
            <div class="flex flex-col gap-6 lg:col-span-2">
                card(
                    card_header(card_title("Articles"))
                    card_content(
                        <table class="w-full text-sm">
                            <tbody>
                                for line in &order.lines {
                                    <tr class="border-b border-border last:border-0">
                                        <td class="py-2">(line.name.clone()) <span class="ml-2 text-muted-foreground">"× " (line.quantity.to_string())</span></td>
                                        <td class="py-2 text-right tabular-nums">(money(&line.unit_price))</td>
                                    </tr>
                                }
                                <tr><td class="pt-3 text-muted-foreground">"Sous-total"</td><td class="pt-3 text-right tabular-nums">(money(&order.subtotal))</td></tr>
                                <tr><td class="text-muted-foreground">"Frais de port"</td><td class="text-right tabular-nums">(money(&order.shipping_fee))</td></tr>
                                <tr><td class="text-muted-foreground">"Frais de dossier"</td><td class="text-right tabular-nums">(money(&order.handling_fee))</td></tr>
                                if let Some(discount) = &order.discount {
                                    <tr><td class="text-muted-foreground">"Remise (" (discount.code.clone()) ")"</td><td class="text-right tabular-nums">"− " (money(&discount.amount))</td></tr>
                                }
                                <tr class="font-semibold"><td class="pt-2">"Total"</td><td class="pt-2 text-right tabular-nums">(money(&order.total))</td></tr>
                            </tbody>
                        </table>
                    )
                )
                card(
                    card_header(card_title("Livraison"))
                    card_content(
                        <dl class="grid gap-2 text-sm sm:grid-cols-2">
                            <div><dt class="text-muted-foreground">"Mode"</dt><dd>(order.delivery.method_code.clone())</dd></div>
                            <div><dt class="text-muted-foreground">"Suivi"</dt><dd>(order.tracking_number.clone().unwrap_or_else(|| "—".into()))</dd></div>
                            <div><dt class="text-muted-foreground">"Adresse de livraison"</dt><dd>address_lines(address: &order.delivery_address)</dd></div>
                            <div><dt class="text-muted-foreground">"Adresse de facturation"</dt><dd>address_lines(address: &order.billing_address)</dd></div>
                        </dl>
                    )
                )
            </div>

            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("État"))
                    card_content(
                        <dl class="flex flex-col gap-2 text-sm">
                            <div><dt class="text-muted-foreground">"Client"</dt><dd class="font-mono text-xs">(order.customer_id.clone())</dd></div>
                            <div><dt class="text-muted-foreground">"Paiement"</dt><dd>(payment_label)</dd></div>
                            <div><dt class="text-muted-foreground">"Expédition"</dt><dd>(shipment.as_ref().map(|s| format!("{:?}", s.status)).unwrap_or_else(|| "—".into()))</dd></div>
                            <div><dt class="text-muted-foreground">"Traitement"</dt><dd>(fulfillment_label)</dd></div>
                            if let Some(reason) = &order.cancelled_reason {
                                <div><dt class="text-muted-foreground">"Motif d'annulation"</dt><dd>(reason.clone())</dd></div>
                            }
                        </dl>
                    )
                )
                card(
                    card_header(card_title("Actions"))
                    card_content(
                        <div class="flex flex-col gap-3">
                            if can_capture {
                                <form method="post" action=(href!(capture_payment, OrderId(id.clone())))>
                                    button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Encaisser le paiement")
                                </form>
                            }
                            if can_ship {
                                <form method="post" action=(href!(ship, OrderId(id.clone()))) class="flex flex-col gap-2">
                                    input(attrs: topcoat::view::attributes! { name="carrier" placeholder="Transporteur" required=(true) value="Chronopost" })
                                    input(attrs: topcoat::view::attributes! { name="tracking_number" placeholder="N° de suivi" required=(true) })
                                    button(attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Expédier")
                                </form>
                            }
                            if order.status != OrderStatus::Cancelled {
                                <form method="post" action=(href!(resend_confirmation, OrderId(id.clone())))>
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Renvoyer la confirmation")
                                </form>
                            }
                            if can_cancel {
                                separator()
                                <form method="post" action=(href!(cancel, OrderId(id.clone()))) class="flex flex-col gap-2">
                                    input(attrs: topcoat::view::attributes! { name="reason" placeholder="Motif" required=(true) })
                                    button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Annuler la commande")
                                </form>
                            }
                        </div>
                    )
                )
            </div>
        </div>
    })
}

#[topcoat::view::component]
async fn address_lines(address: &Address) -> Result<impl View> {
    Ok(view! {
        <span class="block">(address.full_name())</span>
        <span class="block">(address.line1.clone())</span>
        if let Some(line2) = &address.line2 { <span class="block">(line2.clone())</span> }
        <span class="block">(address.postal_code.clone()) " " (address.city.clone()) ", " (address.country_code.clone())</span>
    })
}

#[derive(Debug, Deserialize)]
pub struct ShipForm {
    carrier: String,
    tracking_number: String,
}

/// Hands the parcel to the carrier; the fulfillment saga marks the order shipped.
#[page(POST "./ship")]
pub async fn ship(cx: &Cx, Form(form): Form<ShipForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_shipping::Command(&services.executor)
        .dispatch_shipment(shipment_id(&id), form.carrier, form.tracking_number)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct CancelForm {
    reason: String,
}

#[page(POST "./cancel")]
pub async fn cancel(cx: &Cx, Form(form): Form<CancelForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_order::Command(&services.executor)
        .cancel_order(&id, form.reason)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[page(POST "./resend")]
pub async fn resend_confirmation(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_order::Command(&services.executor)
        .resend_confirmation(&id)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

/// Stands in for the payment provider's capture callback until one is wired.
#[page(POST "./capture-payment")]
pub async fn capture_payment(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_payment::Command(&services.executor)
        .capture_payment(payment_id(&id), format!("manual-{id}"))
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}
