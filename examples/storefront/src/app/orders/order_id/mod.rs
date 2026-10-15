//! `/orders/{order_id}`: an order this browser placed, read from the store.
//! Any other order is a 404: an order id is not a password, and this
//! example has no shopper accounts.

pub mod documents;

use timada_storefront::{
    DocumentKind, OrderStatus, categories, documents as order_documents, format_amount_fr,
    format_money, order,
};
use topcoat::{
    Result,
    context::Cx,
    router::{error::not_found, href, module_param, page, path_param},
    view::{View, view},
};

use crate::app::document;
use crate::shopper::placed;

module_param!(pub order_id);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let order_id = path_param::<OrderId>(cx);
    if !placed(cx, order_id) {
        return Err(not_found().into());
    }
    let Some(order) = order(cx, order_id).await? else {
        return Err(not_found().into());
    };
    let nav = categories(cx).await?;
    let documents = order_documents(cx, order_id).await?;
    let order_key = order_id.to_owned();
    let status = match order.status {
        OrderStatus::Paid => "Paid: the shop is preparing it.",
        OrderStatus::Fulfilled => "On its way.",
        OrderStatus::Cancelled => "Cancelled, and the payment given back.",
    };
    let title = format!("Order {}", order.reference);
    let currency = order.currency.clone();

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <h1>"Thank you!"</h1>
            <p>"Your order " <strong data-order-reference="">(&order.reference)</strong> " is placed."</p>
            <p data-order-status=(order.status.as_str())>(status)</p>
            if let Some(tracking) = &order.tracking {
                <p>"Tracking: " (tracking)</p>
            }
            <table class="cart" data-order="">
                <tbody>
                    for line in &order.lines {
                        <tr>
                            <td>
                                (&line.name)
                                if let Some(label) = &line.variant_label { " · " (label) }
                            </td>
                            <td>"× " (line.quantity)</td>
                            <td>(format_money(line.amount_cents(), &currency))</td>
                        </tr>
                    }
                </tbody>
            </table>
            if let Some(discount) = &order.discount {
                <p data-order-discount=(&discount.code)>"Discount " <code>(&discount.code)</code> ": -" (format_money(discount.amount_cents, &currency))</p>
            }
            <p>"Total: " <strong class="price" data-order-total="">(format_money(order.total_cents, &currency))</strong></p>
            <h2>"Shipping to"</h2>
            <address data-order-address="">
                (&order.shipping.name) <br>
                (&order.shipping.line1) <br>
                if let Some(line2) = &order.shipping.line2 { (line2) <br> }
                (&order.shipping.postal_code) " " (&order.shipping.city) <br>
                (&order.shipping.country)
            </address>
            <p>"A confirmation goes to " (&order.email) "."</p>
            <h2>"Invoice"</h2>
            if documents.is_empty() {
                <p data-invoice-pending="">"Your invoice is being prepared: reload this page in a moment."</p>
            }
            <ul data-documents="">
                for doc in &documents {
                    <li>
                        <a data-document=(&doc.label) href=(href!(documents::document_id::pdf, OrderId(order_key.clone()), documents::document_id::DocumentId(doc.document_id.clone())))>
                            (if doc.kind == DocumentKind::CreditNote { "Credit note " } else { "Invoice " })
                            (&doc.label)
                        </a>
                        " · " (format_amount_fr(doc.total_cents, &doc.currency)) " (PDF)"
                    </li>
                }
            </ul>
        )
    })
}
