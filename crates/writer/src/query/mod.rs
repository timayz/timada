//! Read side. `writer_drafts` holds one row per draft a shop asked for,
//! which [`read_model_subscription`] keeps current from the tenants'
//! streams: what the writer's page lists, and whether a draft of a product
//! is on its way. What a day cost is read from its stream, which is the
//! count the caps hold to.

mod drafts;
mod migrations;
mod read_model;

pub use drafts::*;
pub use migrations::*;
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
        for older in [true, false] {
            let plan = plan(
                &db,
                &super::drafts::page_statement(older),
                ("shop", 0, "", 50),
            )
            .await;
            assert!(
                plan.iter().all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
        let open = plan(&db, super::drafts::LATEST_DRAFT, ("shop", "p1", 0)).await;
        assert!(
            open.iter().all(|step| step.starts_with("SEARCH")),
            "{open:#?}"
        );
    }
}
