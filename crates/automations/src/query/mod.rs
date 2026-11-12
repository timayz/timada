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

/// The plans the runner's reads are written for, under Turso's planner:
/// each off its index and in its order, never a scan of its table nor a
/// sort, however many rows there are.
#[cfg(test)]
mod plans {
    use timada_core::db::{Db, IntoParams};

    use super::{MIGRATIONS, checkouts, runs, starts};
    use crate::catalog::CHECKOUT_ABANDONED;

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

    #[tokio::test]
    async fn the_runner_reads_off_its_indexes() {
        let db = db().await;
        let due = plan_of(&db, runs::DUE_RUNS, (100, 3, 1, 50, 200)).await;
        reads_in_order(&due, "automations_runs_due");
        let owed = plan_of(&db, starts::STARTS_OWED, (100, 200)).await;
        reads_in_order(&owed, "automations_starts_kept");
        let left = plan_of(
            &db,
            checkouts::LEFT_CHECKOUTS,
            (0, 100, CHECKOUT_ABANDONED, 200),
        )
        .await;
        reads_in_order(&left, "automations_checkouts_open");
    }
}
