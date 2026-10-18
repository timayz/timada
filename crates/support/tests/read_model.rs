mod common;

use common::*;
use timada_support::{
    MessageFilter, MessageStatus, count_messages, messages_of_tenant, read_model_subscription,
};

const TENANT: &str = "tenant-acme";

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT)
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn messages_are_listed_newest_first_by_status() {
    let (executor, pool) = executor_with_pool().await;
    let support = timada_support::Command(executor.clone());
    let one = receive(&support, "First question, about shipping.").await;
    let two = receive(&support, "Second question, about returns.").await;

    catch_up(&executor, &pool).await;
    let any = MessageFilter::default();
    assert_eq!(count_messages(&pool, TENANT, &any).await.unwrap(), 2);
    let rows = messages_of_tenant(&pool, TENANT, &any, 10, 0)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec![two.id.as_str(), one.id.as_str()]
    );
    assert_eq!(rows[1].full_name(), "Ada Lovelace");
    assert_eq!(rows[1].email, "ada@example.com");
    assert_eq!(rows[1].body, "First question, about shipping.");
    assert_eq!(rows[1].language.as_deref(), Some("fr"));
    assert_eq!(rows[1].status, "open");
    assert_eq!(rows[1].handled_at, None);

    support.handle_message(ALICE, &one.id).await.unwrap();
    catch_up(&executor, &pool).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool).await;

    let open = MessageFilter {
        status: Some(MessageStatus::Open),
    };
    let handled = MessageFilter {
        status: Some(MessageStatus::Handled),
    };
    assert_eq!(count_messages(&pool, TENANT, &open).await.unwrap(), 1);
    assert_eq!(count_messages(&pool, TENANT, &handled).await.unwrap(), 1);
    let rows = messages_of_tenant(&pool, TENANT, &handled, 10, 0)
        .await
        .unwrap();
    assert_eq!(rows[0].id, one.id);
    assert!(rows[0].handled_at.is_some());
    // Another tenant sees nothing.
    assert_eq!(
        count_messages(&pool, "tenant-other", &any).await.unwrap(),
        0
    );
    // Paging.
    let page = messages_of_tenant(&pool, TENANT, &any, 1, 1).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, one.id);
}
