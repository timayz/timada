//! The subscription that keeps `automations_scenarios`,
//! `automations_versions`, `automations_runs` and `automations_run_counts`
//! current for every tenant.
//!
//! Each handler reloads the strict write model of the stream and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting; a version published is kept from its own
//! event, once. A run's row moves its scenario's counts of runs by status
//! when its status changes, by what the row held before, so a repeated
//! delivery moves nothing. A run's row also says whether its scenario is
//! paused, as the scenario's row does: a scenario paused or going on again
//! marks its runs under way. One subscription follows every tenant's
//! streams (its cursor lives in the store): each row carries the tenant of
//! the event that made it, and it writes into the read-model database it is
//! given. The runner writes the runs' rows too, on its own node: it puts a
//! run's `next_at` off when it leaves the run as it was, until the run's
//! next event writes the row again, and it drops the rows of runs that
//! ended long ago, with their counts.

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, execute, fetch_optional};

use crate::aggregator::run::Run as RunStream;
use crate::aggregator::scenario::{Published, Scenario as ScenarioStream};
use crate::command::{RunState, ScenarioState, run_projection, scenario_projection};
use crate::definition::Definition;
use crate::value_object::ScenarioStatus;

use super::runs::count_listed;
use super::scenarios::SCENARIO_STATUS;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-automations/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_scenario_published())
        .handler(on_scenario_event())
        .handler(on_run_event())
        .any_routing_key()
}

/// A version published: kept as published, for the runs pinned to it and
/// their pages, then the scenario's row as for any other event.
#[evento::subscription]
async fn on_scenario_published<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Published>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    db.execute(
        "INSERT INTO automations_versions (tenant_id, scenario_id, version, definition,
            published_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (tenant_id, scenario_id, version) DO NOTHING",
        (
            event.tenant.as_str(),
            event.aggregate_id.as_str(),
            i64::from(event.data.version),
            event.data.definition.as_str(),
            event.timestamp as i64,
        ),
    )
    .await?;
    refresh_scenario(context, &db, &event.tenant, &event.aggregate_id).await
}

#[evento::subscription_all]
async fn on_scenario_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ScenarioStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    refresh_scenario(context, &db, &event.tenant, &event.aggregate_id).await
}

/// Rewrites a scenario's row from its write model.
async fn refresh_scenario<E: Executor>(
    context: &Context<'_, E>,
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
) -> anyhow::Result<()> {
    let Some(state) = scenario_projection()
        .load(scenario_id)
        .tenant(tenant_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_scenario(db, tenant_id, &state).await
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

/// What [`upsert_run`] writes. Whether the run's scenario is paused is
/// read off the scenario's row, by the table's key (which the planner does
/// not pick by itself, as the listing's index leads with the tenant too).
pub(super) const UPSERT_RUN: &str = "INSERT INTO automations_runs (tenant_id, id, scenario_id,
        scenario_version, trigger, subject_id, status, step, reason, next_at, paused, shard,
        started_at, updated_at)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
        EXISTS (SELECT 1 FROM automations_scenarios
                    INDEXED BY sqlite_autoindex_automations_scenarios_1
                WHERE tenant_id = ?1 AND id = ?3 AND status = 'paused'),
        ?11, ?12, ?13)
     ON CONFLICT (tenant_id, id) DO UPDATE SET
        status = excluded.status, step = excluded.step, reason = excluded.reason,
        next_at = excluded.next_at, paused = excluded.paused, updated_at = excluded.updated_at";

/// What [`upsert_run`] reads of a run's row before writing it: the status
/// it is counted under. By the table's key, which the planner does not
/// pick by itself (the indexes of a scenario's runs lead with the tenant
/// too).
pub(super) const RUN_STATUS: &str = "SELECT status FROM automations_runs
        INDEXED BY sqlite_autoindex_automations_runs_1
     WHERE tenant_id = ?1 AND id = ?2";

/// Writes the row of a run as the write model sees it: `next_at` is when
/// the runner looks at it again, none once it ended; `paused` whether its
/// scenario is paused, as the scenarios' rows say, and then the run is not
/// due; `shard` is which of the nodes up moves it
/// ([`timada_core::shard_of`] of its id). In the same transaction, the run
/// moves from the count of the status its row had (none for a run not
/// listed yet) to the count of its status now: a write repeated moves
/// nothing.
pub(crate) async fn upsert_run(db: &Db, tenant_id: &str, run: &RunState) -> anyhow::Result<()> {
    let row = (
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
        i64::from(timada_core::shard_of(&run.id)),
        run.started_at as i64,
        run.updated_at as i64,
    );
    db.write(&row, |tx, row| {
        Box::pin(async move {
            let (tenant_id, run_id, scenario_id, status) = (row.0, row.1, row.2, row.6);
            let was: Option<(String,)> =
                fetch_optional(tx, RUN_STATUS, (tenant_id, run_id)).await?;
            let was = was.map(|(was,)| was);
            execute(tx, UPSERT_RUN, *row).await?;
            // Most events of a run (a step taken, done) leave its status.
            if was.as_deref() == Some(status) {
                return Ok(());
            }
            if let Some(was) = &was {
                count_listed(tx, tenant_id, scenario_id, was, -1).await?;
            }
            count_listed(tx, tenant_id, scenario_id, status, 1).await
        })
    })
    .await
}

/// What [`upsert_scenario`] writes on the runs under way of a scenario
/// paused, or going on again, off the index of a scenario's runs (which
/// the planner does not pick by itself, as the table's key leads with the
/// tenant too).
pub(super) const PAUSE_RUNS: &str =
    "UPDATE automations_runs INDEXED BY automations_runs_scenario SET paused = ?3
     WHERE tenant_id = ?1 AND scenario_id = ?2 AND next_at IS NOT NULL";

/// Writes the row of a scenario as the write model sees it. The trigger and
/// the count of steps are the running version's once published, the
/// draft's before. A scenario paused, or going on again (resumed,
/// published again, deleted: a deleted one's runs are due, to be
/// cancelled), marks its runs under way so, in the same transaction: the
/// runner reads none of a paused scenario's runs until it goes on.
pub(crate) async fn upsert_scenario(
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
    let row = (
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
    );
    let paused = scenario.status == ScenarioStatus::Paused;
    db.write(&(row, paused), |tx, (row, paused)| {
        Box::pin(async move {
            let (tenant_id, scenario_id) = (row.0, row.1);
            let stored: Option<(String,)> =
                fetch_optional(tx, SCENARIO_STATUS, (tenant_id, scenario_id)).await?;
            let was_paused = stored.is_some_and(|(status,)| {
                ScenarioStatus::parse(&status) == Some(ScenarioStatus::Paused)
            });
            if was_paused != *paused {
                execute(tx, PAUSE_RUNS, (tenant_id, scenario_id, *paused)).await?;
            }
            execute(
                tx,
                "INSERT INTO automations_scenarios (tenant_id, id, name, status, trigger, version,
                    unpublished, steps, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    name = excluded.name, status = excluded.status, trigger = excluded.trigger,
                    version = excluded.version, unpublished = excluded.unpublished,
                    steps = excluded.steps, updated_at = excluded.updated_at",
                *row,
            )
            .await?;
            Ok(())
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use timada_core::page::PageFrom;

    use super::*;
    use crate::query::{MIGRATIONS, RunFilter, count_runs, prune_runs, runs_of_scenario};
    use crate::value_object::RunStatus::{self, *};

    const TENANT: &str = "acme";

    async fn db() -> Db {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        Db::from(store)
    }

    /// A run of `sc1` started at 1,000, last written at `updated_at`.
    fn run(id: &str, status: RunStatus, updated_at: u64) -> RunState {
        RunState {
            id: id.to_owned(),
            scenario_id: "sc1".to_owned(),
            scenario_version: 1,
            trigger: "order.placed".to_owned(),
            subject_id: format!("order-{id}"),
            status,
            started_at: 1_000,
            updated_at,
            ..RunState::default()
        }
    }

    /// The runs of `sc1` the history counts, by status.
    async fn counts(db: &Db) -> Vec<(RunStatus, u64)> {
        let mut counts = Vec::new();
        for status in RunStatus::ALL {
            let filter = RunFilter {
                status: Some(status),
            };
            let n = count_runs(db, TENANT, "sc1", &filter).await.unwrap();
            if n > 0 {
                counts.push((status, n));
            }
        }
        counts
    }

    /// The ids of a page of `sc1`'s history.
    async fn page(db: &Db, filter: &RunFilter, from: PageFrom<'_>, limit: i64) -> Vec<String> {
        runs_of_scenario(db, TENANT, "sc1", filter, from, limit)
            .await
            .unwrap()
            .rows
            .into_iter()
            .map(|row| row.id)
            .collect()
    }

    /// A run counts once, under the status its row says; written again (a
    /// delivery repeated), it counts the same.
    #[tokio::test]
    async fn the_counts_follow_each_run() {
        let db = db().await;
        upsert_run(&db, TENANT, &run("r1", Running, 1_000))
            .await
            .unwrap();
        upsert_run(&db, TENANT, &run("r1", Running, 1_000))
            .await
            .unwrap();
        upsert_run(&db, TENANT, &run("r2", Running, 1_000))
            .await
            .unwrap();
        assert_eq!(counts(&db).await, [(Running, 2)]);

        let mut waiting = run("r1", Waiting, 1_010);
        waiting.log.push(crate::command::StepRecord {
            step: "s1".to_owned(),
            outcome: crate::command::Outcome::Waiting { until: 5_000 },
            at: 1_010,
        });
        upsert_run(&db, TENANT, &waiting).await.unwrap();
        for _ in 0..2 {
            upsert_run(&db, TENANT, &run("r2", Finished, 1_020))
                .await
                .unwrap();
        }
        assert_eq!(counts(&db).await, [(Waiting, 1), (Finished, 1)]);
        let every = RunFilter::default();
        assert_eq!(count_runs(&db, TENANT, "sc1", &every).await.unwrap(), 2);
        assert_eq!(count_runs(&db, TENANT, "sc2", &every).await.unwrap(), 0);
        assert_eq!(
            count_runs(&db, "tenant-other", "sc1", &every)
                .await
                .unwrap(),
            0
        );
    }

    /// Runs that ended long ago leave, the longest ended first, about as
    /// many at once as asked (with every run that ended in the same second
    /// as the last of them), and their counts with them; runs under way
    /// stay, however old.
    #[tokio::test]
    async fn runs_ended_long_ago_leave_with_their_counts() {
        let db = db().await;
        for (id, status, at) in [
            ("r1", Finished, 100),
            ("r2", Failed, 200),
            ("r3", Finished, 200),
            ("r4", Cancelled, 300),
            ("r5", Running, 50),
            ("r6", Finished, 400),
        ] {
            upsert_run(&db, TENANT, &run(id, status, at)).await.unwrap();
        }
        assert_eq!(prune_runs(&db, 350, 1).await.unwrap(), 1);
        assert_eq!(
            counts(&db).await,
            [(Running, 1), (Finished, 2), (Failed, 1), (Cancelled, 1)]
        );
        assert_eq!(prune_runs(&db, 350, 1).await.unwrap(), 2);
        assert_eq!(
            counts(&db).await,
            [(Running, 1), (Finished, 1), (Cancelled, 1)]
        );
        assert_eq!(prune_runs(&db, 350, 10).await.unwrap(), 1);
        assert_eq!(prune_runs(&db, 350, 10).await.unwrap(), 0);
        assert_eq!(counts(&db).await, [(Running, 1), (Finished, 1)]);
        let every = RunFilter::default();
        assert_eq!(page(&db, &every, PageFrom::First, 10).await, ["r6", "r5"]);

        // Written again (the read model replayed), a run dropped is listed
        // and counted again, until the next look drops it.
        upsert_run(&db, TENANT, &run("r1", Finished, 100))
            .await
            .unwrap();
        assert_eq!(counts(&db).await, [(Running, 1), (Finished, 2)]);
        assert_eq!(prune_runs(&db, 350, 10).await.unwrap(), 1);
        assert_eq!(counts(&db).await, [(Running, 1), (Finished, 1)]);
    }

    /// The history is read a page at a time from where the page beside it
    /// stopped, newest first and the runs started in the same second by
    /// id, every status or one; another scenario's runs never show.
    #[tokio::test]
    async fn the_history_is_read_a_page_at_a_time() {
        let db = db().await;
        for n in 0..7u64 {
            let status = if n % 3 == 0 { Failed } else { Finished };
            let mut started = run(&format!("r{n}"), status, 2_000);
            started.started_at = 1_000 + n / 2;
            upsert_run(&db, TENANT, &started).await.unwrap();
        }
        let mut other = run("r9", Failed, 2_000);
        other.scenario_id = "sc2".to_owned();
        upsert_run(&db, TENANT, &other).await.unwrap();

        let every = RunFilter::default();
        let first = runs_of_scenario(&db, TENANT, "sc1", &every, PageFrom::First, 3)
            .await
            .unwrap();
        let ids = |rows: &[crate::query::RunRow]| -> Vec<String> {
            rows.iter().map(|row| row.id.clone()).collect()
        };
        assert_eq!(ids(&first.rows), ["r6", "r5", "r4"]);
        assert_eq!(first.newer, None);
        let older = first.older.unwrap();
        let second = runs_of_scenario(&db, TENANT, "sc1", &every, PageFrom::Older(&older), 3)
            .await
            .unwrap();
        assert_eq!(ids(&second.rows), ["r3", "r2", "r1"]);
        let last = runs_of_scenario(
            &db,
            TENANT,
            "sc1",
            &every,
            PageFrom::Older(second.older.as_ref().unwrap()),
            3,
        )
        .await
        .unwrap();
        assert_eq!(ids(&last.rows), ["r0"]);
        assert_eq!(last.older, None);
        // Back up: the page above the last one, then the first page.
        let back = PageFrom::Newer(last.newer.as_ref().unwrap());
        assert_eq!(page(&db, &every, back, 3).await, ["r3", "r2", "r1"]);
        let top = PageFrom::Newer(second.newer.as_ref().unwrap());
        assert_eq!(page(&db, &every, top, 3).await, ["r6", "r5", "r4"]);

        let failed = RunFilter {
            status: Some(Failed),
        };
        let first = runs_of_scenario(&db, TENANT, "sc1", &failed, PageFrom::First, 2)
            .await
            .unwrap();
        assert_eq!(ids(&first.rows), ["r6", "r3"]);
        let next = PageFrom::Older(first.older.as_ref().unwrap());
        assert_eq!(page(&db, &failed, next, 2).await, ["r0"]);
        assert_eq!(count_runs(&db, TENANT, "sc1", &failed).await.unwrap(), 3);
        assert_eq!(count_runs(&db, TENANT, "sc1", &every).await.unwrap(), 7);
    }
}
