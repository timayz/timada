//! Read side. The list of a user's studies uses `research_studies` and a
//! study's keywords `research_keywords`, which [`read_model_subscription`]
//! keeps current from the platform's streams. Where each seed's fetch
//! stands is read from the study's own stream, by the page that shows it.

mod keywords;
mod migrations;
mod read_model;
mod studies;

pub use keywords::*;
pub use migrations::*;
pub use read_model::*;
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
