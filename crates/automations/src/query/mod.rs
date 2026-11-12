//! Read side. Listing a tenant's scenarios and their runs uses
//! `automations_scenarios` and `automations_runs`, and the versions runs
//! walk are `automations_versions`, which [`read_model_subscription`] keeps
//! current from every tenant's streams into one database (every row carries
//! the tenant id). What starts runs is `automations_listeners`, kept by the
//! subscription that starts them ([`listener_subscription`]), with the
//! checkouts it watches and the starts it leaves to other nodes.

mod checkouts;
mod listeners;
mod migrations;
mod read_model;
mod runs;
mod scenarios;
mod starts;
mod versions;

pub use checkouts::*;
pub use listeners::*;
pub use migrations::*;
pub use read_model::*;
pub use runs::*;
pub use scenarios::*;
pub use starts::*;
pub use versions::*;

/// The plans the runner's reads and the pages' are written for, under
/// Turso's planner: each off its index and in its order, never a scan of
/// its table nor a sort, however many rows there are.
#[cfg(test)]
mod plans {
    use timada_core::db::{Db, IntoParams};

    use super::{MIGRATIONS, RunFilter, checkouts, read_model, runs, scenarios, starts};
    use crate::catalog::CHECKOUT_ABANDONED;
    use crate::value_object::RunStatus;

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

    /// The `detail` of each step of `sql`'s plan.
    async fn plan_of(db: &Db, sql: &str, params: impl IntoParams) -> Vec<String> {
        db.fetch_all::<(i64, i64, i64, String)>(&format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect()
    }

    /// Fails unless a step of `plan` searches `index` (rather than scanning
    /// it whole), and when any step sorts.
    fn reads_in_order(plan: &[String], index: &str) {
        assert!(
            plan.iter()
                .any(|step| step.starts_with("SEARCH") && step.contains(index)),
            "no step searches {index}:\n{}",
            plan.join("\n")
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.contains("SORTER") || step.contains("B-TREE")),
            "the plan sorts:\n{}",
            plan.join("\n")
        );
    }

    /// Fails unless a step of `plan` searches `index` by `key` (the
    /// columns it seeks on): an index read whole names it too.
    fn searches(plan: &[String], index: &str, key: &str) {
        assert!(
            plan.iter().any(|step| step.starts_with("SEARCH")
                && step.contains(&format!("INDEX {index} ({key})"))),
            "no step searches {index} by {key}:\n{}",
            plan.join("\n")
        );
    }

    #[tokio::test]
    async fn the_runner_reads_off_its_indexes() {
        let db = db().await;
        let first = plan_of(&db, runs::DUE_RUNS, (100, 3, 1, 50, i64::MIN, "", 200)).await;
        reads_in_order(&first, "automations_runs_due");
        let next = plan_of(&db, runs::DUE_RUNS, (100, 3, 1, 50, 40, "run-1", 200)).await;
        reads_in_order(&next, "automations_runs_due");
        searches(&next, "automations_runs_due", "next_at>=? AND next_at<=?");
        let owed = plan_of(&db, starts::STARTS_OWED, (100, 200)).await;
        reads_in_order(&owed, "automations_starts_kept");
        let left = plan_of(
            &db,
            checkouts::LEFT_CHECKOUTS,
            (0, 100, CHECKOUT_ABANDONED, 200),
        )
        .await;
        reads_in_order(&left, "automations_checkouts_open");
        // Dropping the runs that ended long ago reads the ended ones only.
        let until = plan_of(&db, runs::PRUNE_UNTIL, (100, 999)).await;
        reads_in_order(&until, "automations_runs_ended");
        searches(&until, "automations_runs_ended", "updated_at<?");
        for statement in [runs::PRUNED_COUNTS, runs::PRUNE_RUNS] {
            let plan = plan_of(&db, statement, (100,)).await;
            searches(&plan, "automations_runs_ended", "updated_at<?");
        }
    }

    /// A page of the run history reads its rows off the index of the
    /// scenario's runs (of the status asked for), from either side of where
    /// it stands, and its counts are the ones kept: never the runs of the
    /// scenario, let alone of the tenant, as the list of scenarios.
    #[tokio::test]
    async fn the_run_history_reads_its_page_and_counts_only() {
        let db = db().await;
        let every = RunFilter::default();
        let failed = RunFilter {
            status: Some(RunStatus::Failed),
        };
        for (older, bound) in [(true, "started_at<=?"), (false, "started_at>=?")] {
            let plan = plan_of(
                &db,
                &runs::page_statement(&every, older),
                ("t1", "sc1", None::<String>, 40, "run-1", 51),
            )
            .await;
            reads_in_order(&plan, "automations_runs_scenario");
            searches(
                &plan,
                "automations_runs_scenario",
                &format!("tenant_id=? AND scenario_id=? AND {bound}"),
            );
            let plan = plan_of(
                &db,
                &runs::page_statement(&failed, older),
                ("t1", "sc1", "failed", 40, "run-1", 51),
            )
            .await;
            reads_in_order(&plan, "automations_runs_scenario_status");
            searches(
                &plan,
                "automations_runs_scenario_status",
                &format!("tenant_id=? AND scenario_id=? AND status=? AND {bound}"),
            );
        }
        let counts = plan_of(
            &db,
            &runs::count_statement(&every),
            ("t1", "sc1", None::<String>),
        )
        .await;
        searches(
            &counts,
            "sqlite_autoindex_automations_run_counts_1",
            "tenant_id=? AND scenario_id=?",
        );
        let counts = plan_of(
            &db,
            &runs::count_statement(&failed),
            ("t1", "sc1", "failed"),
        )
        .await;
        searches(
            &counts,
            "sqlite_autoindex_automations_run_counts_1",
            "tenant_id=? AND scenario_id=? AND status=?",
        );
        let listed = plan_of(
            &db,
            scenarios::SCENARIOS_OF_TENANT,
            ("t1", None::<String>, 50, 0),
        )
        .await;
        searches(
            &listed,
            "sqlite_autoindex_automations_run_counts_1",
            "tenant_id=? AND scenario_id=?",
        );
    }

    /// The writes and lookups by key name their index: another index leads
    /// with the tenant too, and the planner picks that one by itself.
    #[tokio::test]
    async fn the_runs_rows_are_written_by_their_keys() {
        let db = db().await;
        let put_off = plan_of(&db, runs::PUT_OFF_RUN, ("t1", "run-1", 40, 400)).await;
        searches(
            &put_off,
            "sqlite_autoindex_automations_runs_1",
            "tenant_id=? AND id=?",
        );
        let pause = plan_of(&db, read_model::PAUSE_RUNS, ("t1", "sc1", true)).await;
        searches(
            &pause,
            "automations_runs_scenario",
            "tenant_id=? AND scenario_id=?",
        );
        for statement in [scenarios::SCENARIO_STATUS, scenarios::SCENARIO_NAME] {
            let plan = plan_of(&db, statement, ("t1", "sc1")).await;
            searches(
                &plan,
                "sqlite_autoindex_automations_scenarios_1",
                "tenant_id=? AND id=?",
            );
        }
        for statement in [runs::RUN_KNOWN, read_model::RUN_STATUS] {
            let plan = plan_of(&db, statement, ("t1", "run-1")).await;
            searches(
                &plan,
                "sqlite_autoindex_automations_runs_1",
                "tenant_id=? AND id=?",
            );
        }
        let upsert = plan_of(
            &db,
            read_model::UPSERT_RUN,
            (
                "t1",
                "run-1",
                "sc1",
                1,
                "order.placed",
                "order-1",
                "running",
                None::<String>,
                None::<String>,
                40,
                7,
                1,
                1,
            ),
        )
        .await;
        searches(
            &upsert,
            "sqlite_autoindex_automations_scenarios_1",
            "tenant_id=? AND id=?",
        );
    }
}
