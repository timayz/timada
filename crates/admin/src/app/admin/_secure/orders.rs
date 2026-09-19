//! `/{mount}/orders`: every order, newest first, filterable by status and by
//! the start of its number.

pub mod order_id;

use timada_core::Money;
use timada_order::{ListOrders, OrderHistoryRow, OrderStatus, count_orders, list_orders};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    components::{
        input::input,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, money, order_status_badge, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct OrdersQuery {
    page: Option<u32>,
    status: Option<String>,
    number: Option<String>,
}

fn parse_status(status: Option<&str>) -> Option<OrderStatus> {
    match status? {
        "placed" => Some(OrderStatus::Placed),
        "paid" => Some(OrderStatus::Paid),
        "shipped" => Some(OrderStatus::Shipped),
        "cancelled" => Some(OrderStatus::Cancelled),
        _ => None,
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<OrdersQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let status = parse_status(query.status.as_deref());
    let db = &app_context::<AdminServices>(cx).db;

    let filter = ListOrders {
        status,
        number: query.number.clone(),
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
    };
    let rows = list_orders(db, &filter).await?;
    let total = count_orders(db, &filter).await?;

    Ok(view! {
        page_header(
            title: "Commandes",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="number" class="text-muted-foreground">"Numéro"</label>
                input(attrs: topcoat::view::attributes! { id="number" name="number" class="w-40" placeholder="C2026-" autocomplete="off" value=(query.number.clone().unwrap_or_default()) })
                <label for="status" class="text-muted-foreground">"Statut"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    <option value="" selected=(status.is_none())>"Tous"</option>
                    for (value, label) in [("placed", "En attente"), ("paid", "Payée"), ("shipped", "Expédiée"), ("cancelled", "Annulée")] {
                        <option value=(value) selected=(query.status.as_deref() == Some(value))>(label)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        if rows.is_empty() {
            empty_state(message: "Aucune commande.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Commande") table_head("Client") table_head("Vendeur")
                    table_head("Statut") table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Total")
                ))
                table_body(
                    for row in &rows {
                        order_row(row: row)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[topcoat::view::component]
async fn order_row(cx: &Cx, row: &OrderHistoryRow) -> Result<impl View> {
    let link = href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx);
    let total = money(&Money::new(row.total_minor, &row.currency));
    let status = parse_status(Some(&row.status)).unwrap_or_default();
    Ok(view! {
        table_row(
            table_cell((date(row.placed_at as u64)))
            table_cell(<a href=(link) class="font-mono text-xs underline-offset-4 hover:underline">(row.order_number.clone().unwrap_or_else(|| row.order_id.clone()))</a>)
            table_cell(<span class="font-mono text-xs">(row.customer_id.clone())</span>)
            table_cell((row.seller.clone()))
            table_cell(order_status_badge(status: status))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (total))
        )
    })
}
