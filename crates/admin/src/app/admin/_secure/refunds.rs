//! `/{mount}/refunds`: every refund made, newest first. Refunds are issued
//! from the order page.

use timada_core::Money;
use timada_payment::{RefundListRow, count_refunds, list_refunds};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, component, view},
};

use super::orders::order_id;
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

    Ok(view! {
        page_header(title: "Remboursements")
        if rows.is_empty() {
            empty_state(message: "Aucun remboursement. Un remboursement se fait depuis la page d'une commande payée.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Commande") table_head("Motif")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Montant")
                ))
                table_body(
                    for row in &rows {
                        refund_row(row: row)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[component]
async fn refund_row(cx: &Cx, row: &RefundListRow) -> Result<impl View> {
    let order_link = href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx);
    let amount = money(&Money::new(row.amount_minor, &row.currency));
    Ok(view! {
        table_row(
            table_cell((date(row.refunded_at as u64)))
            table_cell(<a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(row.order_id.clone())</a>)
            table_cell((row.reason.clone()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (amount))
        )
    })
}
