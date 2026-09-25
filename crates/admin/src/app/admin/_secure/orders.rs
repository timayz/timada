//! `/{mount}/orders`: every order, newest first, filterable by status and by
//! the start of its number — and `/{mount}/orders/to-ship`, the queue of paid
//! orders waiting for their parcel, the one waiting longest first.

pub mod order_id;

use timada_core::Money;
use timada_order::{
    ListOrders, OrderHistoryRow, OrderStatus, OrderToShipRow, count_orders, count_orders_to_ship,
    list_orders, load_order_details, orders_to_ship,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonVariant, button_variants},
        input::input,
        select::select,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::{AdminConfig, AdminServices},
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
    let (waiting, late) = count_orders_to_ship(db, late_before(cx)?).await?;
    let to_ship_label = match (waiting, late) {
        (0, _) => "À expédier".to_owned(),
        (waiting, 0) => format!("À expédier ({waiting})"),
        (waiting, late) => format!("À expédier ({waiting}, dont {late} en retard)"),
    };
    let to_ship_variant = if late > 0 {
        ButtonVariant::Destructive
    } else {
        ButtonVariant::Outline
    };

    Ok(view! {
        page_header(
            title: "Commandes",
            <a href=(href!(to_ship)) class=(button_variants(to_ship_variant, Default::default()))>(to_ship_label)</a>
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="number" class="text-muted-foreground">"Numéro"</label>
                input(attrs: topcoat::view::attributes! { id="number" name="number" class="w-40" placeholder="C2026-" autocomplete="off" value=(query.number.clone().unwrap_or_default()) })
                <label for="status" class="text-muted-foreground">"Statut"</label>
                select(
                    attrs: topcoat::view::attributes! { id="status" name="status" },
                    <option value="" selected=(status.is_none())>"Tous"</option>
                    for (value, label) in [("placed", "En attente"), ("paid", "Payée"), ("shipped", "Expédiée"), ("cancelled", "Annulée")] {
                        <option value=(value) selected=(query.status.as_deref() == Some(value))>(label)</option>
                    }
                )
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

/// Orders that started waiting before this moment are late.
fn late_before(cx: &Cx) -> Result<u64> {
    let within = app_context::<AdminConfig>(cx).ship_within.as_secs();
    Ok(timada_core::time::now_unix_secs()?.saturating_sub(within))
}

#[query_params(error = bad_request)]
struct ToShipQuery {
    page: Option<u32>,
}

/// An order of the queue, worded.
struct ToShipLine {
    link: String,
    label: String,
    paid_on: String,
    waiting: String,
    late: bool,
    recipient: String,
    delivery: String,
    units: String,
    total: String,
}

/// "3 j", "5 h", "12 min": how long an order has been waiting.
fn waiting_label(seconds: u64) -> String {
    match seconds {
        s if s >= 86_400 => format!("{} j", s / 86_400),
        s if s >= 3_600 => format!("{} h", s / 3_600),
        s => format!("{} min", s / 60),
    }
}

/// The queue of paid orders waiting for their parcel — the fulfillment saga
/// never times these out: somebody ships them.
#[page("./to-ship")]
pub async fn to_ship(cx: &Cx) -> Result<impl View> {
    let page = query::<ToShipQuery>(cx)?.page.unwrap_or(1).max(1);
    let services = app_context::<AdminServices>(cx);
    let now = timada_core::time::now_unix_secs()?;
    let late_before = late_before(cx)?;
    let (waiting, late) = count_orders_to_ship(&services.db, late_before).await?;
    let rows: Vec<OrderToShipRow> =
        orders_to_ship(&services.db, PAGE_SIZE, (page - 1) * PAGE_SIZE).await?;

    // Where each parcel goes comes from the order itself: a page of them.
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let order = load_order_details(&services.executor, &row.order_id).await?;
        let (recipient, delivery, units) = match &order {
            Some(order) => (
                order.delivery_address.full_name(),
                format!(
                    "{} — {} {} ({})",
                    order.delivery.method_code,
                    order.delivery_address.postal_code,
                    order.delivery_address.city,
                    order.delivery_address.country_code
                ),
                order
                    .lines
                    .iter()
                    .map(|line| line.quantity)
                    .sum::<u32>()
                    .to_string(),
            ),
            None => (row.customer_id.clone(), "—".to_owned(), "—".to_owned()),
        };
        let since = row.waiting_since.max(0) as u64;
        lines.push(ToShipLine {
            link: href!(order_id::show, order_id::OrderId(row.order_id.clone())).resolve(cx),
            label: row.order_number.clone().unwrap_or(row.order_id),
            paid_on: date(since),
            waiting: waiting_label(now.saturating_sub(since)),
            late: since < late_before,
            recipient,
            delivery,
            units,
            total: money(&Money::new(row.total_minor, &row.currency)),
        });
    }
    // Paid, but their payment is disputed: they wait for the bank, not for
    // a parcel.
    let held = match timada_order::count_orders_on_hold(&services.db).await? {
        0 => None,
        1 => Some("1 commande payée est retenue : son paiement est contesté.".to_owned()),
        held => Some(format!(
            "{held} commandes payées sont retenues : leur paiement est contesté."
        )),
    };
    let summary = match (waiting, late) {
        (0, _) => "Aucune commande n'attend son colis.".to_owned(),
        (1, 0) => "1 commande attend son colis.".to_owned(),
        (waiting, 0) => format!("{waiting} commandes attendent leur colis."),
        (waiting, late) => {
            format!("{waiting} commande(s) attendent leur colis, dont {late} en retard.")
        }
    };

    Ok(view! {
        page_header(
            title: "Commandes à expédier",
            <a href=(href!(index)) class=(button_variants(ButtonVariant::Outline, Default::default()))>"Toutes les commandes"</a>
        )
        <p role="status" class="-mt-4 mb-6 text-sm text-muted-foreground">(summary) " La plus ancienne d'abord."</p>
        if let Some(held) = &held {
            <p class="-mt-4 mb-6 text-sm">(held.clone()) " " <a href=(href!(super::disputes::index)) class="underline underline-offset-4">"Voir les litiges"</a></p>
        }
        if !lines.is_empty() {
            table(
                table_header(table_row(
                    table_head("Payée le") table_head("Attente") table_head("Commande") table_head("Destinataire")
                    table_head("Livraison")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Articles")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Total")
                ))
                table_body(
                    for line in &lines {
                        table_row(
                            table_cell((line.paid_on.clone()))
                            table_cell(
                                <span class="tabular-nums">(line.waiting.clone())</span>
                                if line.late { " " badge(variant: BadgeVariant::Destructive, "En retard") }
                            )
                            table_cell(<a href=(line.link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(line.label.clone())</a>)
                            table_cell((line.recipient.clone()))
                            table_cell((line.delivery.clone()))
                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.units.clone()))
                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.total.clone()))
                        )
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: waiting as u64)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::waiting_label;

    #[test]
    fn a_wait_is_told_in_its_largest_unit() {
        assert_eq!(waiting_label(59), "0 min");
        assert_eq!(waiting_label(3_599), "59 min");
        assert_eq!(waiting_label(3_600), "1 h");
        assert_eq!(waiting_label(86_399), "23 h");
        assert_eq!(waiting_label(3 * 86_400 + 5), "3 j");
    }
}
