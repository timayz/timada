use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::{
    AeOrderState, Delivery, DeliveryRead, ForwardRow, ForwardState, ForwardStatus, LinkRow,
    LinkState, LinkStatus, LinkedProduct, MIGRATIONS, NODE_MIGRATIONS, Placement, PlannedItem,
    Share, TABLES, delivery_read, forget_unrecorded, forward_of_order, forward_plan_shown,
    forwards_due, forwards_stalled, keep_unrecorded, link_of_product, linked_products, links_due,
    note_delivery, note_forward_read, note_parcels_followed, note_published, note_read, note_tried,
    note_unrecorded_try, note_wanted, unrecorded, upsert_forward, upsert_link,
};
use timada_catalog::DeliveryDays;
use timada_core::db::{Db, Turso};
use timada_orders::OrderLine;

/// What a shopper wanted before this does not count any more.
const WANTED_SINCE: i64 = 500;

/// A read model with the AliExpress schema.
async fn database() -> Db {
    let store = Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    for migrations in [&MIGRATIONS, &NODE_MIGRATIONS] {
        migrations
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
    }
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
            order_id: format!("order-{index:04}"),
            status: ForwardStatus::Placed,
            updated_at,
            ..Default::default()
        };
        upsert_forward(db, tenant, &forward).await.unwrap();
    }
}

/// The order's page reads its forward whole from the read model.
#[tokio::test]
async fn a_forward_is_read_whole_by_its_order() {
    let db = database().await;
    let forward = ForwardState {
        id: "fwd".to_owned(),
        order_id: "order-1".to_owned(),
        status: ForwardStatus::Placed,
        ae_orders: vec![AeOrderState {
            id: "8001".to_owned(),
            status: "WAIT_SELLER_SEND_GOODS".to_owned(),
            tracking: vec!["LP1".to_owned()],
            ..Default::default()
        }],
        complete: true,
        placed_at: Some(100),
        updated_at: 120,
        ..Default::default()
    };
    upsert_forward(&db, "acme", &forward).await.unwrap();

    let shown = forward_of_order(&db, "acme", "order-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown, forward);
    assert_eq!(
        forward_of_order(&db, "acme", "order-2").await.unwrap(),
        None
    );
    assert_eq!(
        forward_of_order(&db, "zeta", "order-1").await.unwrap(),
        None
    );

    // An order has one forward: a second row for it is refused.
    let twice = ForwardState {
        id: "fwd-2".to_owned(),
        order_id: "order-1".to_owned(),
        ..Default::default()
    };
    assert!(upsert_forward(&db, "acme", &twice).await.is_err());
}

/// The order's page tells an AliExpress line from the rows: the combinations
/// each variant was imported from are in them, so the plan it shows is the
/// forwarder's, without a read of the store.
#[tokio::test]
async fn a_page_tells_an_aliexpress_line_from_the_rows() {
    let db = database().await;
    let sku = |sku_id: &str, variant_id: Option<&str>| SkuLink {
        sku_id: sku_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        cost_cents: 100,
    };
    let linked = |id: &str, ae: &str, product_id: &str, skus: Vec<SkuLink>, at: u64| LinkState {
        id: id.to_owned(),
        ae_product_id: ae.to_owned(),
        product_id: Some(product_id.to_owned()),
        status: LinkStatus::Linked,
        skus,
        updated_at: at,
        ..Default::default()
    };
    let red_and_blue = vec![sku("A1", Some("red")), sku("A2", Some("blue"))];
    upsert_link(
        &db,
        "acme",
        &linked("item-1", "1001", "product-1", red_and_blue.clone(), 100),
    )
    .await
    .unwrap();
    upsert_link(
        &db,
        "acme",
        &linked("item-2", "1002", "product-2", vec![sku("B1", None)], 100),
    )
    .await
    .unwrap();
    // The same product imported again from another item: the latest fills it.
    upsert_link(
        &db,
        "acme",
        &linked("item-3", "1003", "product-2", vec![sku("C1", None)], 200),
    )
    .await
    .unwrap();
    // Unlinked, or another shop's: neither counts.
    let mut unlinked = linked("item-4", "1004", "product-4", vec![sku("D1", None)], 100);
    unlinked.status = LinkStatus::Unlinked;
    upsert_link(&db, "acme", &unlinked).await.unwrap();
    upsert_link(
        &db,
        "zeta",
        &linked("item-5", "1005", "product-5", vec![sku("E1", None)], 100),
    )
    .await
    .unwrap();

    let found = linked_products(
        &db,
        "acme",
        &[
            "product-2",
            "product-1",
            "product-4",
            "product-5",
            "product-2",
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        found,
        vec![
            LinkedProduct {
                ae_product_id: "1001".to_owned(),
                product_id: "product-1".to_owned(),
                skus: red_and_blue,
            },
            LinkedProduct {
                ae_product_id: "1003".to_owned(),
                product_id: "product-2".to_owned(),
                skus: vec![sku("C1", None)],
            },
        ]
    );
    assert!(linked_products(&db, "acme", &[]).await.unwrap().is_empty());

    let line = |product_id: &str, variant_id: Option<&str>, quantity: u32| OrderLine {
        product_id: product_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        name: product_id.to_owned(),
        quantity,
        unit_amount_cents: 1000,
        ..Default::default()
    };
    let plan = forward_plan_shown(
        &db,
        "acme",
        &[
            line("product-1", Some("red"), 2),
            line("product-1", Some("red"), 1),
            line("product-1", Some("hand-made"), 1),
            line("product-2", None, 1),
            line("product-4", None, 1),
        ],
    )
    .await
    .unwrap();
    assert!(!plan.complete, "a variant added by hand, an unlinked item");
    assert_eq!(
        plan.items,
        vec![
            PlannedItem {
                ae_product_id: "1001".to_owned(),
                sku_id: "A1".to_owned(),
                quantity: 3,
            },
            PlannedItem {
                ae_product_id: "1003".to_owned(),
                sku_id: "C1".to_owned(),
                quantity: 1,
            },
        ]
    );
    let none = forward_plan_shown(&db, "acme", &[line("product-own", None, 1)])
        .await
        .unwrap();
    assert!(none.items.is_empty());
}

/// When this node last read a thing is kept for as long as the thing is read
/// again: an unlinked item's row goes, and so does a settled order's.
#[tokio::test]
async fn when_a_node_last_read_a_thing_is_forgotten_once_it_is_no_longer_read() {
    let db = database().await;
    let kept = async |db: &Db| -> Vec<String> {
        let rows: Vec<(String,)> = db
            .fetch_all(
                "SELECT id FROM aliexpress_read_times WHERE tenant_id = 'acme' ORDER BY id",
                (),
            )
            .await
            .unwrap();
        rows.into_iter().map(|(id,)| id).collect()
    };
    seed(&db, "acme", 1, Some(100)).await;
    note_read(&db, "acme", "acme-0000", 500).await.unwrap();
    let placed = ForwardState {
        id: "fwd".to_owned(),
        order_id: "order-1".to_owned(),
        status: ForwardStatus::Placed,
        updated_at: 100,
        ..Default::default()
    };
    upsert_forward(&db, "acme", &placed).await.unwrap();
    note_forward_read(&db, "acme", "fwd", 500).await.unwrap();
    assert_eq!(kept(&db).await, ["acme-0000", "fwd"]);

    // Written again as they stand: kept.
    seed(&db, "acme", 1, Some(100)).await;
    upsert_forward(&db, "acme", &placed).await.unwrap();
    assert_eq!(kept(&db).await, ["acme-0000", "fwd"]);

    // Unlinked: the item's row goes. A read noted later makes one again.
    let unlinked = LinkState {
        id: "acme-0000".to_owned(),
        product_id: Some("acme-0000".to_owned()),
        status: LinkStatus::Unlinked,
        ..Default::default()
    };
    upsert_link(&db, "acme", &unlinked).await.unwrap();
    assert_eq!(kept(&db).await, ["fwd"]);

    // Done: the order's row goes; its parcels' stays, they are followed from
    // here on (the first follow may be noted before the row hears of it).
    note_parcels_followed(&db, "acme", "fwd", 600, true)
        .await
        .unwrap();
    let done = ForwardState {
        status: ForwardStatus::Done,
        ..placed.clone()
    };
    upsert_forward(&db, "acme", &done).await.unwrap();
    assert_eq!(kept(&db).await, ["parcels:fwd"]);

    // Nothing placed: no parcels to follow.
    let failed = ForwardState {
        status: ForwardStatus::Failed,
        ..placed.clone()
    };
    upsert_forward(&db, "acme", &failed).await.unwrap();
    assert!(kept(&db).await.is_empty());
}

/// What delivering an item last cost is kept beside its read times, and
/// goes with them.
#[tokio::test]
async fn what_delivering_an_item_cost_is_kept_until_it_is_unlinked() {
    let db = database().await;
    seed(&db, "acme", 1, Some(100)).await;
    let delivery = Delivery {
        sku_id: "sku-1".to_owned(),
        sku_cost_cents: 431,
        fee_cents: 250,
        days: DeliveryDays::new(7, 12).ok(),
    };
    note_delivery(&db, "acme", "acme-0000", &delivery, 500)
        .await
        .unwrap();
    assert_eq!(
        delivery_read(&db, "acme", "acme-0000").await.unwrap(),
        Some(DeliveryRead {
            delivery: delivery.clone(),
            read_at: 500
        })
    );
    assert_eq!(delivery_read(&db, "acme", "other").await.unwrap(), None);

    // Read again: the later answer stands.
    let free = Delivery {
        fee_cents: 0,
        days: None,
        ..delivery
    };
    note_delivery(&db, "acme", "acme-0000", &free, 600)
        .await
        .unwrap();
    assert_eq!(
        delivery_read(&db, "acme", "acme-0000")
            .await
            .unwrap()
            .map(|read| (read.delivery.fee_cents, read.delivery.days, read.read_at)),
        Some((0, None, 600))
    );

    let unlinked = LinkState {
        id: "acme-0000".to_owned(),
        product_id: Some("acme-0000".to_owned()),
        status: LinkStatus::Unlinked,
        ..Default::default()
    };
    upsert_link(&db, "acme", &unlinked).await.unwrap();
    assert_eq!(delivery_read(&db, "acme", "acme-0000").await.unwrap(), None);
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

    let due = forwards_due(&db, 1_000, 1_000, Share::ALONE, 10)
        .await
        .unwrap();

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
        order_id: "order-oldest".to_owned(),
        status: ForwardStatus::Placed,
        updated_at: 100,
        ..Default::default()
    };
    upsert_forward(&db, "acme", &oldest).await.unwrap();

    let due = forwards_due(&db, 1_000, 1_000, Share::ALONE, 2)
        .await
        .unwrap();

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
            order_id: format!("order-{tenant}-{index}"),
            status: ForwardStatus::Placed,
            updated_at: 100 + index as u64,
            ..Default::default()
        };
        upsert_forward(&db, tenant, &forward).await.unwrap();
    }

    let due = forwards_due(&db, 1_000, 1_000, Share::ALONE, 6)
        .await
        .unwrap();

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
        let due = forwards_due(&db, 1_000, 1_000, share, 100).await.unwrap();
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
async fn an_order_read_or_changed_lately_is_not_due() {
    let db = database().await;
    seed_forwards(&db, "acme", 2, 100).await;
    seed_forwards(&db, "changed", 1, 950).await;
    assert_eq!(
        forwards_due(&db, 1_000, 1_000, Share::ALONE, 10)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        forwards_due(&db, 900, 900, Share::ALONE, 10)
            .await
            .unwrap()
            .len(),
        2
    );

    // Read, by a push or by the sweep: a period to go, whatever it found.
    note_forward_read(&db, "acme", "acme-0000", 950)
        .await
        .unwrap();
    let due = forwards_due(&db, 900, 900, Share::ALONE, 10).await.unwrap();
    let ids: Vec<&str> = due.iter().map(|forward| forward.id.as_str()).collect();
    assert_eq!(ids, ["acme-0001"]);

    // A later change of the row does not bring it back, nor an older read.
    let changed = ForwardState {
        id: "acme-0000".to_owned(),
        order_id: "order-acme-0000".to_owned(),
        status: ForwardStatus::Placed,
        updated_at: 120,
        ..Default::default()
    };
    upsert_forward(&db, "acme", &changed).await.unwrap();
    note_forward_read(&db, "acme", "acme-0000", 500)
        .await
        .unwrap();
    let due = forwards_due(&db, 900, 900, Share::ALONE, 10).await.unwrap();
    assert_eq!(due.len(), 1);
}

#[tokio::test]
async fn the_longest_unread_goes_first_so_a_big_backlog_comes_round() {
    let db = database().await;
    seed_forwards(&db, "acme", 3, 100).await;

    // Reading two a sweep gets through all three.
    let mut read = Vec::new();
    for at in [800, 801] {
        for forward in forwards_due(&db, 900, 900, Share::ALONE, 2).await.unwrap() {
            note_forward_read(&db, "acme", &forward.id, at)
                .await
                .unwrap();
            read.push(forward.id);
        }
    }
    read.sort();
    read.dedup();
    assert_eq!(read.len(), 3);
}

#[tokio::test]
async fn an_order_with_every_parcel_known_stays_due_until_it_is_settled() {
    let db = database().await;
    let order = |id: &str, tracking: &[&str]| AeOrderState {
        id: id.to_owned(),
        tracking: tracking.iter().map(|number| (*number).to_owned()).collect(),
        ..Default::default()
    };
    let forward = |id: &str, complete: bool, ae_orders: Vec<AeOrderState>| ForwardState {
        id: id.to_owned(),
        order_id: format!("order-{id}"),
        status: ForwardStatus::Placed,
        complete,
        ae_orders,
        updated_at: 950,
        ..Default::default()
    };
    // Read just now, and each with its reason not to wait, or to.
    for row in [
        forward(
            "stopped",
            true,
            vec![order("1", &["LP1"]), order("2", &["LP2"])],
        ),
        forward(
            "one-to-go",
            true,
            vec![order("1", &["LP1"]), order("2", &[])],
        ),
        forward("mixed", false, vec![order("1", &["LP1"])]),
        forward("none-yet", true, vec![order("1", &[])]),
    ] {
        upsert_forward(&db, "acme", &row).await.unwrap();
    }

    // Read just now: a day to go for the others, a sweep for the one every
    // parcel of is known.
    let due = forwards_due(&db, 900, 1_000, Share::ALONE, 10)
        .await
        .unwrap();
    let ids: Vec<&str> = due.iter().map(|forward| forward.id.as_str()).collect();
    assert_eq!(ids, ["stopped"]);

    // Not even a sweep gone by: nothing yet.
    assert!(
        forwards_due(&db, 900, 900, Share::ALONE, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn orders_left_being_placed_are_found_once_silent_long_enough() {
    let db = database().await;
    for (id, status, updated_at) in [
        ("started-early", ForwardStatus::Started, 100),
        ("started-late", ForwardStatus::Started, 300),
        ("started-now", ForwardStatus::Started, 950),
        ("requested", ForwardStatus::Requested, 100),
        ("placed", ForwardStatus::Placed, 100),
        ("failed", ForwardStatus::Failed, 100),
        ("done", ForwardStatus::Done, 100),
    ] {
        let forward = ForwardState {
            id: id.to_owned(),
            order_id: format!("order-{id}"),
            status,
            updated_at,
            ..Default::default()
        };
        upsert_forward(&db, "acme", &forward).await.unwrap();
    }

    let stalled = forwards_stalled(&db, 900, Share::ALONE, 10).await.unwrap();

    let ids: Vec<&str> = stalled.iter().map(|forward| forward.id.as_str()).collect();
    assert_eq!(
        ids,
        ["started-early", "started-late"],
        "the longest silent first"
    );
    assert!(stalled.iter().all(|forward| forward.status == "started"));
    let db = &db;
    let on_node = |index| async move {
        forwards_stalled(db, 900, Share { index, of: 2 }, 10)
            .await
            .unwrap()
            .len()
    };
    assert_eq!(on_node(0).await + on_node(1).await, 2, "each on one node");
}

/// What AliExpress took and the store would not record is kept on the node,
/// oldest first, until recorded; a copy from a peer does not empty it.
#[tokio::test]
async fn what_aliexpress_took_is_kept_on_the_node_until_recorded() {
    let db = database().await;
    let placement = |id: &str| Placement {
        ae_order_ids: vec![id.to_owned()],
        items: Vec::new(),
        pay_requested: true,
        complete: true,
    };
    keep_unrecorded(
        &db,
        "acme",
        "late",
        "order-2",
        &placement("82"),
        200,
        "paused",
    )
    .await
    .unwrap();
    keep_unrecorded(
        &db,
        "acme",
        "early",
        "order-1",
        &placement("81"),
        100,
        "paused",
    )
    .await
    .unwrap();
    // Kept again: the first time stays.
    keep_unrecorded(
        &db,
        "acme",
        "early",
        "order-1",
        &placement("81"),
        150,
        "joining",
    )
    .await
    .unwrap();
    // A second placement of the same forward keeps the first's numbers.
    keep_unrecorded(
        &db,
        "acme",
        "early",
        "order-1",
        &placement("91"),
        300,
        "paused",
    )
    .await
    .unwrap();
    let kept = unrecorded(&db, 10).await.unwrap();
    note_unrecorded_try(&db, &kept[0], "still paused")
        .await
        .unwrap();

    let kept = unrecorded(&db, 10).await.unwrap();
    let ids: Vec<(&str, &str)> = kept
        .iter()
        .map(|row| (row.forward_id.as_str(), row.ae_order_ids.as_str()))
        .collect();
    assert_eq!(ids, [("early", "81"), ("late", "82"), ("early", "91")]);
    assert_eq!(kept[0].placed_at, 100);
    assert_eq!(kept[0].tries, 1);
    assert_eq!(kept[0].last_error.as_deref(), Some("still paused"));
    assert_eq!(kept[0].placement().unwrap(), placement("81"));
    assert_eq!(kept[2].tries, 0);

    forget_unrecorded(&db, &kept[0]).await.unwrap();
    let kept = unrecorded(&db, 10).await.unwrap();
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|row| row.ae_order_ids != "81"));
    assert!(!TABLES.contains(&"aliexpress_unrecorded"));
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
            order_id: format!("order-{id}"),
            status,
            ..Default::default()
        };
        upsert_forward(&db, "acme", &forward).await.unwrap();
    }

    let due = forwards_due(&db, 1_000, 1_000, Share::ALONE, 10)
        .await
        .unwrap();

    assert_eq!(due.len(), 2);
}
