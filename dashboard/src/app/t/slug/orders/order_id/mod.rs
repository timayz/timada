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
    view::{View, attributes, class, view},
};

use super::{order_tone, orders};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::{card, card_content, card_header};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_footer, table_head, table_header, table_row,
};
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};

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
    let heading = format!("Order {}", order.reference);
    let placed = format!("Placed {}", format_time(order.placed_at));

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "orders"),
            page_header(
                title: &heading,
                subtitle: Some(&placed),
                h1_attrs: attributes! { class="font-mono" },
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(super::index, Slug(slug.clone())))>"All orders"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                facts(
                    fact(
                        term: "Status",
                        attrs: attributes! { data-order-status=(status.as_str()) },
                        status_badge(tone: order_tone(status), (status.label()))
                    )
                    fact(term: "Placed", <time datetime=(order.placed_at)>(format_time(order.placed_at))</time>)
                    fact(term: "Payment", attrs: attributes! { data-order-payment="" }, (&payment))
                    if let Some(refunded) = &refunded {
                        fact(term: "Refunded", attrs: attributes! { data-order-refunded="" }, (refunded))
                    }
                    if let Some(at) = order.fulfilled_at {
                        fact(term: "Fulfilled", <time datetime=(at)>(format_time(at))</time>)
                    }
                    if let Some(tracking) = &order.tracking {
                        fact(term: "Tracking", attrs: attributes! { data-order-tracking="" }, (tracking))
                    }
                    if let Some(at) = order.cancelled_at {
                        fact(term: "Cancelled", <time datetime=(at)>(format_time(at))</time>)
                    }
                    if let Some(reason) = &order.cancel_reason {
                        fact(term: "Reason", attrs: attributes! { data-order-cancel-reason="" }, (reason))
                    }
                    fact(term: "E-mail", <a class=(LINK) href=(format!("mailto:{}", order.email))>(&order.email)</a>)
                    fact(
                        term: "Confirmation",
                        attrs: attributes! { data-order-confirmation=(if confirmation.is_some() { "mailed" } else { "none" }) },
                        if let Some(mailed) = &confirmation {
                            "Mailed " <time datetime=(mailed.mailed_at)>(format_time(mailed.mailed_at))</time>
                        } else {
                            "Not mailed"
                        }
                    )
                    fact(
                        term: "Review request",
                        attrs: attributes! { data-order-review-request=(if review_request.is_some() { "mailed" } else { "none" }) },
                        if let Some(mailed) = &review_request {
                            "Mailed " <time datetime=(mailed.mailed_at)>(format_time(mailed.mailed_at))</time>
                        } else {
                            "Not mailed"
                        }
                    )
                )
            )

            section_card(
                title: Some("Invoices"),
                attrs: attributes! { data-order-documents="" },
                if documents.is_empty() {
                    <p class="text-sm text-muted-foreground">"The invoice is being issued: reload in a moment."</p>
                }
                <ul class="space-y-1 text-sm">
                    for doc in &documents {
                        <li data-document=(&doc.label)>
                            <a class=(class!(LINK, "font-mono")) href=(crate::app::t::slug::invoices::pdf_href(cx, &slug, &doc.document_id))>(&doc.label)</a>
                            " · "
                            (if doc.kind == DocumentKind::CreditNote { "credit note" } else { "invoice" })
                            " · "
                            (format_amount_fr(doc.total_cents, &doc.currency))
                        </li>
                    }
                </ul>
            )

            section_card(
                title: Some("Items"),
                table(
                    table_header(
                        table_row(
                            table_head("Product")
                            table_head("Code")
                            table_head(attrs: attributes! { class="text-right" }, "Unit price")
                            table_head(attrs: attributes! { class="text-right" }, "Quantity")
                            table_head(attrs: attributes! { class="text-right" }, "Amount")
                        )
                    )
                    table_body(
                        for line in lines {
                            table_row(
                                attrs: attributes! { data-order-line=(&line.name) },
                                table_cell(
                                    <span class="font-medium">(&line.name)</span>
                                    if let Some(variant) = &line.variant {
                                        <span class="text-muted-foreground">" · " (variant)</span>
                                    }
                                )
                                table_cell(<code>(line.sku.as_deref().unwrap_or("—"))</code>)
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&line.unit))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (line.quantity))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&line.amount))
                            )
                        }
                    )
                    table_footer(
                        if let Some((code, amount)) = &discount {
                            table_row(
                                attrs: attributes! { data-order-discount=(code) },
                                table_cell(attrs: attributes! { colspan="4" }, "Discount " <code>(code)</code>)
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (amount))
                            )
                        }
                        if let Some((name, amount)) = &shipping {
                            table_row(
                                attrs: attributes! { data-order-shipping=(name) },
                                table_cell(attrs: attributes! { colspan="4" }, "Shipping: " (name))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (amount))
                            )
                        }
                        table_row(
                            table_cell(attrs: attributes! { colspan="4" }, "Total")
                            table_cell(attrs: attributes! { class="text-right tabular-nums" data-order-total="" }, (&total))
                        )
                    )
                )
            )

            <section class="mb-6 grid gap-4 md:grid-cols-2">
                card(
                    card_header(<h2 class="text-base font-semibold">"Ship to"</h2>)
                    card_content(
                        <address class="text-sm not-italic" data-order-address="">
                            (&address.name) <br>
                            (&address.line1) <br>
                            if let Some(line2) = &address.line2 { (line2) <br> }
                            (&address.postal_code) " " (&address.city) <br>
                            (&address.country)
                            if let Some(phone) = &address.phone { <br> (phone) }
                        </address>
                    )
                )
                if let Some(billing) = &billing {
                    card(
                        card_header(<h2 class="text-base font-semibold">"Bill to"</h2>)
                        card_content(
                            <address class="text-sm not-italic" data-order-billing="">
                                (&billing.name) <br>
                                (&billing.line1) <br>
                                if let Some(line2) = &billing.line2 { (line2) <br> }
                                (&billing.postal_code) " " (&billing.city) <br>
                                (&billing.country)
                            </address>
                        )
                    )
                }
                if let Some(language) = &order.language {
                    card(
                        card_header(<h2 class="text-base font-semibold">"Language"</h2>)
                        card_content(
                            <p class="text-sm" data-order-language=(language)>(timada_tenancy::language_name(language).unwrap_or(language))</p>
                        )
                    )
                }
                if let Some(note) = &order.note {
                    card(
                        card_header(<h2 class="text-base font-semibold">"Note from the customer"</h2>)
                        card_content(
                            <p class="whitespace-pre-line text-sm" data-order-note="">(note)</p>
                        )
                    )
                }
            </section>

            if can_fulfil || can_cancel {
                section_card(
                    title: Some("Next step"),
                    <div class="grid gap-6 md:grid-cols-2">
                        if can_fulfil {
                            <form method="post" action=(href!(fulfil::fulfil, Slug(slug.clone()), OrderId(order_id.clone()))) class="flex flex-col gap-4">
                                field(
                                    field_label(attrs: attributes! { for="order-tracking" }, "Tracking number (optional)")
                                    input(attrs: attributes! { id="order-tracking" name="tracking" type="text" maxlength=(MAX_TRACKING_LEN) })
                                )
                                <div>
                                    button(attrs: attributes! { type="submit" }, "Mark as fulfilled")
                                </div>
                            </form>
                        }
                        if can_cancel {
                            <form method="post" action=(href!(cancel::cancel, Slug(slug.clone()), OrderId(order_id.clone()))) class="flex flex-col gap-4">
                                field(
                                    field_label(attrs: attributes! { for="order-cancel-reason" }, "Reason for cancelling")
                                    input(attrs: attributes! { id="order-cancel-reason" name="reason" type="text" required="" maxlength=(MAX_REASON_LEN) })
                                )
                                <div>
                                    button(variant: ButtonVariant::Destructive, attrs: attributes! { type="submit" }, "Cancel and refund")
                                </div>
                            </form>
                        }
                    </div>
                )
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
