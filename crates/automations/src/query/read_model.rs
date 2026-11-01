//! The subscription that keeps `automations_scenarios` and
//! `automations_runs` current for every tenant.
//!
//! Each handler reloads the strict write model of the stream and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting. One subscription follows every tenant's streams
//! (its cursor lives in the store): each row carries the tenant of the event
//! that made it, and it writes into the read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::Db;

use crate::aggregator::run::Run as RunStream;
use crate::aggregator::scenario::Scenario as ScenarioStream;
use crate::command::{RunState, ScenarioState, run_projection, scenario_projection};
use crate::definition::Definition;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-automations/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_scenario_event())
        .handler(on_run_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_scenario_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ScenarioStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = scenario_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_scenario(&db, &event.tenant, &state).await
}

#[evento::subscription_all]
async fn on_run_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<RunStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = run_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_run(&db, &event.tenant, &state).await
}

/// Writes the row of a run as the write model sees it: `next_at` is when
/// the runner looks at it again, none once it ended.
pub async fn upsert_run(db: &Db, tenant_id: &str, run: &RunState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO automations_runs (tenant_id, id, scenario_id, scenario_version, trigger,
            subject_id, status, step, reason, next_at, started_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, step = excluded.step, reason = excluded.reason,
            next_at = excluded.next_at, updated_at = excluded.updated_at",
        (
            tenant_id,
            run.id.as_str(),
            run.scenario_id.as_str(),
            i64::from(run.scenario_version),
            run.trigger.as_str(),
            run.subject_id.as_str(),
            run.status.as_str(),
            run.log.last().map(|record| record.step.as_str()),
            run.reason.as_deref(),
            run.next_at().map(|at| at as i64),
            run.started_at as i64,
            run.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// Writes the row of a scenario as the write model sees it. The trigger and
/// the count of steps are the running version's once published, the
/// draft's before.
pub async fn upsert_scenario(
    db: &Db,
    tenant_id: &str,
    scenario: &ScenarioState,
) -> anyhow::Result<()> {
    let shown = if scenario.version > 0 {
        &scenario.live
    } else {
        &scenario.draft
    };
    // A definition this build cannot read lists without its trigger.
    let shown = Definition::from_json(shown).unwrap_or_default();
    db.execute(
        "INSERT INTO automations_scenarios (tenant_id, id, name, status, trigger, version,
            unpublished, steps, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            name = excluded.name, status = excluded.status, trigger = excluded.trigger,
            version = excluded.version, unpublished = excluded.unpublished,
            steps = excluded.steps, updated_at = excluded.updated_at",
        (
            tenant_id,
            scenario.id.as_str(),
            scenario.name.as_str(),
            scenario.status.as_str(),
            shown.trigger.as_ref().map(|trigger| trigger.kind.as_str()),
            i64::from(scenario.version),
            scenario.has_unpublished_changes(),
            shown.count() as i64,
            scenario.created_at as i64,
            scenario.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}
