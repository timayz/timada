//! `automations_listeners`: which published scenario each trigger starts,
//! in which version, and whether it is held. It is kept by the subscription
//! that starts the runs, from the scenarios' own events, so the order of
//! the store decides: an event that follows a publication starts a run of
//! that version, one that precedes it never does.
//!
//! The host builds that subscription from [`listener_subscription`] and adds
//! a handler per trigger event; each looks the listeners of its trigger up
//! with [`listeners_for`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::{FromRow, SqlitePool};

use crate::aggregator::scenario::{Archived, Paused, Published, Resumed};
use crate::definition::Definition;
use crate::value_object::ListenerState;

/// One row of `automations_listeners`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Listener {
    pub tenant_id: String,
    pub scenario_id: String,
    /// The trigger's kind.
    pub trigger: String,
    /// The version running.
    pub version: i64,
    pub state: String,
    /// Unix seconds since when its trigger starts runs.
    pub since: i64,
}

impl Listener {
    pub fn state(&self) -> ListenerState {
        ListenerState::parse(&self.state).unwrap_or(ListenerState::Paused)
    }
}

/// The base of the subscription that starts runs: it keeps the listeners
/// from the scenarios' events, into `pool` (the read-model database, which
/// must be allowed to write). The host adds `.data(…)` and a handler per
/// trigger event, then `.any_routing_key()` and its retry policy.
pub fn listener_subscription<E: Executor>(
    key: &'static str,
    pool: SqlitePool,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(key)
        .data(pool)
        .handler(on_scenario_published())
        .handler(on_scenario_paused())
        .handler(on_scenario_resumed())
        .handler(on_scenario_archived())
}

#[evento::subscription]
async fn on_scenario_published<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Published>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    // Published only once it reads and has a trigger: anything else starts
    // nothing.
    let Some(trigger) = Definition::from_json(&event.data.definition)
        .ok()
        .and_then(|definition| definition.trigger)
    else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO automations_listeners (tenant_id, scenario_id, trigger, version, state, since)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5)
         ON CONFLICT (tenant_id, scenario_id) DO UPDATE SET
            trigger = excluded.trigger, version = excluded.version,
            state = excluded.state, since = excluded.since",
    )
    .bind(&event.tenant)
    .bind(&event.aggregate_id)
    .bind(&trigger.kind)
    .bind(i64::from(event.data.version))
    .bind(event.timestamp as i64)
    .execute(&pool)
    .await?;
    Ok(())
}

async fn set_state(
    pool: &SqlitePool,
    tenant_id: &str,
    scenario_id: &str,
    state: ListenerState,
    since: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE automations_listeners SET state = ?3, since = ?4
         WHERE tenant_id = ?1 AND scenario_id = ?2",
    )
    .bind(tenant_id)
    .bind(scenario_id)
    .bind(state.as_str())
    .bind(since as i64)
    .execute(pool)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_scenario_paused<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Paused>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    set_state(
        &pool,
        &event.tenant,
        &event.aggregate_id,
        ListenerState::Paused,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_scenario_resumed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Resumed>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    set_state(
        &pool,
        &event.tenant,
        &event.aggregate_id,
        ListenerState::Active,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_scenario_archived<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Archived>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    set_state(
        &pool,
        &event.tenant,
        &event.aggregate_id,
        ListenerState::Archived,
        event.timestamp,
    )
    .await
}

/// The scenarios of a tenant whose trigger is `trigger` and that run.
pub async fn listeners_for(
    pool: &SqlitePool,
    tenant_id: &str,
    trigger: &str,
) -> anyhow::Result<Vec<Listener>> {
    Ok(sqlx::query_as(
        "SELECT * FROM automations_listeners
         WHERE tenant_id = ?1 AND trigger = ?2 AND state = 'active'
         ORDER BY scenario_id",
    )
    .bind(tenant_id)
    .bind(trigger)
    .fetch_all(pool)
    .await?)
}

/// Where a scenario stands for its runs; `None` until the subscription
/// saw it published.
pub async fn listener_of(
    pool: &SqlitePool,
    tenant_id: &str,
    scenario_id: &str,
) -> anyhow::Result<Option<Listener>> {
    Ok(sqlx::query_as(
        "SELECT * FROM automations_listeners WHERE tenant_id = ?1 AND scenario_id = ?2",
    )
    .bind(tenant_id)
    .bind(scenario_id)
    .fetch_optional(pool)
    .await?)
}
