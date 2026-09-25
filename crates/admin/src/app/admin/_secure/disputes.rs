//! `/{mount}/disputes`: payments a cardholder contested with their bank. The
//! open ones first, the nearest deadline on top — the evidence itself goes to
//! the payment provider, from its own dashboard. What a dispute means for an
//! order (held, then released or charged back) is on the order's page.

use timada_core::Money;
use timada_order::order_numbers_by_ids;
use timada_payment::{
    DisputeRow, DisputeStatus, count_disputes, dispute_reason_label, list_disputes,
};
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
struct DisputesQuery {
    page: Option<u32>,
    /// `open` (the default), `won`, `lost` or `all`.
    status: Option<String>,
}

/// One dispute of the list, worded.
struct DisputeLine {
    order_link: String,
    order_name: String,
    reference: String,
    opened_on: String,
    reason: String,
    amount: String,
    /// When the evidence is due, and whether that is past — for open
    /// disputes only.
    deadline: Option<(String, bool)>,
    status: DisputeStatus,
}

fn parse_status(status: &str) -> Option<DisputeStatus> {
    match status {
        "open" => Some(DisputeStatus::Open),
        "won" => Some(DisputeStatus::Won),
        "lost" => Some(DisputeStatus::Lost),
        _ => None,
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let asked = query::<DisputesQuery>(cx)?;
    let page = asked.page.unwrap_or(1).max(1);
    let chosen = asked.status.clone().unwrap_or_else(|| "open".to_owned());
    let status = parse_status(&chosen);
    let db = &app_context::<AdminServices>(cx).db;
    let now = timada_core::time::now_unix_secs()? as i64;

    let rows: Vec<DisputeRow> =
        list_disputes(db, status, PAGE_SIZE, (page - 1) * PAGE_SIZE).await?;
    let total = count_disputes(db, status).await?;
    let open = count_disputes(db, Some(DisputeStatus::Open)).await?;
    let order_ids: Vec<String> = rows.iter().map(|row| row.order_id.clone()).collect();
    let order_numbers = order_numbers_by_ids(db, &order_ids).await?;

    let lines: Vec<DisputeLine> = rows
        .into_iter()
        .map(|row| {
            let standing = parse_status(&row.status).unwrap_or_default();
            DisputeLine {
                order_link: href!(order_id::show, order_id::OrderId(row.order_id.clone()))
                    .resolve(cx),
                order_name: order_numbers
                    .get(&row.order_id)
                    .cloned()
                    .unwrap_or(row.order_id),
                reference: row.dispute_id,
                opened_on: date(row.opened_at.max(0) as u64),
                reason: dispute_reason_label(&row.reason).to_owned(),
                amount: money(&Money::new(row.amount_minor, &row.currency)),
                deadline: row
                    .respond_by
                    .filter(|_| standing == DisputeStatus::Open)
                    .map(|respond_by| (date(respond_by.max(0) as u64), respond_by < now)),
                status: standing,
            }
        })
        .collect();
    let summary = match open {
        0 => "Aucun litige n'attend de réponse.".to_owned(),
        1 => "1 litige en cours : sa commande est retenue, aucun remboursement ne part sur son paiement.".to_owned(),
        open => format!("{open} litiges en cours : leurs commandes sont retenues, aucun remboursement ne part sur leurs paiements."),
    };
    let empty = match status {
        Some(DisputeStatus::Open) => "Aucun litige en cours.",
        _ => "Aucun litige.",
    };

    Ok(view! {
        page_header(
            title: "Litiges",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="status" class="text-muted-foreground">"État"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    for (value, wording) in [("open", "En cours"), ("won", "Gagnés"), ("lost", "Perdus"), ("all", "Tous")] {
                        <option value=(value) selected=(chosen == value)>(wording)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        <p class="mb-4 text-sm text-muted-foreground">(summary) " Les justificatifs se transmettent depuis l'espace du prestataire de paiement."</p>
        if lines.is_empty() {
            empty_state(message: empty)
        } else {
            table(
                table_header(table_row(
                    table_head("Ouvert le") table_head("Commande") table_head("Référence") table_head("Motif")
                    table_head("Réponse avant le") table_head("État")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Montant")
                ))
                table_body(
                    for line in &lines {
                        dispute_row(line: line)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[component]
pub async fn dispute_status_badge(status: DisputeStatus) -> Result<impl View> {
    let (variant, wording) = match status {
        DisputeStatus::Open => (BadgeVariant::Destructive, "En cours"),
        DisputeStatus::Won => (BadgeVariant::Success, "Gagné"),
        DisputeStatus::Lost => (BadgeVariant::Outline, "Perdu"),
    };
    Ok(view! { badge(variant: variant, (wording)) })
}

#[component]
async fn dispute_row(line: &DisputeLine) -> Result<impl View> {
    Ok(view! {
        table_row(
            table_cell((line.opened_on.clone()))
            table_cell(<a href=(line.order_link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(line.order_name.clone())</a>)
            table_cell(<span class="font-mono text-xs">(line.reference.clone())</span>)
            table_cell((line.reason.clone()))
            table_cell(
                match &line.deadline {
                    Some((day, true)) => { <span class="text-destructive">(day.clone()) " — dépassé"</span> }
                    Some((day, false)) => { (day.clone()) }
                    None => { <span class="text-muted-foreground">"—"</span> }
                }
            )
            table_cell(dispute_status_badge(status: line.status))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.amount.clone()))
        )
    })
}
