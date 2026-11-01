mod common;

use timada_core::db::Db;

use std::sync::Arc;

use common::*;
use timada_newsletter::{
    SubscriberFilter, SubscriberStatus, count_subscribers, read_model_subscription,
    subscribers_of_tenant,
};

async fn catch_up(
    executor: &evento::Turso,
    pool: &Db,
    keys: &Arc<MemoryKeys>,
) -> anyhow::Result<()> {
    read_model_subscription::<evento::Turso>(pool.clone(), ring(keys))
        .run_once(executor)
        .await
}

fn only(status: SubscriberStatus) -> SubscriberFilter {
    SubscriberFilter {
        status: Some(status),
    }
}

#[tokio::test]
async fn subscribers_are_listed_by_status() {
    let (executor, pool) = executor_with_pool().await;
    let newsletter = timada_newsletter::Command::new(executor.clone(), TENANT);
    let keys = Arc::new(MemoryKeys::default());
    let (ada, token) = request(&newsletter, &keys, EMAIL).await;
    let (grace, _) = request(&newsletter, &keys, "grace@example.com").await;

    catch_up(&executor, &pool, &keys).await.unwrap();
    let any = SubscriberFilter::default();
    assert_eq!(count_subscribers(&pool, TENANT, &any).await.unwrap(), 2);
    assert_eq!(
        count_subscribers(&pool, TENANT, &only(SubscriberStatus::Pending))
            .await
            .unwrap(),
        2
    );
    let rows = subscribers_of_tenant(&pool, TENANT, &any, 10, 0)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == ada).unwrap();
    assert_eq!(row.email, EMAIL);
    assert_eq!(row.language.as_deref(), Some("fr"));
    assert_eq!(row.source, "form");
    assert_eq!(row.status(), SubscriberStatus::Pending);

    newsletter
        .confirm_subscription(&*keys, VISITOR, &token, now())
        .await
        .unwrap();
    newsletter.unsubscribe(ALICE, &grace).await.unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool, &keys).await.unwrap();

    let confirmed = subscribers_of_tenant(&pool, TENANT, &only(SubscriberStatus::Confirmed), 10, 0)
        .await
        .unwrap();
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].id, ada);
    assert!(confirmed[0].confirmed_at.is_some());
    let gone = subscribers_of_tenant(&pool, TENANT, &only(SubscriberStatus::Unsubscribed), 10, 0)
        .await
        .unwrap();
    assert_eq!(gone[0].id, grace);
    assert!(gone[0].unsubscribed_at.is_some());
    // Another tenant sees nothing.
    assert_eq!(
        count_subscribers(&pool, "tenant-other", &any)
            .await
            .unwrap(),
        0
    );
    // Paging.
    assert_eq!(
        subscribers_of_tenant(&pool, TENANT, &any, 1, 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn an_erased_subscriber_loses_their_row() {
    let (executor, pool) = executor_with_pool().await;
    let newsletter = timada_newsletter::Command::new(executor.clone(), TENANT);
    let keys = Arc::new(MemoryKeys::default());
    let (ada, _) = request(&newsletter, &keys, EMAIL).await;
    catch_up(&executor, &pool, &keys).await.unwrap();
    newsletter
        .forget_subscriber(&*keys, ALICE, &ada)
        .await
        .unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    assert_eq!(
        count_subscribers(&pool, TENANT, &SubscriberFilter::default())
            .await
            .unwrap(),
        0
    );
}
