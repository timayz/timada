//! Read side. `proposals` holds one row per proposal of a tenant, which
//! [`read_model_subscription`] keeps current from the tenants' streams:
//! what a shop's page lists as waiting, and what was answered and by whom.
//! A member about to answer a proposal reads it from its stream; the table
//! says which ones there are.

mod migrations;
mod proposals;
mod read_model;

pub use migrations::*;
pub use proposals::*;
pub use read_model::*;

#[cfg(test)]
mod tests {
    use timada_core::db::Db;

    async fn plan(
        db: &Db,
        statement: &str,
        params: impl timada_core::db::IntoParams,
    ) -> Vec<String> {
        db.fetch_all::<(i64, i64, i64, String)>(&format!("EXPLAIN QUERY PLAN {statement}"), params)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect()
    }

    /// The statements a page runs read the index they are written for:
    /// none scans the table whole.
    #[tokio::test]
    async fn the_statements_search_their_keys() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        super::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store);
        for waiting in [true, false] {
            for older in [true, false] {
                let plan = plan(
                    &db,
                    &super::proposals::page_statement(waiting, older),
                    ("shop", 0, "", 50),
                )
                .await;
                assert!(
                    plan.iter().all(|step| step.starts_with("SEARCH")),
                    "{plan:#?}"
                );
            }
        }
        let count = plan(&db, super::proposals::COUNT_WAITING, ("shop",)).await;
        assert!(
            count.iter().all(|step| step.starts_with("SEARCH")),
            "{count:#?}"
        );
        let sources = plan(&db, super::proposals::WAITING_SOURCES, ("shop",)).await;
        assert!(
            sources
                .iter()
                // Adding up by source what the index gives by time.
                .filter(|step| !step.starts_with("USE SORTER"))
                .all(|step| step.starts_with("SEARCH")),
            "{sources:#?}"
        );
    }
}
