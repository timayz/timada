//! The subscription that keeps `writer_drafts` current for every tenant.
//!
//! The handler reloads the strict write model of the draft and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting. One subscription follows every tenant's
//! streams (its cursor lives in the store): each row carries the tenant of
//! the event that made it, and the rows live in the store's own database
//! ([`Db`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, Value, params_from_iter};

use crate::aggregator::Draft;
use crate::command::{DraftState, draft_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-writer/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_draft_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_draft_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Draft>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(draft) = draft_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_draft(&db, &event.tenant, &draft).await
}

/// Writes the row of a draft as the write model sees it.
pub(crate) async fn upsert_draft(
    db: &Db,
    tenant_id: &str,
    draft: &DraftState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO writer_drafts (tenant_id, id, kind, subject_id, subject_name, keyword,
            requested_by, status, proposal_id, cost_usd_micros, to_check, reason, requested_at,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, proposal_id = excluded.proposal_id,
            cost_usd_micros = excluded.cost_usd_micros, to_check = excluded.to_check,
            reason = excluded.reason, updated_at = excluded.updated_at",
        params_from_iter([
            Value::from(tenant_id),
            Value::from(draft.id.as_str()),
            Value::from(draft.kind.as_str()),
            Value::from(draft.subject_id.as_str()),
            Value::from(draft.subject_name.as_str()),
            Value::from(draft.keyword.as_str()),
            Value::from(draft.requested_by.as_str()),
            Value::from(draft.status.as_str()),
            Value::from(draft.proposal_id.as_str()),
            Value::from(draft.cost_usd_micros as i64),
            Value::from(draft.to_check.join("\n")),
            Value::from(draft.reason.as_str()),
            Value::from(draft.requested_at as i64),
            Value::from(draft.updated_at as i64),
        ]),
    )
    .await?;
    Ok(())
}
