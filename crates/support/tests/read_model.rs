mod common;

use timada_core::db::Db;

use std::sync::Arc;

use common::*;
use timada_core::Keys;
use timada_support::{
    MessageFilter, MessageStatus, count_messages, messages_of_tenant, read_model_subscription,
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

#[tokio::test]
async fn messages_are_listed_newest_first_by_status() {
    let (executor, pool) = executor_with_pool().await;
    let support = timada_support::Command::new(executor.clone(), TENANT);
    let keys = Arc::new(MemoryKeys::default());
    let one = receive(&support, &keys, "First question, about shipping.").await;
    let two = receive(&support, &keys, "Second question, about returns.").await;
    // Newest first; messages received in the same second fall back to their
    // ids, and two messages written back to back usually share one.
    let mut newest_first = [&one, &two];
    newest_first.sort_by(|a, b| (b.received_at, &b.id).cmp(&(a.received_at, &a.id)));

    catch_up(&executor, &pool, &keys).await.unwrap();
    let any = MessageFilter::default();
    assert_eq!(count_messages(&pool, TENANT, &any).await.unwrap(), 2);
    let rows = messages_of_tenant(&pool, TENANT, &any, 10, 0)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        newest_first.map(|message| message.id.as_str())
    );
    let first = rows.iter().find(|row| row.id == one.id).unwrap();
    assert_eq!(first.full_name(), "Ada Lovelace");
    assert_eq!(first.email, "ada@example.com");
    assert_eq!(first.body, "First question, about shipping.");
    assert_eq!(first.language.as_deref(), Some("fr"));
    assert_eq!(first.status, "open");
    assert_eq!(first.handled_at, None);

    support.handle_message(ALICE, &one.id).await.unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool, &keys).await.unwrap();

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
    assert_eq!(page[0].id, newest_first[1].id);
}

#[tokio::test]
async fn an_erased_message_loses_its_row() {
    let (executor, pool) = executor_with_pool().await;
    let support = timada_support::Command::new(executor.clone(), TENANT);
    let keys = Arc::new(MemoryKeys::default());
    let gone = receive(&support, &keys, "Please forget I ever wrote.").await;
    let kept = receive(&support, &keys, "Do you ship to Belgium?").await;
    catch_up(&executor, &pool, &keys).await.unwrap();
    let any = MessageFilter::default();
    assert_eq!(count_messages(&pool, TENANT, &any).await.unwrap(), 2);

    support
        .forget_message(&*keys, ALICE, &gone.id)
        .await
        .unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    let rows = messages_of_tenant(&pool, TENANT, &any, 10, 0)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec![kept.id.as_str()]
    );
}

#[tokio::test]
async fn a_message_whose_key_is_not_here_yet_is_retried() {
    let (executor, pool) = executor_with_pool().await;
    let support = timada_support::Command::new(executor.clone(), TENANT);
    let keys = Arc::new(MemoryKeys::default());
    let message = receive(&support, &keys, "Where is my order? It is late.").await;
    let state = support.load_message(&message.id).await.unwrap().unwrap();
    let key = keys.get(&state.key_id).await.unwrap().unwrap();

    // The key has not reached this node: the handler fails, the row waits.
    // (The subscription retries that for a long while; one pass shows it.)
    keys.lose_all();
    let one_pass = read_model_subscription::<evento::Turso>(pool.clone(), ring(&keys))
        .no_retry()
        .run_once(&executor)
        .await;
    assert!(one_pass.is_err());
    let any = MessageFilter::default();
    assert_eq!(count_messages(&pool, TENANT, &any).await.unwrap(), 0);

    // Once it arrives, the row follows.
    keys.put(&state.key_id, &key).await.unwrap();
    catch_up(&executor, &pool, &keys).await.unwrap();
    assert_eq!(count_messages(&pool, TENANT, &any).await.unwrap(), 1);
}
