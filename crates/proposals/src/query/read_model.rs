//! The subscription that keeps `proposals` current for every tenant.
//!
//! The handler reloads the strict write model of the proposal and rewrites
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
use timada_core::db::{Db, Json, Value, params_from_iter};

use crate::aggregator::Proposal;
use crate::command::{ProposalState, proposal_projection};
use crate::value_object::ProposalStatus;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-proposals/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_proposal_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_proposal_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Proposal>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(proposal) = proposal_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_proposal(&db, &event.tenant, &proposal).await
}

/// Writes the row of a proposal as the write model sees it.
pub(crate) async fn upsert_proposal(
    db: &Db,
    tenant_id: &str,
    proposal: &ProposalState,
) -> anyhow::Result<()> {
    let waiting = proposal.status == ProposalStatus::Waiting;
    let listed_at = if waiting {
        proposal.prepared_at
    } else {
        proposal.updated_at
    };
    db.execute(
        "INSERT INTO proposals (tenant_id, id, kind, subject_id, subject_name, source,
            prepared_by, reason, changes, edited, status, waiting, valid_until, prepared_at,
            answered_by, note, listed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            changes = excluded.changes, edited = excluded.edited, status = excluded.status,
            waiting = excluded.waiting, answered_by = excluded.answered_by,
            note = excluded.note, listed_at = excluded.listed_at",
        params_from_iter([
            Value::from(tenant_id),
            Value::from(proposal.id.as_str()),
            Value::from(proposal.kind.as_str()),
            Value::from(proposal.subject_id.as_str()),
            Value::from(proposal.subject_name.as_str()),
            Value::from(proposal.source.as_str()),
            Value::from(proposal.prepared_by.as_str()),
            Value::from(proposal.reason.as_str()),
            Value::from(Json(&proposal.changes).to_text()?),
            Value::from(i64::from(proposal.edited)),
            Value::from(proposal.status.as_str()),
            Value::from(i64::from(waiting)),
            Value::from(proposal.valid_until as i64),
            Value::from(proposal.prepared_at as i64),
            Value::from(proposal.answered_by.as_str()),
            Value::from(proposal.note.as_str()),
            Value::from(listed_at as i64),
        ]),
    )
    .await?;
    Ok(())
}
