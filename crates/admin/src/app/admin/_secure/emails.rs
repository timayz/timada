//! `/{mount}/emails`: the mailer's outbox — what was written to customers,
//! what is waiting, and what the relay keeps refusing.

pub mod message_id;

use timada_mailer::{OutboxRow, OutboxStatus, count_outbox, list_outbox};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, component, view},
};

use crate::{
    components::{
        badge::{BadgeVariant, badge},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct EmailsQuery {
    page: Option<u32>,
    status: Option<String>,
}

fn parse_status(status: Option<&str>) -> Option<OutboxStatus> {
    match status? {
        "pending" => Some(OutboxStatus::Pending),
        "sent" => Some(OutboxStatus::Sent),
        "failed" => Some(OutboxStatus::Failed),
        _ => None,
    }
}

#[component]
pub async fn outbox_status_badge(status: OutboxStatus) -> Result<impl View> {
    let (variant, label) = match status {
        OutboxStatus::Pending => (BadgeVariant::Secondary, "En attente"),
        OutboxStatus::Sent => (BadgeVariant::Success, "Envoyé"),
        OutboxStatus::Failed => (BadgeVariant::Destructive, "Échec"),
    };
    Ok(view! { badge(variant: variant, (label)) })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<EmailsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let status = parse_status(query.status.as_deref());
    let db = &app_context::<AdminServices>(cx).db;
    let rows = list_outbox(db, status, PAGE_SIZE, (page - 1) * PAGE_SIZE)
        .await
        .map_err(anyhow::Error::from)?;
    let total = count_outbox(db, status)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(view! {
        page_header(
            title: "E-mails",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="status" class="text-muted-foreground">"Statut"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    <option value="" selected=(status.is_none())>"Tous"</option>
                    for (value, label) in [("pending", "En attente"), ("sent", "Envoyés"), ("failed", "En échec")] {
                        <option value=(value) selected=(query.status.as_deref() == Some(value))>(label)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun e-mail.")
        } else {
            table(
                table_header(table_row(
                    table_head("Date") table_head("Destinataire") table_head("Objet")
                    table_head("Statut") table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Essais")
                ))
                table_body(
                    for row in &rows {
                        email_row(row: row)
                    }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[component]
async fn email_row(cx: &Cx, row: &OutboxRow) -> Result<impl View> {
    let link = href!(
        message_id::show,
        message_id::MessageId(row.message_id.clone())
    )
    .resolve(cx);
    Ok(view! {
        table_row(
            table_cell((date(row.created_at.max(0) as u64)))
            table_cell((row.recipient.clone()))
            table_cell(<a href=(link) class="underline-offset-4 hover:underline">(row.subject.clone())</a>)
            table_cell(outbox_status_badge(status: row.status()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (row.attempts.to_string()))
        )
    })
}
