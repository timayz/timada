//! `/{mount}/returns`: the returns customers asked for. The default view is
//! what waits for an operator: requests to review and parcels to receive.

pub mod return_id;

use std::collections::HashMap;

use timada_core::Money;
use timada_order::order_numbers_by_ids;
use timada_returns::{ListReturns, ReturnListRow, ReturnStatus, count_returns, list_returns};
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
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, money, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct ReturnsQuery {
    page: Option<u32>,
    /// `requested` (the default), another status, or `all`.
    status: Option<String>,
}

pub fn parse_status(status: &str) -> Option<ReturnStatus> {
    match status {
        "requested" => Some(ReturnStatus::Requested),
        "approved" => Some(ReturnStatus::Approved),
        "refused" => Some(ReturnStatus::Refused),
        "cancelled" => Some(ReturnStatus::Cancelled),
        "received" => Some(ReturnStatus::Received),
        "completed" => Some(ReturnStatus::Completed),
        _ => None,
    }
}

#[component]
pub async fn return_status_badge(status: ReturnStatus) -> Result<impl View> {
    let (variant, label) = match status {
        ReturnStatus::Requested => (BadgeVariant::Secondary, "À examiner"),
        ReturnStatus::Approved => (BadgeVariant::Outline, "Colis attendu"),
        ReturnStatus::Refused => (BadgeVariant::Destructive, "Refusé"),
        ReturnStatus::Cancelled => (BadgeVariant::Outline, "Annulé par le client"),
        ReturnStatus::Received => (BadgeVariant::Secondary, "Reçu, en traitement"),
        ReturnStatus::Completed => (BadgeVariant::Primary, "Traité"),
    };
    Ok(view! { badge(variant: variant, (label)) })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<ReturnsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let selected = query
        .status
        .clone()
        .unwrap_or_else(|| "requested".to_owned());
    let status = parse_status(&selected);
    let db = &app_context::<AdminServices>(cx).db;

    let rows = list_returns(
        db,
        &ListReturns {
            status,
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_returns(db, status).await?;
    let order_ids: Vec<String> = rows.iter().map(|r| r.order_id.clone()).collect();
    let order_numbers = order_numbers_by_ids(db, &order_ids).await?;

    Ok(view! {
        page_header(
            title: "Retours",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="status" class="text-muted-foreground">"Statut"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    for (value, label) in [("requested", "À examiner"), ("approved", "Colis attendus"), ("received", "En traitement"), ("completed", "Traités"), ("refused", "Refusés"), ("cancelled", "Annulés"), ("all", "Tous")] {
                        <option value=(value) selected=(selected == value)>(label)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun retour dans cette file.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Retour") table_head("Commande") table_head("Motif")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Articles")
                    table_head("Statut")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Rendu au client")
                ))
                table_body(
                    for row in &rows {
                        return_row(row: row, order_numbers: &order_numbers)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[component]
async fn return_row(
    cx: &Cx,
    row: &ReturnListRow,
    order_numbers: &HashMap<String, String>,
) -> Result<impl View> {
    let link = href!(return_id::show, return_id::ReturnId(row.return_id.clone())).resolve(cx);
    let order_link = href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx);
    let order_label = order_numbers
        .get(&row.order_id)
        .unwrap_or(&row.order_id)
        .clone();
    let status = parse_status(&row.status).unwrap_or_default();
    let given_back = money(&Money::new(
        row.refunded_minor + row.credited_minor,
        &row.currency,
    ));
    Ok(view! {
        table_row(
            table_cell((date(row.requested_at.max(0) as u64)))
            table_cell(<a href=(link) class="font-mono text-xs underline-offset-4 hover:underline">(row.rma_number.clone())</a>)
            table_cell(<a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(order_label)</a>)
            table_cell((row.reason.clone()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (row.units.to_string()))
            table_cell(return_status_badge(status: status))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (given_back))
        )
    })
}
