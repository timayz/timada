//! `/{mount}/orders/{order_id}`: the order, its payment, shipment and
//! fulfillment state, and the operator actions — refunds included.

use serde::Deserialize;
use timada_core::{Address, Money};
use timada_invoice::{invoice_id as invoice_id_of, load_invoice};
use timada_order::{
    FulfillmentStatus, OrderDetailsView, OrderStatus, load_fulfillment, load_order_details,
};
use timada_payment::{DisputeStatus, PaymentError, PaymentStatus, RefundStatus, payment_id};
use timada_shipping::{ShipmentStatus, shipment_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param, query_params, query_params as query,
    },
    view::{View, view},
};

use crate::{
    app::admin::_secure::{
        disputes::dispute_status_badge, invoices::invoice_id, returns::return_id,
    },
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        separator::separator,
    },
    config::{AdminConfig, AdminServices},
    ui::{date, money, order_status_badge, page_header, vat_rate},
};

path_param!(pub order_id: String, error = not_found);

/// Set by [`refund`] when the payment context refuses the refund.
#[query_params(error = bad_request)]
struct ShowQuery {
    refund_error: Option<String>,
}

fn refund_error_message(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "exceeds" => Some("Le remboursement dépasse le montant encaissé restant."),
        "amount" => Some("Le montant à rembourser doit être positif."),
        "state" => Some("Seul un paiement encaissé peut être remboursé."),
        "stale" => Some("Ce remboursement n'attend plus cette action."),
        "disputed" => Some(
            "Un litige bancaire est en cours sur ce paiement : ni expédition ni remboursement avant la décision de la banque.",
        ),
        "rate" => {
            Some("Aucun cours de change n'a pu être obtenu pour cette devise. Réessayez plus tard.")
        }
        "credit" => Some(
            "L'avoir n'a pas pu être émis : la facture n'est pas émise, ou ses avoirs couvrent déjà son montant.",
        ),
        _ => None,
    }
}

/// One dispute of the order's payment, worded.
struct DisputeLine {
    reference: String,
    amount: String,
    reason: String,
    opened_on: String,
    /// When the evidence is due, while the dispute is open.
    respond_by: Option<String>,
    status: DisputeStatus,
    /// Lost, and not documented by a credit note yet.
    can_credit: bool,
    credit_note: Option<String>,
}

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
    let invoice = load_invoice(&services.executor, invoice_id_of(&id)).await?;
    let invoice_link = invoice.map(|i| {
        let label = i
            .invoice_number
            .unwrap_or_else(|| "non numérotée".to_owned());
        let link = href!(invoice_id::show, invoice_id::InvoiceId(i.id)).resolve(cx);
        (link, label)
    });
    let title = format!("Commande {}", order.display_number());
    let tax_zone = order.tax.as_ref().map(|tax| tax.zone_code.clone());
    let vat_lines: Vec<(String, String, String)> = order
        .tax
        .iter()
        .flat_map(|tax| &tax.vat_lines)
        .map(|line| (vat_rate(line.rate_bp), money(&line.base), money(&line.vat)))
        .collect();
    let vat_mention = order.regime_mention();
    // The business the order is for, as its invoice will name it.
    let business = order
        .buyer
        .as_ref()
        .map(|buyer| format!("{} — {}", buyer.company_name, buyer.vat_number));
    let order_returns: Vec<(String, String)> = timada_returns::returns_of_order(&services.db, &id)
        .await?
        .into_iter()
        .map(|row| {
            let link = href!(return_id::show, return_id::ReturnId(row.return_id)).resolve(cx);
            (link, row.rma_number)
        })
        .collect();
    let refund_error = refund_error_message(query::<ShowQuery>(cx)?.refund_error.as_deref());

    // An order in another currency than the books: the rate it goes to them
    // at — or that it still lacks one, which keeps it out of the VAT report.
    let base_currency = app_context::<AdminConfig>(cx).currencies.base().to_owned();
    let exchange_rate = order
        .exchange_rate
        .as_ref()
        .map(|rate| format!("{} — {} du {}", rate.quote(), rate.source, date(rate.as_of)));
    let rate_missing = order.exchange_rate.is_none() && order.total.currency != base_currency;
    let can_pin_rate = rate_missing && services.exchange_rates.is_some();
    // Said where the button is: this page has no refund section to say it in.
    let rate_error = refund_error.filter(|_| {
        query::<ShowQuery>(cx).is_ok_and(|q| q.refund_error.as_deref() == Some("rate"))
    });

    // The payment's disputes. An open one holds the order: no parcel, no
    // refund. A lost one may be documented by a credit note — the operator's
    // decision, once per dispute.
    let disputed = payment.as_ref().is_some_and(|p| p.open_dispute().is_some());
    let mut disputes = Vec::new();
    for dispute in payment.iter().flat_map(|p| &p.disputes) {
        let credit_note = timada_invoice::load_credit_note(
            &services.executor,
            timada_invoice::credit_note_id(&timada_invoice::dispute_credit_reference(
                &dispute.dispute_id,
            )),
        )
        .await?
        .map(|note| note.credit_note_number);
        disputes.push(DisputeLine {
            reference: dispute.dispute_id.clone(),
            amount: money(&dispute.amount),
            reason: timada_payment::dispute_reason_label(&dispute.reason).to_owned(),
            opened_on: date(dispute.opened_at),
            respond_by: dispute
                .respond_by
                .filter(|_| dispute.status == DisputeStatus::Open)
                .map(date),
            status: dispute.status,
            can_credit: dispute.status == DisputeStatus::Lost && credit_note.is_none(),
            credit_note,
        });
    }

    // What is still refundable, in cents, once the payment is captured.
    // Refunds on their way to the provider already hold their amount.
    let refundable = match payment
        .as_ref()
        .filter(|p| p.status == PaymentStatus::Captured)
    {
        Some(p) if !disputed => {
            Some(p.refundable().map_err(anyhow::Error::from)?.minor).filter(|left| *left > 0)
        }
        _ => None,
    };
    // Refunds asked for that did not go back yet: `(id, amount, reason,
    // failure)` — a failure is what the operator can act on.
    let open_refunds: Vec<(String, String, String, Option<String>)> = payment
        .iter()
        .flat_map(|p| &p.refunds)
        .filter(|r| r.status != RefundStatus::Settled)
        .map(|r| {
            let failure = (r.status == RefundStatus::Failed)
                .then(|| r.failure.clone().unwrap_or_else(|| "refusé".to_owned()));
            (
                r.refund_id.clone(),
                money(&r.amount),
                r.reason.clone(),
                failure,
            )
        })
        .collect();
    let refunded = payment
        .as_ref()
        .filter(|p| p.refunded.is_positive())
        .map(|p| money(&p.refunded));
    let payment_label = match &payment {
        Some(p) => format!("{:?}", p.status),
        None if !order.total.is_positive() => "aucun paiement requis".to_owned(),
        None => "non demandé".to_owned(),
    };
    let can_capture = payment
        .as_ref()
        .is_some_and(|p| p.status == PaymentStatus::Requested);
    let can_ship = order.status != OrderStatus::Cancelled
        && !disputed
        && shipment
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
            title: &title,
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
                                for (rate, base, vat) in &vat_lines {
                                    <tr class="text-muted-foreground"><td>"TVA " (rate.clone()) " sur " (base.clone())</td><td class="text-right tabular-nums">(vat.clone())</td></tr>
                                }
                            </tbody>
                        </table>
                        if let Some(mention) = vat_mention { <p class="mt-3 text-xs text-muted-foreground">(mention)</p> }
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
                            if let Some(zone) = &tax_zone {
                                <div><dt class="text-muted-foreground">"Zone fiscale"</dt><dd class="font-mono text-xs">(zone.clone())</dd></div>
                            }
                            if let Some(business) = &business {
                                <div><dt class="text-muted-foreground">"Entreprise"</dt><dd>(business.clone())</dd></div>
                            }
                            if let Some(rate) = &exchange_rate {
                                <div><dt class="text-muted-foreground">"Cours de change"</dt><dd>(rate.clone())</dd></div>
                            }
                            if rate_missing {
                                <div>
                                    <dt class="text-muted-foreground">"Cours de change"</dt>
                                    <dd>
                                        <span role="alert" class="text-destructive">"Aucun cours épinglé : cette vente en devise étrangère reste hors du rapport de TVA."</span>
                                        if let Some(error) = rate_error {
                                            <span role="alert" class="mt-1 block text-destructive">(error)</span>
                                        }
                                        if can_pin_rate {
                                            <form method="post" action=(href!(pin_rate, OrderId(id.clone()))) class="mt-2">
                                                button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Épingler le cours du jour")
                                            </form>
                                        }
                                    </dd>
                                </div>
                            }
                            <div><dt class="text-muted-foreground">"Paiement"</dt><dd>(payment_label)</dd></div>
                            if let Some(refunded) = &refunded {
                                <div><dt class="text-muted-foreground">"Remboursé"</dt><dd class="tabular-nums">(refunded.clone())</dd></div>
                            }
                            if let Some((link, label)) = &invoice_link {
                                <div><dt class="text-muted-foreground">"Facture"</dt><dd><a href=(link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(label.clone())</a></dd></div>
                            }
                            <div><dt class="text-muted-foreground">"Expédition"</dt><dd>(shipment.as_ref().map(|s| format!("{:?}", s.status)).unwrap_or_else(|| "—".into()))</dd></div>
                            <div><dt class="text-muted-foreground">"Traitement"</dt><dd>(fulfillment_label)</dd></div>
                            if let Some(reason) = &order.cancelled_reason {
                                <div><dt class="text-muted-foreground">"Motif d'annulation"</dt><dd>(reason.clone())</dd></div>
                            }
                            if !order_returns.is_empty() {
                                <div>
                                    <dt class="text-muted-foreground">"Retours"</dt>
                                    <dd class="flex flex-wrap gap-2">
                                        for (link, number) in &order_returns {
                                            <a href=(link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(number.clone())</a>
                                        }
                                    </dd>
                                </div>
                            }
                        </dl>
                    )
                )
                if !disputes.is_empty() {
                    card(
                        card_header(card_title("Litige bancaire"))
                        card_content(
                            if disputed {
                                <p role="alert" class="mb-3 text-sm text-destructive">"Paiement contesté : la commande est retenue, aucun remboursement ne part avant la décision de la banque."</p>
                            }
                            <ul class="flex flex-col gap-4 text-sm">
                                for dispute in &disputes {
                                    <li class="flex flex-col gap-1">
                                        <span class="flex items-center justify-between gap-2">
                                            <span class="tabular-nums">(dispute.amount.clone())</span>
                                            dispute_status_badge(status: dispute.status)
                                        </span>
                                        <span>(dispute.reason.clone())</span>
                                        <span class="text-muted-foreground">"Ouvert le " (dispute.opened_on.clone()) " · " <span class="font-mono text-xs">(dispute.reference.clone())</span></span>
                                        if let Some(respond_by) = &dispute.respond_by {
                                            <span>"Justificatifs à transmettre au prestataire avant le " <strong>(respond_by.clone())</strong></span>
                                        }
                                        if let Some(number) = &dispute.credit_note {
                                            <span class="text-muted-foreground">"Documenté par l'avoir " <span class="font-mono text-xs">(number.clone())</span></span>
                                        }
                                        if dispute.can_credit {
                                            <span class="text-muted-foreground">"La banque a rendu cette somme au client. Si la vente est annulée, un avoir la sort du chiffre d'affaires et de la TVA ; si c'est une créance irrécouvrable, n'en émettez pas."</span>
                                            <form method="post" action=(href!(credit_dispute, OrderId(id.clone())))>
                                                <input type="hidden" name="dispute_id" value=(dispute.reference.clone())>
                                                button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Émettre un avoir")
                                            </form>
                                        }
                                    </li>
                                }
                            </ul>
                            if refundable.is_none() && open_refunds.is_empty() {
                                if let Some(error) = refund_error {
                                    <p role="alert" class="mt-3 text-sm text-destructive">(error)</p>
                                }
                            }
                        )
                    )
                }
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
                            if !open_refunds.is_empty() {
                                separator()
                                <h3 class="text-sm font-medium">"Remboursements en cours"</h3>
                                <ul class="flex flex-col gap-3 text-sm">
                                    for (refund_id, amount, reason, failure) in &open_refunds {
                                        <li class="flex flex-col gap-2">
                                            <span><span class="tabular-nums">(amount.clone())</span> " — " (reason.clone())</span>
                                            match failure {
                                                None => { <span class="text-muted-foreground">"Transmis au prestataire de paiement, en attente de confirmation."</span> }
                                                Some(failure) => {
                                                    <span role="alert" class="text-destructive">"Refusé par le prestataire : " (failure.clone())</span>
                                                    <form method="post" action=(href!(retry_refund, OrderId(id.clone())))>
                                                        <input type="hidden" name="refund_id" value=(refund_id.clone())>
                                                        button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Relancer le remboursement")
                                                    </form>
                                                    <form method="post" action=(href!(settle_refund, OrderId(id.clone()))) class="flex flex-col gap-2">
                                                        <input type="hidden" name="refund_id" value=(refund_id.clone())>
                                                        input(attrs: topcoat::view::attributes! { name="reference" placeholder="Référence du virement" aria-label="Référence du remboursement fait à la main" required=(true) })
                                                        button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Remboursé par un autre moyen")
                                                    </form>
                                                }
                                            }
                                        </li>
                                    }
                                </ul>
                                if refundable.is_none() {
                                    if let Some(error) = refund_error {
                                        <p role="alert" class="text-sm text-destructive">(error)</p>
                                    }
                                }
                            }
                            if let Some(left) = refundable {
                                separator()
                                <form method="post" action=(href!(refund, OrderId(id.clone()))) class="flex flex-col gap-2">
                                    <label for="refund-amount" class="text-sm text-muted-foreground">"Montant à rembourser (centimes)"</label>
                                    input(attrs: topcoat::view::attributes! { id="refund-amount" name="amount_cents" type="number" min="1" max=(left.to_string()) value=(left.to_string()) required=(true) })
                                    input(attrs: topcoat::view::attributes! { name="reason" placeholder="Motif du remboursement" aria-label="Motif du remboursement" required=(true) })
                                    if let Some(error) = refund_error {
                                        <p role="alert" class="text-sm text-destructive">(error)</p>
                                    }
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Rembourser")
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
pub async fn address_lines(address: &Address) -> Result<impl View> {
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
    if is_disputed(cx, &id).await? {
        let target = format!("{}?refund_error=disputed", back(cx, &id));
        return Err::<(), _>(see_other(target).into());
    }
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

/// Records a payment the shop received by its own means (no provider, a bank
/// transfer…); with a provider, captures come from its events instead.
#[page(POST "./capture-payment")]
pub async fn capture_payment(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_payment::Command(&services.executor)
        .capture_payment(payment_id(&id), format!("manual-{id}"))
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct RefundForm {
    amount_cents: i64,
    reason: String,
}

/// Asks for part or all of the captured payment to be given back; the refund
/// worker hands it to the provider. What the payment context refuses comes
/// back as a message on the order page.
#[page(POST "./refund")]
pub async fn refund(cx: &Cx, Form(form): Form<RefundForm>) -> Result<impl View> {
    let (id, order) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    // The payment context would take the request and hold it; an operator is
    // told no instead — the money is with the bank.
    if is_disputed(cx, &id).await? {
        let target = format!("{}?refund_error=disputed", back(cx, &id));
        return Err::<(), _>(see_other(target).into());
    }
    let refunded = timada_payment::Command(&services.executor)
        .refund_payment(
            payment_id(&id),
            Money::new(form.amount_cents, &order.total.currency),
            form.reason.trim().to_owned(),
        )
        .await;
    let refused = match refunded {
        Ok(_) => None,
        Err(PaymentError::RefundExceedsCapture) => Some("exceeds"),
        Err(PaymentError::InvalidAmount) => Some("amount"),
        Err(PaymentError::NotCaptured | PaymentError::PaymentNotFound) => Some("state"),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    let target = match refused {
        Some(code) => format!("{}?refund_error={code}", back(cx, &id)),
        None => back(cx, &id),
    };
    Err::<(), _>(see_other(target).into())
}

fn refund_outcome(cx: &Cx, id: &str, outcome: Result<(), PaymentError>) -> Result<()> {
    let refused = match outcome {
        Ok(()) => None,
        Err(PaymentError::RefundExceedsCapture) => Some("exceeds"),
        Err(
            PaymentError::RefundNotFound
            | PaymentError::RefundNotFailed
            | PaymentError::RefundAlreadySettled,
        ) => Some("stale"),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    let target = match refused {
        Some(code) => format!("{}?refund_error={code}", back(cx, id)),
        None => back(cx, id),
    };
    Err(see_other(target).into())
}

#[derive(Debug, Deserialize)]
pub struct RetryRefundForm {
    refund_id: String,
}

/// Asks the provider again for a refund it refused.
#[page(POST "./refunds/retry")]
pub async fn retry_refund(cx: &Cx, Form(form): Form<RetryRefundForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_payment::Command(&services.executor)
        .retry_refund(payment_id(&id), &form.refund_id)
        .await;
    refund_outcome(cx, &id, outcome)
}

#[derive(Debug, Deserialize)]
pub struct SettleRefundForm {
    refund_id: String,
    reference: String,
}

/// The money of a refused refund went back some other way (a bank transfer):
/// the refund is settled with the operator's reference.
#[page(POST "./refunds/settle")]
pub async fn settle_refund(cx: &Cx, Form(form): Form<SettleRefundForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_payment::Command(&services.executor)
        .settle_refund(
            payment_id(&id),
            &form.refund_id,
            format!("manual-{}", form.reference.trim()),
        )
        .await
        .map(|_| ());
    refund_outcome(cx, &id, outcome)
}

/// Whether the order's payment has a dispute the bank has not decided.
async fn is_disputed(cx: &Cx, order_id: &str) -> Result<bool> {
    let services = app_context::<AdminServices>(cx);
    Ok(
        timada_payment::load_payment(&services.executor, payment_id(order_id))
            .await?
            .is_some_and(|payment| payment.open_dispute().is_some()),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreditDisputeForm {
    dispute_id: String,
}

/// The operator's decision on a lost dispute: the sale is cancelled, a
/// credit note documents what the bank gave back. One per dispute — asking
/// again changes nothing.
#[page(POST "./disputes/credit-note")]
pub async fn credit_dispute(cx: &Cx, Form(form): Form<CreditDisputeForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let lost = timada_payment::load_payment(&services.executor, payment_id(&id))
        .await?
        .and_then(|payment| {
            payment
                .disputes
                .into_iter()
                .find(|d| d.dispute_id == form.dispute_id && d.status == DisputeStatus::Lost)
        });
    let Some(dispute) = lost else {
        let target = format!("{}?refund_error=stale", back(cx, &id));
        return Err::<(), _>(see_other(target).into());
    };
    let reference = timada_invoice::dispute_credit_reference(&dispute.dispute_id);
    let issued = timada_invoice::Command {
        executor: &services.executor,
        db: services.db.clone(),
    }
    .issue_credit_note(timada_invoice::IssueCreditNote {
        refund_id: reference.clone(),
        invoice_id: invoice_id_of(&id),
        amount: dispute.amount,
        reason: reference,
    })
    .await;
    let target = match issued {
        Ok(_) => back(cx, &id),
        Err(
            timada_invoice::InvoiceError::CreditExceedsInvoice
            | timada_invoice::InvoiceError::InvoiceNotIssued
            | timada_invoice::InvoiceError::InvoiceVoided
            | timada_invoice::InvoiceError::InvoiceNotFound,
        ) => format!("{}?refund_error=credit", back(cx, &id)),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Err::<(), _>(see_other(target).into())
}

/// Pins today's rate on an order placed while no rate could be had. The
/// first rate stays: asking again changes nothing.
#[page(POST "./exchange-rate")]
pub async fn pin_rate(cx: &Cx) -> Result<impl View> {
    let (id, order) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let base = app_context::<AdminConfig>(cx).currencies.base().to_owned();
    let rate = match &services.exchange_rates {
        Some(source) if order.exchange_rate.is_none() && order.total.currency != base => source
            .0
            .rate(
                &base,
                &order.total.currency,
                timada_core::time::now_unix_secs()?,
            )
            .await
            .map_err(|error| tracing::warn!(order_id = %id, %error, "no exchange rate"))
            .ok(),
        _ => None,
    };
    let target = match rate {
        Some(rate) => {
            timada_order::Command(&services.executor)
                .pin_exchange_rate(&id, rate)
                .await
                .map_err(anyhow::Error::from)?;
            back(cx, &id)
        }
        None if order.exchange_rate.is_some() => back(cx, &id),
        None => format!("{}?refund_error=rate", back(cx, &id)),
    };
    Err::<(), _>(see_other(target).into())
}
