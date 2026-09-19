//! `/{mount}/invoices/{invoice_id}`: one invoice as it will be printed —
//! lines, fees, reduction and total. Read-only: orders drive its lifecycle.

use timada_invoice::load_invoice;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::RouterErrorExt, href, page, path_param, path_param as param},
    view::{View, view},
};

use super::invoice_status_badge;
use crate::{
    app::admin::_secure::{
        customers::customer_id,
        orders::order_id::{self, address_lines},
    },
    components::card::{card, card_content, card_header, card_title},
    config::AdminServices,
    ui::{money, page_header},
};

path_param!(pub invoice_id: String, error = not_found);

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<InvoiceId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let invoice = load_invoice(&services.executor, &id)
        .await?
        .ok_or_not_found()?;

    let title = match &invoice.invoice_number {
        Some(number) => format!("Facture {number}"),
        None => "Facture non numérotée".to_owned(),
    };
    let order_link = href!(order_id::show, order_id::OrderId(invoice.order_id.clone())).resolve(cx);
    let customer_link = href!(
        customer_id::show,
        customer_id::CustomerId(invoice.customer_id.clone())
    )
    .resolve(cx);
    let mut lines = Vec::with_capacity(invoice.lines.len());
    for line in &invoice.lines {
        lines.push((
            line.label.clone(),
            line.quantity.to_string(),
            money(&line.unit_price),
            money(&line.total()?),
        ));
    }

    Ok(view! {
        page_header(
            title: &title,
            invoice_status_badge(status: invoice.status)
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone())</p>

        <div class="grid gap-6 lg:grid-cols-3">
            <div class="lg:col-span-2">
                card(
                    card_header(card_title("Lignes"))
                    card_content(
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-left text-muted-foreground">
                                    <th scope="col" class="py-2 font-normal">"Désignation"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Qté"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Prix unitaire TTC"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Total TTC"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (label, quantity, unit_price, total) in &lines {
                                    <tr class="border-b border-border">
                                        <td class="py-2">(label.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(quantity.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(unit_price.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(total.clone())</td>
                                    </tr>
                                }
                            </tbody>
                            <tfoot>
                                <tr><td colspan="3" class="pt-3 text-muted-foreground">"Sous-total"</td><td class="pt-3 text-right tabular-nums">(money(&invoice.subtotal))</td></tr>
                                <tr><td colspan="3" class="text-muted-foreground">"Frais de port"</td><td class="text-right tabular-nums">(money(&invoice.shipping_fee))</td></tr>
                                <tr><td colspan="3" class="text-muted-foreground">"Frais de dossier"</td><td class="text-right tabular-nums">(money(&invoice.handling_fee))</td></tr>
                                if let Some(discount) = &invoice.discount {
                                    <tr><td colspan="3" class="text-muted-foreground">(discount.label.clone())</td><td class="text-right tabular-nums">"− " (money(&discount.amount))</td></tr>
                                }
                                <tr class="font-semibold"><td colspan="3" class="pt-2">"Total TTC"</td><td class="pt-2 text-right tabular-nums">(money(&invoice.total))</td></tr>
                            </tfoot>
                        </table>
                    )
                )
            </div>

            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("Facturé à"))
                    card_content(
                        <div class="text-sm">address_lines(address: &invoice.billing_address)</div>
                    )
                )
                card(
                    card_header(card_title("Références"))
                    card_content(
                        <dl class="flex flex-col gap-2 text-sm">
                            <div><dt class="text-muted-foreground">"Commande"</dt><dd><a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(invoice.order_id.clone())</a></dd></div>
                            <div><dt class="text-muted-foreground">"Client"</dt><dd><a href=(customer_link) class="font-mono text-xs underline-offset-4 hover:underline">(invoice.customer_id.clone())</a></dd></div>
                            if let Some(reason) = &invoice.voided_reason {
                                <div><dt class="text-muted-foreground">"Motif d'annulation"</dt><dd>(reason.clone())</dd></div>
                            }
                        </dl>
                    )
                )
            </div>
        </div>
    })
}
