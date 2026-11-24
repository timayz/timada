//! Read side. `google_connections` and `google_search_watch` say which
//! shops' daily pass is due; `google_search_days`, `google_search_queries`,
//! `google_search_pages` and `google_search_page_queries` hold what Search
//! Console said of each site, a day at a time, which [`read_model_subscription`] keeps current from the
//! platform's streams. Whether a shop is connected, and where its pass
//! stands, is read from the streams themselves by the page that shows it:
//! the tables only say where to look and what a period comes to.

mod migrations;
mod read_model;
mod search;

pub use migrations::*;
pub use read_model::*;
pub use search::*;

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

    /// The statements a page and a sweep run read the keys they are written
    /// for: none scans a table whole.
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
        let period = ("shop", "sc-domain:shop.example", "2026-08-01", "2026-11-01");
        for statement in [super::search::TOTALS.to_owned()] {
            let plan = plan(&db, &statement, period).await;
            assert!(
                plan.iter().all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
        for (table, column) in [
            ("google_search_queries", "query"),
            ("google_search_pages", "page"),
        ] {
            let plan = plan(
                &db,
                &super::search::top_statement(table, column),
                (
                    "shop",
                    "sc-domain:shop.example",
                    "2026-08-01",
                    "2026-11-01",
                    100,
                ),
            )
            .await;
            assert!(
                plan.iter()
                    // Adding up by query, then sorting what that gives.
                    .filter(|step| !step.starts_with("USE SORTER"))
                    .all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
        let pairs = plan(
            &db,
            super::search::PAGE_QUERIES,
            (
                "shop",
                "sc-domain:shop.example",
                "2026-08-01",
                "2026-11-01",
                100,
            ),
        )
        .await;
        assert!(
            pairs
                .iter()
                .filter(|step| !step.starts_with("USE SORTER"))
                .all(|step| step.starts_with("SEARCH")),
            "{pairs:#?}"
        );
        let held = plan(
            &db,
            super::search::DAYS_HELD,
            ("shop", "sc-domain:shop.example", "2026-08-01"),
        )
        .await;
        assert!(
            held.iter().all(|step| step.starts_with("SEARCH")),
            "{held:#?}"
        );
        let due = plan(&db, super::search::SEARCHES_DUE, ("", 0, 200)).await;
        assert!(
            due.iter().all(|step| step.starts_with("SEARCH")),
            "{due:#?}"
        );
    }
}
