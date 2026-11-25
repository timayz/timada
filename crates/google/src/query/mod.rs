//! Read side. `google_connections` and `google_search_watch` say which
//! shops' daily pass is due; `google_search_days`, `google_search_queries`,
//! `google_search_pages` and `google_search_page_queries` hold what Search
//! Console said of each site, a day at a time, which [`read_model_subscription`] keeps current from the
//! platform's streams. Whether a shop is connected, and where its pass
//! stands, is read from the streams themselves by the page that shows it:
//! the tables only say where to look and what a period comes to.
//! The `google_ads_*` tables do the same for Google Ads: which shops' pass
//! is due, what the passes asked of the day's allowance, and what each
//! account cost and brought, a day at a time.

mod ads;
mod merchant;
mod migrations;
mod read_model;
mod search;

pub use ads::*;
pub use merchant::*;
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
        let due = plan(&db, super::ads::ADS_DUE, ("", 0, 200)).await;
        assert!(
            due.iter().all(|step| step.starts_with("SEARCH")),
            "{due:#?}"
        );
        let calls = plan(&db, super::ads::CALLS_ON, ("2026-11-20",)).await;
        assert!(
            calls.iter().all(|step| step.starts_with("SEARCH")),
            "{calls:#?}"
        );
        let due = plan(&db, super::merchant::MERCHANT_DUE, ("", 0, 200)).await;
        assert!(
            due.iter().all(|step| step.starts_with("SEARCH")),
            "{due:#?}"
        );
        for statement in [
            super::merchant::TALLY,
            super::merchant::ISSUES_OF,
            super::merchant::ACCOUNT_ISSUES,
        ] {
            let plan = plan(&db, statement, ("shop", "123")).await;
            assert!(
                plan.iter()
                    // Counting by standing sorts what the key gave.
                    .filter(|step| !step.starts_with("USE SORTER FOR GROUP BY"))
                    .all(|step| step.starts_with("SEARCH")),
                "{statement}: {plan:#?}"
            );
        }
        let problems = plan(&db, super::merchant::PROBLEMS, ("shop", "123", 100)).await;
        assert!(
            problems
                .iter()
                .all(|step| step.starts_with("SEARCH")
                    && step.contains("google_merchant_products_rank")),
            "{problems:#?}"
        );
        let account = ("shop", "1234567890", "2026-08-01", "2026-11-01");
        let totals = plan(&db, super::ads::TOTALS, account).await;
        assert!(
            totals.iter().all(|step| step.starts_with("SEARCH")),
            "{totals:#?}"
        );
        use super::ads::{By, by_statement};
        for by in [By::Campaign, By::Device, By::AdGroup] {
            let plan = plan(&db, &by_statement(by), account).await;
            assert!(
                plan.iter()
                    .filter(|step| !step.starts_with("USE SORTER"))
                    .all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
        use super::ads::{Within, within_statement};
        for within in [
            Within::Devices,
            Within::AdGroups,
            Within::Products,
            Within::Terms,
        ] {
            let plan = plan(
                &db,
                &within_statement(within),
                ("shop", "1234567890", "2026-08-01", "2026-11-01", "11", 100),
            )
            .await;
            assert!(
                plan.iter()
                    .filter(|step| !step.starts_with("USE SORTER"))
                    .all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
        for by in [By::Product, By::Term] {
            let plan = plan(
                &db,
                &by_statement(by),
                ("shop", "1234567890", "2026-08-01", "2026-11-01", 100),
            )
            .await;
            assert!(
                plan.iter()
                    .filter(|step| !step.starts_with("USE SORTER"))
                    .all(|step| step.starts_with("SEARCH")),
                "{plan:#?}"
            );
        }
    }
}
