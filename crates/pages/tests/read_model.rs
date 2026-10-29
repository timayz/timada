mod common;

use common::*;
use timada_pages::{count_pages, pages_of_tenant, read_model_subscription};

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn pages_are_listed_by_title() {
    let (executor, pool) = executor_with_pool().await;
    let pages = timada_pages::Command::new(executor.clone(), TENANT);
    let terms = pages.create_page(ALICE, draft("Terms")).await.unwrap();
    let about = pages.create_page(ALICE, draft("About")).await.unwrap();
    let legal = pages
        .create_page(ALICE, draft("Legal notice"))
        .await
        .unwrap();
    let draft_page = pages.create_page(ALICE, draft("Draft")).await.unwrap();
    pages.publish_page(ALICE, &terms).await.unwrap();

    catch_up(&executor, &pool).await;
    assert_eq!(count_pages(&pool, TENANT).await.unwrap(), 4);
    let rows = pages_of_tenant(&pool, TENANT).await.unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        [&about, &draft_page, &legal, &terms].map(String::as_str)
    );
    assert!(rows[3].published);
    assert!(!rows[2].published);
    assert_eq!(rows[3].slug, "terms");

    pages.delete_page(ALICE, &about).await.unwrap();
    pages.rename_page(ALICE, &terms, "cgv").await.unwrap();
    catch_up(&executor, &pool).await;
    let rows = pages_of_tenant(&pool, TENANT).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].slug, "cgv");
    assert!(pages_of_tenant(&pool, "other").await.unwrap().is_empty());
}
