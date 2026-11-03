use timada_aliexpress::{
    ForwardRow, ForwardState, ForwardStatus, LinkRow, LinkState, LinkStatus, MIGRATIONS, Share,
    forwards_due, link_of_product, links_due, note_published, note_read, note_tried, note_wanted,
    upsert_forward, upsert_link,
};
use timada_core::db::{Db, Turso};

/// What a shopper wanted before this does not count any more.
const WANTED_SINCE: i64 = 500;

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
            product_id: Some(format!("{tenant}-{index:04}")),
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

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 10)
        .await
        .unwrap();

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

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 3)
        .await
        .unwrap();

    let ids: Vec<&str> = due.iter().map(|link| link.id.as_str()).collect();
    assert_eq!(ids[..2], ["never-read", "oldest"]);
    assert_eq!(due.len(), 3);
}

#[tokio::test]
async fn what_shoppers_want_comes_first_then_what_is_on_sale_then_the_rest() {
    let db = database().await;
    // Tried longest ago, so going by age alone it would come first.
    seed(&db, "drafts", 3, Some(100)).await;
    seed(&db, "on-sale", 3, Some(200)).await;
    seed(&db, "wanted", 3, Some(300)).await;
    for index in 0..3 {
        let id = format!("on-sale-{index:04}");
        note_published(&db, "on-sale", &id, true).await.unwrap();
    }
    for index in 0..3 {
        let id = format!("wanted-{index:04}");
        note_wanted(&db, "wanted", &id, 600).await.unwrap();
    }

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 9)
        .await
        .unwrap();

    let shops: Vec<&str> = due.iter().map(|link| link.tenant_id.as_str()).collect();
    assert_eq!(
        shops,
        [
            "wanted", "wanted", "wanted", "on-sale", "on-sale", "on-sale", "drafts", "drafts",
            "drafts"
        ]
    );
    // A budget that is short goes to what shoppers want.
    let few = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 2)
        .await
        .unwrap();
    assert!(few.iter().all(|link| link.tenant_id == "wanted"));
}

#[tokio::test]
async fn what_shoppers_wanted_long_ago_is_one_product_among_others() {
    let db = database().await;
    seed(&db, "acme", 2, Some(100)).await;
    note_wanted(&db, "acme", "acme-0001", WANTED_SINCE as u64 - 1)
        .await
        .unwrap();
    note_published(&db, "acme", "acme-0000", true)
        .await
        .unwrap();

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 2)
        .await
        .unwrap();

    assert_eq!(due[0].id, "acme-0000", "on sale, and the other is not");
}

#[tokio::test]
async fn within_each_rank_the_shops_take_turns() {
    let db = database().await;
    // One huge catalogue, tried longest ago, and two small ones, all on sale.
    seed(&db, "a-huge", 30, Some(100)).await;
    seed(&db, "b-small", 2, Some(200)).await;
    seed(&db, "c-small", 2, Some(300)).await;
    for (tenant, count) in [("a-huge", 30), ("b-small", 2), ("c-small", 2)] {
        for index in 0..count {
            let id = format!("{tenant}-{index:04}");
            note_published(&db, tenant, &id, true).await.unwrap();
        }
    }

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 6)
        .await
        .unwrap();

    assert_eq!(count_of(&due, "b-small"), 2);
    assert_eq!(count_of(&due, "c-small"), 2);
    assert_eq!(count_of(&due, "a-huge"), 2);
}

#[tokio::test]
async fn what_makes_a_product_matter_stays_with_that_product() {
    let db = database().await;
    let imported = |product_id: Option<&str>, status| LinkState {
        id: "item".to_owned(),
        product_id: product_id.map(str::to_owned),
        status,
        ..Default::default()
    };
    let wanted = async |db: &Db| -> Vec<String> {
        links_due(db, 1_000, WANTED_SINCE, Share::ALONE, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|link| link.id)
            .collect()
    };
    seed(&db, "acme", 1, Some(100)).await;
    upsert_link(&db, "acme", &imported(Some("first"), LinkStatus::Linked))
        .await
        .unwrap();
    note_wanted(&db, "acme", "first", 600).await.unwrap();
    assert_eq!(wanted(&db).await, ["item", "acme-0000"]);

    // Written again as it stands, the row keeps it.
    upsert_link(&db, "acme", &imported(Some("first"), LinkStatus::Linked))
        .await
        .unwrap();
    assert_eq!(wanted(&db).await, ["item", "acme-0000"]);

    // Imported anew: another product, which nobody wanted yet. It comes behind
    // the other for being tried later.
    upsert_link(&db, "acme", &imported(None, LinkStatus::Importing))
        .await
        .unwrap();
    upsert_link(&db, "acme", &imported(Some("second"), LinkStatus::Linked))
        .await
        .unwrap();
    note_tried(&db, "acme", "item", 950).await.unwrap();
    assert_eq!(wanted(&db).await, ["acme-0000", "item"]);
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

    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 10)
        .await
        .unwrap();

    assert_eq!(count_of(&due, "stale"), 2);
    assert_eq!(due.len(), 2);
}

#[tokio::test]
async fn each_due_item_goes_to_one_node() {
    let db = database().await;
    seed(&db, "acme", 40, Some(100)).await;
    seed(&db, "other", 40, Some(100)).await;

    let mut seen = Vec::new();
    for index in 0..3 {
        let share = Share { index, of: 3 };
        let due = links_due(&db, 1_000, WANTED_SINCE, share, 100)
            .await
            .unwrap();
        assert!(!due.is_empty(), "node {index} has its part");
        for link in &due {
            assert!(share.owns(&link.id));
            seen.push(format!("{}/{}", link.tenant_id, link.id));
        }
    }

    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no item goes to two nodes");
    assert_eq!(seen.len(), 80, "every item goes to a node");
}

#[tokio::test]
async fn an_item_tried_waits_for_the_next_period_whether_or_not_the_read_worked() {
    let db = database().await;
    seed(&db, "acme", 2, Some(100)).await;

    // A shop that cannot read, or a read that failed: tried, and not read.
    note_tried(&db, "acme", "acme-0000", 950).await.unwrap();
    let due = links_due(&db, 1_000, WANTED_SINCE, Share::ALONE, 10)
        .await
        .unwrap();
    let ids: Vec<&str> = due.iter().map(|link| link.id.as_str()).collect();
    assert_eq!(ids, ["acme-0001", "acme-0000"], "the longest untried first");
    assert_eq!(due[1].last_synced_at, Some(100));
    assert_eq!(due[1].last_tried_at, Some(950));

    let due = links_due(&db, 900, WANTED_SINCE, Share::ALONE, 10)
        .await
        .unwrap();
    let ids: Vec<&str> = due.iter().map(|link| link.id.as_str()).collect();
    assert_eq!(ids, ["acme-0001"], "tried after the cutoff, it is not due");
}

#[tokio::test]
async fn a_read_that_changed_nothing_is_kept_when_the_link_writes_its_row_again() {
    let db = database().await;
    let link = LinkState {
        id: "item".to_owned(),
        product_id: Some("product".to_owned()),
        status: LinkStatus::Linked,
        last_synced_at: Some(100),
        ..Default::default()
    };
    upsert_link(&db, "acme", &link).await.unwrap();

    // No event says so: only this node's read model knows of the read.
    note_read(&db, "acme", "item", 500).await.unwrap();
    let row = link_of_product(&db, "acme", "product")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_synced_at, Some(500));
    assert_eq!(row.last_tried_at, Some(500));

    // An older record does not take the time back; a request moves the try.
    let later = LinkState {
        last_requested_at: Some(700),
        ..link
    };
    upsert_link(&db, "acme", &later).await.unwrap();
    let row = link_of_product(&db, "acme", "product")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_synced_at, Some(500));
    assert_eq!(row.last_tried_at, Some(700));
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

    let due = forwards_due(&db, Share::ALONE, 10).await.unwrap();

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

    let due = forwards_due(&db, Share::ALONE, 2).await.unwrap();

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

    let due = forwards_due(&db, Share::ALONE, 6).await.unwrap();

    // The check paces its reads within one run of a shop's forwards.
    let runs = due.chunk_by(|a, b| a.tenant_id == b.tenant_id).count();
    assert_eq!(runs, 2);
}

#[tokio::test]
async fn each_open_order_goes_to_one_node() {
    let db = database().await;
    seed_forwards(&db, "acme", 40, 100).await;
    seed_forwards(&db, "other", 40, 100).await;

    let mut seen = Vec::new();
    for index in 0..3 {
        let share = Share { index, of: 3 };
        let due = forwards_due(&db, share, 100).await.unwrap();
        assert!(!due.is_empty(), "node {index} has its part");
        for forward in &due {
            assert!(share.owns(&forward.id));
            seen.push(format!("{}/{}", forward.tenant_id, forward.id));
        }
    }

    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no order goes to two nodes");
    assert_eq!(seen.len(), 80, "every order goes to a node");
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

    let due = forwards_due(&db, Share::ALONE, 10).await.unwrap();

    assert_eq!(due.len(), 2);
}
