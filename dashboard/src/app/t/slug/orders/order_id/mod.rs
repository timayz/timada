//! `/t/{slug}/orders/{order_id}`: one paid order, read from its own stream
//! (so a change shows at once), with what the merchant does next: mark it
//! fulfilled, or cancel it.

pub mod cancel;
pub mod fulfil;

use timada_catalog::format_money;
use timada_invoicing::{DocumentKind, format_amount_fr};
use timada_orders::{MAX_REASON_LEN, MAX_TRACKING_LEN, OrderState, OrderStatus, OrdersError};
use timada_tenancy::{PERMISSION_ORDERS_MANAGE, PERMISSION_ORDERS_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use super::orders;
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::ui::{document, format_time, notice};

module_param!(pub order_id);

#[topcoat::router::query_params(error = bad_request)]
struct OrderQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn order_url(
    cx: &Cx,
    slug: &str,
    order_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), OrderId(order_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The order named in the URL, or 404.
pub(crate) async fn require_order(cx: &Cx, access: &TenantAccess) -> Result<OrderState> {
    let id = path_param::<OrderId>(cx);
    orders(access)?
        .load_order(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on an order shares: the member may manage orders, and
/// the outcome goes back to the order page.
pub(crate) async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_orders::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), OrdersError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = orders(&access)?;
    match run(&gateway, &access.user_id, &order_id).await {
        Ok(()) => Ok(see_other(order_url(
            cx,
            &slug,
            &order_id,
            None,
            Some(message),
        ))),
        Err(err) => match err.code() {
            Some(code) => Ok(see_other(order_url(cx, &slug, &order_id, Some(code), None))),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}

/// A line of the order, ready to render.
struct LineRow {
    name: String,
    variant: Option<String>,
    sku: Option<String>,
    quantity: u32,
    unit: String,
    amount: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_ORDERS_MANAGE).await?;
    let query = query_params::<OrderQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let order = require_order(cx, &access).await?;
    // Read from their own streams, like the order: issued a moment after
    // the payment (or the refund), they show as soon as they exist.
    let invoicing = crate::app::t::slug::invoices::invoicing(&access)?;
    let mut documents = Vec::new();
    let invoice = invoicing
        .load_invoice_of_order(&order.id)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    documents.extend(invoice);
    for refund_id in &order.refund_ids {
        let note = invoicing
            .load_credit_note_of_refund(refund_id)
            .await
            .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
        documents.extend(note);
    }
    let confirmation = orders(&access)?
        .load_order_confirmation(&order.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let review_request = crate::app::t::slug::reviews::reviews(&access)?
        .load_review_request(&order.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let lines: Vec<LineRow> = order
        .lines
        .iter()
        .map(|line| LineRow {
            name: line.name.clone(),
            variant: line.variant_label.clone(),
            sku: line.sku.clone(),
            quantity: line.quantity,
            unit: format_money(line.unit_amount_cents, &order.currency),
            amount: format_money(line.amount_cents(), &order.currency),
        })
        .collect();
    let total = format_money(order.total_cents, &order.currency);
    // The code it was paid with and what it took off, as `-2.50 EUR`.
    let discount = order.discount.as_ref().map(|discount| {
        (
            discount.code.clone(),
            format!("-{}", format_money(discount.amount_cents, &order.currency)),
        )
    });
    let shipping = order.shipping_charge.as_ref().map(|shipping| {
        (
            shipping.name.clone(),
            match shipping.amount_cents {
                0 => "Free".to_owned(),
                cents => format_money(cents, &order.currency),
            },
        )
    });
    let status = order.status;
    let address = order.shipping.clone();
    let billing = order.billing.clone();
    let order_id = order.id.clone();

    let can_fulfil = can_manage && status == OrderStatus::Paid;
    let payment = format!(
        "{} · {}",
        format_money(order.payment.amount_cents, &order.payment.currency),
        order.payment.payment_id
    );
    let refunded =
        (order.refunded_cents > 0).then(|| format_money(order.refunded_cents, &order.currency));
    let can_cancel = can_manage && status == OrderStatus::Paid;
    let title = format!("Order {} · {}", order.reference, tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Order " <span class="font-mono">(&order.reference)</span></h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <p class="mb-4 text-sm">
                <a class="underline" href=(href!(super::index, Slug(slug.clone())))>"All orders"</a>
            </p>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Status"</dt>
                    <dd data-order-status=(status.as_str())>(status.label())</dd>
                    <dt class="text-neutral-500">"Placed"</dt>
                    <dd><time datetime=(order.placed_at)>(format_time(order.placed_at))</time></dd>
                    <dt class="text-neutral-500">"Payment"</dt>
                    <dd data-order-payment="">(&payment)</dd>
                    if let Some(refunded) = &refunded {
                        <dt class="text-neutral-500">"Refunded"</dt>
                        <dd data-order-refunded="">(refunded)</dd>
                    }
                    if let Some(at) = order.fulfilled_at {
                        <dt class="text-neutral-500">"Fulfilled"</dt>
                        <dd><time datetime=(at)>(format_time(at))</time></dd>
                    }
                    if let Some(tracking) = &order.tracking {
                        <dt class="text-neutral-500">"Tracking"</dt>
                        <dd data-order-tracking="">(tracking)</dd>
                    }
                    if let Some(at) = order.cancelled_at {
                        <dt class="text-neutral-500">"Cancelled"</dt>
                        <dd><time datetime=(at)>(format_time(at))</time></dd>
                    }
                    if let Some(reason) = &order.cancel_reason {
                        <dt class="text-neutral-500">"Reason"</dt>
                        <dd data-order-cancel-reason="">(reason)</dd>
                    }
                    <dt class="text-neutral-500">"E-mail"</dt>
                    <dd><a class="underline" href=(format!("mailto:{}", order.email))>(&order.email)</a></dd>
                    <dt class="text-neutral-500">"Confirmation"</dt>
                    <dd data-order-confirmation=(if confirmation.is_some() { "mailed" } else { "none" })>
                        if let Some(mailed) = &confirmation {
                            "Mailed " <time datetime=(mailed.mailed_at)>(format_time(mailed.mailed_at))</time>
                        } else {
                            "Not mailed"
                        }
                    </dd>
                    <dt class="text-neutral-500">"Review request"</dt>
                    <dd data-order-review-request=(if review_request.is_some() { "mailed" } else { "none" })>
                        if let Some(mailed) = &review_request {
                            "Mailed " <time datetime=(mailed.mailed_at)>(format_time(mailed.mailed_at))</time>
                        } else {
                            "Not mailed"
                        }
                    </dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-order-documents="">
                <h2 class="mb-2 text-lg font-medium">"Invoices"</h2>
                if documents.is_empty() {
                    <p class="text-sm text-neutral-500">"The invoice is being issued: reload in a moment."</p>
                }
                <ul class="space-y-1 text-sm">
                    for doc in &documents {
                        <li data-document=(&doc.label)>
                            <a class="font-mono underline" href=(crate::app::t::slug::invoices::pdf_href(cx, &slug, &doc.document_id))>(&doc.label)</a>
                            " · "
                            (if doc.kind == DocumentKind::CreditNote { "credit note" } else { "invoice" })
                            " · "
                            (format_amount_fr(doc.total_cents, &doc.currency))
                        </li>
                    }
                </ul>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Items"</h2>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Product"</th>
                            <th class="py-2 font-medium">"Code"</th>
                            <th class="py-2 font-medium">"Unit price"</th>
                            <th class="py-2 font-medium">"Quantity"</th>
                            <th class="py-2 text-right font-medium">"Amount"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        for line in lines {
                            <tr data-order-line=(&line.name)>
                                <td class="py-2">
                                    (&line.name)
                                    if let Some(variant) = &line.variant {
                                        <span class="text-neutral-500">" · " (variant)</span>
                                    }
                                </td>
                                <td class="py-2"><code>(line.sku.as_deref().unwrap_or("—"))</code></td>
                                <td class="py-2">(&line.unit)</td>
                                <td class="py-2">(line.quantity)</td>
                                <td class="py-2 text-right">(&line.amount)</td>
                            </tr>
                        }
                    </tbody>
                    <tfoot>
                        if let Some((code, amount)) = &discount {
                            <tr class="border-t border-neutral-200" data-order-discount=(code)>
                                <td class="py-2" colspan="4">"Discount " <code>(code)</code></td>
                                <td class="py-2 text-right">(amount)</td>
                            </tr>
                        }
                        if let Some((name, amount)) = &shipping {
                            <tr class="border-t border-neutral-200" data-order-shipping=(name)>
                                <td class="py-2" colspan="4">"Shipping: " (name)</td>
                                <td class="py-2 text-right">(amount)</td>
                            </tr>
                        }
                        <tr class="border-t border-neutral-200 font-medium">
                            <td class="py-2" colspan="4">"Total"</td>
                            <td class="py-2 text-right" data-order-total="">(&total)</td>
                        </tr>
                    </tfoot>
                </table>
            </section>

            <section class="mb-8 grid gap-4 md:grid-cols-2">
                <div class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Ship to"</h2>
                    <address class="text-sm not-italic" data-order-address="">
                        (&address.name) <br>
                        (&address.line1) <br>
                        if let Some(line2) = &address.line2 { (line2) <br> }
                        (&address.postal_code) " " (&address.city) <br>
                        (&address.country)
                        if let Some(phone) = &address.phone { <br> (phone) }
                    </address>
                </div>
                if let Some(billing) = &billing {
                    <div class="rounded border border-neutral-200 bg-white p-4">
                        <h2 class="mb-2 text-lg font-medium">"Bill to"</h2>
                        <address class="text-sm not-italic" data-order-billing="">
                            (&billing.name) <br>
                            (&billing.line1) <br>
                            if let Some(line2) = &billing.line2 { (line2) <br> }
                            (&billing.postal_code) " " (&billing.city) <br>
                            (&billing.country)
                        </address>
                    </div>
                }
                if let Some(note) = &order.note {
                    <div class="rounded border border-neutral-200 bg-white p-4">
                        <h2 class="mb-2 text-lg font-medium">"Note from the customer"</h2>
                        <p class="whitespace-pre-line text-sm" data-order-note="">(note)</p>
                    </div>
                }
            </section>

            if can_fulfil || can_cancel {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Next step"</h2>
                    <div class="space-y-4">
                        if can_fulfil {
                            <form method="post" action=(href!(fulfil::fulfil, Slug(slug.clone()), OrderId(order_id.clone()))) class="space-y-2">
                                <label class="block text-sm" for="order-tracking">"Tracking number (optional)"</label>
                                <input id="order-tracking" name="tracking" type="text" maxlength=(MAX_TRACKING_LEN)
                                    class="w-full rounded border border-neutral-300 px-3 py-2">
                                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Mark as fulfilled"</button>
                            </form>
                        }
                        if can_cancel {
                            <form method="post" action=(href!(cancel::cancel, Slug(slug.clone()), OrderId(order_id.clone()))) class="space-y-2">
                                <label class="block text-sm" for="order-cancel-reason">"Reason for cancelling"</label>
                                <input id="order-cancel-reason" name="reason" type="text" required="" maxlength=(MAX_REASON_LEN)
                                    class="w-full rounded border border-neutral-300 px-3 py-2">
                                <button class="rounded border border-red-300 px-4 py-2 font-medium text-red-800 hover:bg-red-50">"Cancel and refund"</button>
                            </form>
                        }
                    </div>
                </section>
            }
        )
    })
}

#[derive(serde::Deserialize)]
pub struct FulfilForm {
    #[serde(default)]
    pub tracking: String,
}

#[derive(serde::Deserialize)]
pub struct CancelForm {
    pub reason: String,
}
