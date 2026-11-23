//! Read side. The list of a user's studies uses `research_studies` and a
//! study's keywords `research_keywords`, which [`read_model_subscription`]
//! keeps current from the platform's streams. Where each seed's fetch
//! stands is read from the study's own stream, by the page that shows it.
//! `research_fetches` says where a seed was last fetched, for a new fetch
//! to reuse, and `research_spend` what each user's calls cost by day, for
//! the platform's administrators; what a cap decides on is the day's own
//! stream, never these.

mod keywords;
mod migrations;
mod read_model;
mod spend;
mod studies;

pub use keywords::*;
pub use migrations::*;
pub use read_model::*;
pub use spend::*;
pub use studies::*;

#[cfg(test)]
mod tests {
    use timada_core::db::Db;

    /// The statements a page runs read the index they are written for, in
    /// its order: no scan of the table, no sort.
    #[tokio::test]
    async fn the_lists_walk_their_indexes() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        super::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store);
        let statements = [
            (
                super::keywords::page_statement(true),
                "research_keywords_volume",
            ),
            (
                super::keywords::page_statement(false),
                "research_keywords_volume",
            ),
        ];
        for (statement, index) in statements {
            let plan = plan(&db, &statement, ("study", 0, "", 10)).await;
            assert!(
                plan.iter().all(|step| step.starts_with("SEARCH")
                    && step.contains(index)
                    && !step.contains("TEMP B-TREE")),
                "{plan:#?}"
            );
        }
        let plan = plan(&db, super::studies::STUDIES_OF_USER, ("user", 10)).await;
        assert!(
            plan.iter().all(|step| step.starts_with("SEARCH")
                && step.contains("research_studies_owner")
                && !step.contains("TEMP B-TREE")),
            "{plan:#?}"
        );
    }

    /// The statements of the spend and of the reuse read their primary
    /// keys: no scan of the table.
    #[tokio::test]
    async fn the_spend_and_the_fetches_are_searched_by_key() {
        let db = db().await;
        let spend = plan(
            &db,
            super::spend::SPEND_BY_USER,
            ("2023-11-14", "2023-11-01", 10),
        )
        .await;
        assert!(spend[0].starts_with("SEARCH"), "{spend:#?}");
        let fetch = plan(&db, super::spend::LAST_FETCH, ("FR", "fr", "griffoir", 0)).await;
        assert!(
            fetch.iter().all(|step| step.starts_with("SEARCH")),
            "{fetch:#?}"
        );
    }

    /// The subscription is at-least-once: a charge delivered again, or
    /// after a later one of its user, moves nothing.
    #[tokio::test]
    async fn a_charge_delivered_twice_counts_once() {
        let db = db().await;
        let count = |cost, version| {
            super::read_model::count_charge(&db, "2023-11-14", "alice", cost, version)
        };
        count(12_000, 2).await.unwrap();
        count(12_000, 2).await.unwrap();
        count(600, 4).await.unwrap();
        count(12_000, 2).await.unwrap();
        count(600, 4).await.unwrap();
        let rows = super::spend_by_user(&db, "2023-11-14", 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].day_usd_micros, rows[0].day_calls), (12_600, 2));
    }

    async fn db() -> Db {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        super::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        Db::from(store)
    }

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
}
