mod common;

use common::*;
use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_research::aggregator::{Created, KeywordsFetched};
use timada_research::{
    RESEARCH_ACTOR, keywords_of_study, read_model_subscription, studies_of_user,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// The keywords of a study, the most searched first, as `(seed, keyword)`.
async fn listed(db: &Db, study_id: &str) -> Vec<(String, String)> {
    keywords_of_study(db, study_id, PageFrom::First, 1_000)
        .await
        .unwrap()
        .rows
        .into_iter()
        .map(|row| (row.seed, row.keyword))
        .collect()
}

#[tokio::test]
async fn a_users_studies_are_listed_with_their_counts() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let cats = study(&research, &["griffoir", "arbre à chat"]).await;
    let theirs = research
        .create_study(MALLORY, draft("Litières", &["litière"]))
        .await
        .unwrap();
    fetch(
        &research,
        &cats.id,
        "griffoir",
        vec![
            row("griffoir", Some(12_100)),
            row("griffoir carton", Some(880)),
        ],
    )
    .await;
    research
        .rename_study(ALICE, &cats.id, "Griffoirs")
        .await
        .unwrap();

    catch_up(&executor, &db).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &db).await;
    let mine = studies_of_user(&db, ALICE, 10).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, cats.id);
    assert_eq!(mine[0].name, "Griffoirs");
    assert_eq!(
        (mine[0].country.as_str(), mine[0].language.as_str()),
        ("FR", "fr")
    );
    assert_eq!(mine[0].seed_count, 2);
    assert_eq!(mine[0].keyword_count, 2);
    let others = studies_of_user(&db, MALLORY, 10).await.unwrap();
    assert_eq!(others.len(), 1);
    assert_eq!(others[0].id, theirs.id);
    assert_eq!(others[0].keyword_count, 0);
}

#[tokio::test]
async fn keywords_read_back_per_study_the_most_searched_first() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["griffoir", "arbre à chat"]).await;
    fetch(
        &research,
        &study.id,
        "griffoir",
        vec![
            row("griffoir", Some(12_100)),
            row("griffoir carton", Some(880)),
            // Nobody knows how often it is searched: last, not first.
            row("griffoir diy", None),
        ],
    )
    .await;
    fetch(
        &research,
        &study.id,
        "arbre à chat",
        vec![
            row("arbre à chat", Some(40_500)),
            // Two keywords of one volume keep the order the source gave.
            row("arbre à chat xxl", Some(880)),
            row("arbre à chat mural", Some(880)),
        ],
    )
    .await;
    catch_up(&executor, &db).await;

    let page = keywords_of_study(&db, &study.id, PageFrom::First, 10)
        .await
        .unwrap();
    assert!(page.older.is_none() && page.newer.is_none());
    let first = &page.rows[0];
    assert_eq!(first.keyword, "arbre à chat");
    assert_eq!(first.seed, "arbre à chat");
    assert_eq!(first.volume, Some(40_500));
    assert_eq!(first.cpc_usd_cents, Some(42));
    assert_eq!(first.difficulty, Some(38));
    assert_eq!(first.intent.as_deref(), Some("commercial"));
    assert!(first.fetched_at > 0);
    assert_eq!(page.rows.last().unwrap().volume, None);
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.keyword.as_str())
            .collect::<Vec<_>>(),
        [
            "arbre à chat",
            "griffoir",
            "arbre à chat xxl",
            "arbre à chat mural",
            "griffoir carton",
            "griffoir diy",
        ]
    );

    // Page after page, down and back up, with no row twice or missing.
    let top = keywords_of_study(&db, &study.id, PageFrom::First, 4)
        .await
        .unwrap();
    assert_eq!(top.rows.len(), 4);
    let cursor = top.older.clone().expect("a page beyond");
    let rest = keywords_of_study(&db, &study.id, PageFrom::Older(&cursor), 4)
        .await
        .unwrap();
    assert_eq!(
        rest.rows
            .iter()
            .map(|row| row.keyword.as_str())
            .collect::<Vec<_>>(),
        ["griffoir carton", "griffoir diy"]
    );
    assert!(rest.older.is_none());
    let back = keywords_of_study(
        &db,
        &study.id,
        PageFrom::Newer(rest.newer.as_ref().unwrap()),
        4,
    )
    .await
    .unwrap();
    assert_eq!(back.rows, top.rows);
}

#[tokio::test]
async fn the_rows_of_a_fetch_replace_the_earlier_rows_of_the_same_seed() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["griffoir", "arbre à chat"]).await;
    fetch(
        &research,
        &study.id,
        "griffoir",
        vec![row("griffoir", Some(100)), row("griffoir carton", Some(50))],
    )
    .await;
    fetch(
        &research,
        &study.id,
        "arbre à chat",
        vec![row("arbre à chat", Some(10))],
    )
    .await;
    catch_up(&executor, &db).await;
    assert_eq!(listed(&db, &study.id).await.len(), 3);

    fetch(
        &research,
        &study.id,
        "griffoir",
        vec![row("griffoir", Some(120)), row("griffoir mural", Some(70))],
    )
    .await;
    // A fetch that fails afterwards leaves them.
    research
        .request_fetch(ALICE, &study.id, Some("griffoir"))
        .await
        .unwrap();
    research
        .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
        .await
        .unwrap();
    research
        .record_fetch_failure(RESEARCH_ACTOR, &study.id, "griffoir", "no answer")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let pair = |seed: &str, keyword: &str| (seed.to_owned(), keyword.to_owned());
    assert_eq!(
        listed(&db, &study.id).await,
        [
            pair("griffoir", "griffoir"),
            pair("griffoir", "griffoir mural"),
            pair("arbre à chat", "arbre à chat"),
        ]
    );
    assert_eq!(
        studies_of_user(&db, ALICE, 10).await.unwrap()[0].keyword_count,
        3
    );

    // A read model built again from the stream alone holds the same rows.
    let before = keywords_of_study(&db, &study.id, PageFrom::First, 10)
        .await
        .unwrap();
    for table in timada_research::TABLES {
        db.execute(&format!("DELETE FROM {table}"), ())
            .await
            .unwrap();
    }
    db.execute(
        "DELETE FROM subscriber WHERE key = ?1",
        (timada_research::READ_MODEL_SUBSCRIPTION,),
    )
    .await
    .unwrap();
    catch_up(&executor, &db).await;
    let rebuilt = keywords_of_study(&db, &study.id, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(rebuilt, before);
}

#[tokio::test]
async fn a_thousand_rows_are_written_and_read_whole() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["griffoir"]).await;
    let rows = (0..1_000)
        .map(|n| row(&format!("griffoir {n}"), Some(1_000 - n)))
        .collect();
    fetch(&research, &study.id, "griffoir", rows).await;
    catch_up(&executor, &db).await;
    let listed = listed(&db, &study.id).await;
    assert_eq!(listed.len(), 1_000);
    assert_eq!(listed[0].1, "griffoir 0");
    assert_eq!(listed[999].1, "griffoir 999");
    assert_eq!(
        studies_of_user(&db, ALICE, 10).await.unwrap()[0].keyword_count,
        1_000
    );
}

#[tokio::test]
async fn a_deleted_study_leaves_no_row() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let gone = study(&research, &["griffoir"]).await;
    let kept = study(&research, &["litière"]).await;
    fetch(
        &research,
        &gone.id,
        "griffoir",
        vec![row("griffoir", Some(1))],
    )
    .await;
    fetch(
        &research,
        &kept.id,
        "litière",
        vec![row("litière", Some(2))],
    )
    .await;
    catch_up(&executor, &db).await;
    assert_eq!(studies_of_user(&db, ALICE, 10).await.unwrap().len(), 2);

    research.delete_study(ALICE, &gone.id).await.unwrap();
    catch_up(&executor, &db).await;
    let mine = studies_of_user(&db, ALICE, 10).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, kept.id);
    assert!(listed(&db, &gone.id).await.is_empty());
    assert_eq!(listed(&db, &kept.id).await.len(), 1);
}

/// A study lives in the platform's tenant: a copy of its events written in
/// a shop's tenant is never read.
#[tokio::test]
async fn a_study_forged_in_a_shops_tenant_is_not_listed() {
    let (executor, db) = executor_with_pool().await;
    evento::append("forged-study")
        .original_version(0)
        .event(&Created {
            name: "Forged".to_owned(),
            country: "FR".to_owned(),
            language: "fr".to_owned(),
            created_by: ALICE.to_owned(),
        })
        .event(&KeywordsFetched {
            seed: "griffoir".to_owned(),
            rows: vec![row("griffoir", Some(1))],
            cost_usd_micros: 0,
            fetched_by: RESEARCH_ACTOR.to_owned(),
        })
        .tenant("zeta")
        .commit(&executor)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(studies_of_user(&db, ALICE, 10).await.unwrap().is_empty());
    assert!(listed(&db, "forged-study").await.is_empty());
}
