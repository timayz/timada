mod common;

use common::*;
use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_writer::{
    Command, DRAFT_STALE_SECS, DraftStatus, drafts_of_tenant, open_draft_of,
    read_model_subscription,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_list_follows_the_drafts() {
    let (executor, pool) = executor_with_pool().await;
    let writer = Command::new(executor.clone(), TENANT);
    let one = writer.request_draft(ALICE, gloves()).await.unwrap();
    let two = writer.request_draft(ALICE, gloves()).await.unwrap();
    Command::new(executor.clone(), "other")
        .request_draft(ALICE, gloves())
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let listed = drafts_of_tenant(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(listed.rows.len(), 2);
    let row = listed.rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(row.status(), DraftStatus::Requested);
    assert_eq!(row.subject_name, "Gants");
    assert_eq!(row.keyword, "gants moto été");
    let now = row.requested_at as u64;
    // One is on its way for the product; none for another.
    assert!(
        open_draft_of(&pool, TENANT, "p1", now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        open_draft_of(&pool, TENANT, "p2", now)
            .await
            .unwrap()
            .is_none()
    );
    // Left without an outcome for too long, it no longer holds the place.
    assert!(
        open_draft_of(&pool, TENANT, "p1", now + DRAFT_STALE_SECS)
            .await
            .unwrap()
            .is_none()
    );

    for id in [&one, &two] {
        writer.start_draft("node-a", id).await.unwrap();
    }
    writer
        .record_draft(
            "saga",
            &one,
            "proposal-1",
            21_500,
            vec!["30 cm".into(), "cuir".into()],
        )
        .await
        .unwrap();
    writer
        .record_draft_failure("saga", &two, "The day's cap is reached.")
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool).await;

    let listed = drafts_of_tenant(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    let drafted = listed.rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(drafted.status(), DraftStatus::Drafted);
    assert_eq!(drafted.proposal_id, "proposal-1");
    assert_eq!(drafted.cost_usd_micros, 21_500);
    assert_eq!(drafted.to_check, "30 cm\ncuir");
    let failed = listed.rows.iter().find(|row| row.id == two).unwrap();
    assert_eq!(failed.status(), DraftStatus::Failed);
    assert_eq!(failed.reason, "The day's cap is reached.");
    assert!(
        open_draft_of(&pool, TENANT, "p1", now)
            .await
            .unwrap()
            .is_none()
    );
}
