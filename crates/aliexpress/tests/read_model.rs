use timada_aliexpress::{
    ForwardRow, ForwardState, ForwardStatus, LinkRow, LinkState, LinkStatus, MIGRATIONS,
    forwards_due, links_due, upsert_forward, upsert_link,
};
use timada_core::db::{Db, Turso};

/// A read model with the AliExpress schema.
async fn database() -> Db {
    let store = Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    Db::from(store)
}

/// Links `count` items of `tenant`, last read at `last_synced_at`.
async fn seed(db: &Db, tenant: &str, count: usize, last_synced_at: Option<u64>) {
    for index in 0..count {
        let link = LinkState {
            id: format!("{tenant}-{index:04}"),
            status: LinkStatus::Linked,
            last_synced_at,
            ..Default::default()
        };
        upsert_link(db, tenant, &link).await.unwrap();
    }
}

fn count_of(due: &[LinkRow], tenant: &str) -> usize {
    due.iter().filter(|link| link.tenant_id == tenant).count()
}

#[tokio::test]
async fn a_backlog_leaves_the_other_shops_their_turn() {
    let db = database().await;
    // Sorts first and was read longest ago, as a shop that no longer answers.
    seed(&db, "a-backlog", 30, Some(100)).await;
    seed(&db, "b-small", 3, Some(200)).await;
    seed(&db, "c-small", 3, Some(300)).await;

    let due = links_due(&db, 1_000, 10).await.unwrap();

    assert_eq!(due.len(), 10);
    assert_eq!(count_of(&due, "b-small"), 3);
    assert_eq!(count_of(&due, "c-small"), 3);
    assert_eq!(count_of(&due, "a-backlog"), 4);
}

#[tokio::test]
async fn a_shop_alone_fills_the_sweep_its_oldest_first() {
    let db = database().await;
    seed(&db, "acme", 2, Some(500)).await;
    let oldest = LinkState {
        id: "oldest".to_owned(),
        status: LinkStatus::Linked,
        last_synced_at: Some(100),
        ..Default::default()
    };
    upsert_link(&db, "acme", &oldest).await.unwrap();
    let never_read = LinkState {
        id: "never-read".to_owned(),
        status: LinkStatus::Linked,
        ..Default::default()
    };
    upsert_link(&db, "acme", &never_read).await.unwrap();

    let due = links_due(&db, 1_000, 3).await.unwrap();

    let ids: Vec<&str> = due.iter().map(|link| link.id.as_str()).collect();
    assert_eq!(ids[..2], ["never-read", "oldest"]);
    assert_eq!(due.len(), 3);
}

#[tokio::test]
async fn the_due_links_of_a_shop_come_together() {
    let db = database().await;
    // The two shops' reads alternate in time.
    for (index, tenant) in ["a", "b", "a", "b", "a", "b"].into_iter().enumerate() {
        let link = LinkState {
            id: format!("{tenant}-{index}"),
            status: LinkStatus::Linked,
            last_synced_at: Some(100 + index as u64),
            ..Default::default()
        };
        upsert_link(&db, tenant, &link).await.unwrap();
    }

    let due = links_due(&db, 1_000, 6).await.unwrap();

    // The sweep paces its reads within one run of a shop's links.
    let runs = due.chunk_by(|a, b| a.tenant_id == b.tenant_id).count();
    assert_eq!(runs, 2);
}

#[tokio::test]
async fn only_linked_items_read_before_the_cutoff_are_due() {
    let db = database().await;
    seed(&db, "fresh", 2, Some(1_000)).await;
    seed(&db, "stale", 2, Some(999)).await;
    let failed = LinkState {
        id: "failed".to_owned(),
        status: LinkStatus::Failed,
        ..Default::default()
    };
    upsert_link(&db, "stale", &failed).await.unwrap();

    let due = links_due(&db, 1_000, 10).await.unwrap();

    assert_eq!(count_of(&due, "stale"), 2);
    assert_eq!(due.len(), 2);
}

/// Places `count` orders of `tenant` on AliExpress, last changed at `updated_at`.
async fn seed_forwards(db: &Db, tenant: &str, count: usize, updated_at: u64) {
    for index in 0..count {
        let forward = ForwardState {
            id: format!("{tenant}-{index:04}"),
            status: ForwardStatus::Placed,
            updated_at,
            ..Default::default()
        };
        upsert_forward(db, tenant, &forward).await.unwrap();
    }
}

fn forwards_of(due: &[ForwardRow], tenant: &str) -> usize {
    due.iter()
        .filter(|forward| forward.tenant_id == tenant)
        .count()
}

#[tokio::test]
async fn open_orders_leave_the_other_shops_their_turn() {
    let db = database().await;
    // Sorts first and changed longest ago, as a shop that no longer answers.
    seed_forwards(&db, "a-backlog", 30, 100).await;
    seed_forwards(&db, "b-small", 3, 200).await;
    seed_forwards(&db, "c-small", 3, 300).await;

    let due = forwards_due(&db, 10).await.unwrap();

    assert_eq!(due.len(), 10);
    assert_eq!(forwards_of(&due, "b-small"), 3);
    assert_eq!(forwards_of(&due, "c-small"), 3);
    assert_eq!(forwards_of(&due, "a-backlog"), 4);
}

#[tokio::test]
async fn a_shop_alone_fills_the_check_longest_unchanged_first() {
    let db = database().await;
    seed_forwards(&db, "acme", 2, 500).await;
    let oldest = ForwardState {
        id: "oldest".to_owned(),
        status: ForwardStatus::Placed,
        updated_at: 100,
        ..Default::default()
    };
    upsert_forward(&db, "acme", &oldest).await.unwrap();

    let due = forwards_due(&db, 2).await.unwrap();

    assert_eq!(due[0].id, "oldest");
    assert_eq!(due.len(), 2);
}

#[tokio::test]
async fn the_due_forwards_of_a_shop_come_together() {
    let db = database().await;
    // The two shops' changes alternate in time.
    for (index, tenant) in ["a", "b", "a", "b", "a", "b"].into_iter().enumerate() {
        let forward = ForwardState {
            id: format!("{tenant}-{index}"),
            status: ForwardStatus::Placed,
            updated_at: 100 + index as u64,
            ..Default::default()
        };
        upsert_forward(&db, tenant, &forward).await.unwrap();
    }

    let due = forwards_due(&db, 6).await.unwrap();

    // The check paces its reads within one run of a shop's forwards.
    let runs = due.chunk_by(|a, b| a.tenant_id == b.tenant_id).count();
    assert_eq!(runs, 2);
}

#[tokio::test]
async fn only_placed_orders_are_due() {
    let db = database().await;
    seed_forwards(&db, "acme", 2, 100).await;
    for (id, status) in [
        ("requested", ForwardStatus::Requested),
        ("started", ForwardStatus::Started),
        ("failed", ForwardStatus::Failed),
        ("done", ForwardStatus::Done),
    ] {
        let forward = ForwardState {
            id: id.to_owned(),
            status,
            ..Default::default()
        };
        upsert_forward(&db, "acme", &forward).await.unwrap();
    }

    let due = forwards_due(&db, 10).await.unwrap();

    assert_eq!(due.len(), 2);
}
