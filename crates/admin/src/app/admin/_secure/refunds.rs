//! `/{mount}/refunds`: every refund made, newest first, with the credit note
//! documenting it. Refunds are issued from the order page, or by the
//! fulfillment saga when a paid order is cancelled.

use std::collections::HashMap;

use timada_core::Money;
use timada_invoice::credit_notes_of_refunds;
use timada_order::order_numbers_by_ids;
use timada_payment::{RefundListRow, count_refunds, list_refunds};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, component, view},
};

use super::{invoices::invoice_id, orders::order_id};
use crate::{
    components::table::{table, table_body, table_cell, table_head, table_header, table_row},
    config::AdminServices,
    ui::{date, empty_state, money, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct RefundsQuery {
    page: Option<u32>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = query::<RefundsQuery>(cx)?.page.unwrap_or(1).max(1);
    let db = &app_context::<AdminServices>(cx).db;
    let rows = list_refunds(db, PAGE_SIZE, (page - 1) * PAGE_SIZE).await?;
    let total = count_refunds(db).await?;

    // Credit notes trail the refunds by one subscription: a refund may not
    // have its note yet.
    let refund_ids: Vec<String> = rows.iter().map(|r| r.refund_id.clone()).collect();
    let notes: HashMap<String, (String, String)> = credit_notes_of_refunds(db, &refund_ids)
        .await?
        .into_iter()
        .map(|n| (n.refund_id, (n.credit_note_number, n.invoice_id)))
        .collect();
    let order_ids: Vec<String> = rows.iter().map(|r| r.order_id.clone()).collect();
    let order_numbers = order_numbers_by_ids(db, &order_ids).await?;
    let lines: Vec<RefundLine> = rows
        .into_iter()
        .map(|row| RefundLine {
            note: notes.get(&row.refund_id).cloned(),
            order_label: order_numbers
                .get(&row.order_id)
                .unwrap_or(&row.order_id)
                .clone(),
            row,
        })
        .collect();

    Ok(view! {
        page_header(title: "Remboursements")
        if lines.is_empty() {
            empty_state(message: "Aucun remboursement. Un remboursement se fait depuis la page d'une commande payée.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Commande") table_head("Avoir") table_head("Motif")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Montant")
                ))
                table_body(
                    for line in &lines {
                        refund_row(line: line)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

/// One refund of the journal, ready to render.
struct RefundLine {
    row: RefundListRow,
    /// The order's number, or its id when it has none.
    order_label: String,
    /// `(credit note number, invoice id)`, once the note exists.
    note: Option<(String, String)>,
}

#[component]
async fn refund_row(cx: &Cx, line: &RefundLine) -> Result<impl View> {
    let RefundLine {
        row,
        order_label,
        note,
    } = line;
    let note_link = note.clone().map(|(number, invoice)| {
        let link = href!(invoice_id::show, invoice_id::InvoiceId(invoice)).resolve(cx);
        (number, link)
    });
    let order_link = href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx);
    let amount = money(&Money::new(row.amount_minor, &row.currency));
    Ok(view! {
        table_row(
            table_cell((date(row.refunded_at as u64)))
            table_cell(<a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(order_label.clone())</a>)
            table_cell(
                match &note_link {
                    Some((number, link)) => { <a href=(link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(number.clone())</a> }
                    None => { <span class="text-muted-foreground">"—"</span> }
                }
            )
            table_cell((row.reason.clone()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (amount))
        )
    })
}
