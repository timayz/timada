//! `/{mount}/invoices`: every invoice, newest first, filterable by status and
//! by the start of its legal number.

pub mod invoice_id;

use std::collections::HashMap;

use timada_core::Money;
use timada_invoice::{InvoiceListRow, InvoiceStatus, ListInvoices, count_invoices, list_invoices};
use timada_order::order_numbers_by_ids;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, component, view},
};

use super::orders::order_id;
use crate::{
    components::{
        badge::{BadgeVariant, badge},
        input::input,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, money, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct InvoicesQuery {
    page: Option<u32>,
    status: Option<String>,
    number: Option<String>,
}

fn parse_status(status: Option<&str>) -> Option<InvoiceStatus> {
    match status? {
        "draft" => Some(InvoiceStatus::Draft),
        "issued" => Some(InvoiceStatus::Issued),
        "voided" => Some(InvoiceStatus::Voided),
        _ => None,
    }
}

#[component]
pub async fn invoice_status_badge(status: InvoiceStatus) -> Result<impl View> {
    let (variant, label) = match status {
        InvoiceStatus::Draft => (BadgeVariant::Secondary, "Brouillon"),
        InvoiceStatus::Issued => (BadgeVariant::Success, "Émise"),
        InvoiceStatus::Voided => (BadgeVariant::Destructive, "Annulée"),
    };
    Ok(view! { badge(variant: variant, (label)) })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<InvoicesQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let status = parse_status(query.status.as_deref());
    let db = &app_context::<AdminServices>(cx).db;

    let filter = ListInvoices {
        status,
        number: query.number.clone(),
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
    };
    let rows = list_invoices(db, &filter).await?;
    let total = count_invoices(db, &filter).await?;
    let order_ids: Vec<String> = rows.iter().map(|r| r.order_id.clone()).collect();
    let order_numbers = order_numbers_by_ids(db, &order_ids).await?;

    Ok(view! {
        page_header(
            title: "Factures",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="number" class="text-muted-foreground">"Numéro"</label>
                input(attrs: topcoat::view::attributes! { id="number" name="number" class="w-40" placeholder="F2026-" autocomplete="off" value=(query.number.clone().unwrap_or_default()) })
                <label for="status" class="text-muted-foreground">"Statut"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    <option value="" selected=(status.is_none())>"Tous"</option>
                    for (value, label) in [("draft", "Brouillon"), ("issued", "Émise"), ("voided", "Annulée")] {
                        <option value=(value) selected=(query.status.as_deref() == Some(value))>(label)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        if rows.is_empty() {
            empty_state(message: "Aucune facture.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Numéro") table_head("Commande") table_head("Client")
                    table_head("Statut") table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Total")
                ))
                table_body(
                    for row in &rows {
                        invoice_row(row: row, order_numbers: &order_numbers)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[component]
async fn invoice_row(
    cx: &Cx,
    row: &InvoiceListRow,
    order_numbers: &HashMap<String, String>,
) -> Result<impl View> {
    let order_label = order_numbers
        .get(&row.order_id)
        .unwrap_or(&row.order_id)
        .clone();
    let link = href!(
        invoice_id::show,
        invoice_id::InvoiceId(row.invoice_id.clone())
    )
    .resolve(cx);
    let order_link = href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx);
    let total = money(&Money::new(row.total_minor, &row.currency));
    let status = parse_status(Some(&row.status)).unwrap_or_default();
    let number = row
        .invoice_number
        .clone()
        .unwrap_or_else(|| "non numérotée".to_owned());
    Ok(view! {
        table_row(
            table_cell((date(row.issued_at.unwrap_or(row.drafted_at) as u64)))
            table_cell(<a href=(link) class="font-mono text-xs underline-offset-4 hover:underline">(number)</a>)
            table_cell(<a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(order_label)</a>)
            table_cell(<span class="font-mono text-xs">(row.customer_id.clone())</span>)
            table_cell(invoice_status_badge(status: status))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (total))
        )
    })
}
