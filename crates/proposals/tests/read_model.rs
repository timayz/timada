mod common;

use common::*;
use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_proposals::{
    Command, ProposalStatus, answered_proposals, count_waiting, read_model_subscription,
    waiting_proposals, waiting_sources,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_lists_follow_the_proposals() {
    let (executor, pool) = executor_with_pool().await;
    let proposals = Command::new(executor.clone(), TENANT);
    let one = prepared(&proposals).await;
    let two = prepared(&proposals).await;
    let mut by_hand = texts();
    by_hand.source = "person".to_owned();
    let three = proposals.prepare(ALICE, by_hand, NOW).await.unwrap();
    // Another shop's are its own.
    prepared(&Command::new(executor.clone(), "other")).await;

    catch_up(&executor, &pool).await;
    assert_eq!(count_waiting(&pool, TENANT).await.unwrap(), 3);
    assert_eq!(count_waiting(&pool, "other").await.unwrap(), 1);
    assert_eq!(
        waiting_sources(&pool, TENANT).await.unwrap(),
        [("person".to_owned(), 1), ("writer".to_owned(), 2)]
    );
    let waiting = waiting_proposals(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(waiting.rows.len(), 3);
    let row = waiting.rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(row.status(), ProposalStatus::Waiting);
    assert_eq!(row.subject_name, "Gants");
    assert_eq!(row.prepared_by, BOB);
    assert_eq!(row.changes.0, texts().changes);
    assert!(!row.is_expired(NOW));
    assert!(row.is_expired(row.valid_until as u64 + 1));
    assert!(
        answered_proposals(&pool, TENANT, PageFrom::First, 10)
            .await
            .unwrap()
            .rows
            .is_empty()
    );

    proposals
        .approve(
            ALICE,
            &one,
            vec![("name".into(), "Gants d'été".into())],
            NOW,
        )
        .await
        .unwrap();
    proposals.refuse(ALICE, &two, "Not now.").await.unwrap();
    catch_up(&executor, &pool).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool).await;

    assert_eq!(count_waiting(&pool, TENANT).await.unwrap(), 1);
    let waiting = waiting_proposals(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(
        waiting.rows.iter().map(|row| &row.id).collect::<Vec<_>>(),
        [&three]
    );
    let answered = answered_proposals(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(answered.rows.len(), 2);
    let approved = answered.rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(approved.status(), ProposalStatus::Approved);
    assert_eq!(approved.answered_by, ALICE);
    assert!(approved.edited);
    assert_eq!(approved.changes.0[0].after, "Gants d'été");
    let refused = answered.rows.iter().find(|row| row.id == two).unwrap();
    assert_eq!(refused.status(), ProposalStatus::Refused);
    assert_eq!(refused.note, "Not now.");

    proposals.record_applied("saga", &one).await.unwrap();
    catch_up(&executor, &pool).await;
    let answered = answered_proposals(&pool, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    let applied = answered.rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(applied.status(), ProposalStatus::Applied);
    assert_eq!(applied.answered_by, ALICE);
}

#[tokio::test]
async fn a_list_is_read_a_page_at_a_time() {
    let (executor, pool) = executor_with_pool().await;
    let proposals = Command::new(executor.clone(), TENANT);
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(prepared(&proposals).await);
    }
    catch_up(&executor, &pool).await;
    let first = waiting_proposals(&pool, TENANT, PageFrom::First, 2)
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    assert_eq!(first.newer, None);
    let second = waiting_proposals(
        &pool,
        TENANT,
        PageFrom::Older(first.older.as_ref().unwrap()),
        2,
    )
    .await
    .unwrap();
    let third = waiting_proposals(
        &pool,
        TENANT,
        PageFrom::Older(second.older.as_ref().unwrap()),
        2,
    )
    .await
    .unwrap();
    assert_eq!(third.rows.len(), 1);
    assert_eq!(third.older, None);
    let mut seen: Vec<String> = [&first, &second, &third]
        .iter()
        .flat_map(|page| page.rows.iter().map(|row| row.id.clone()))
        .collect();
    seen.sort();
    ids.sort();
    assert_eq!(seen, ids);
    let back = waiting_proposals(
        &pool,
        TENANT,
        PageFrom::Newer(third.newer.as_ref().unwrap()),
        2,
    )
    .await
    .unwrap();
    assert_eq!(back.rows, second.rows);
}
