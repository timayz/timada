use timada_inventory::{
    Availability, Command, InventoryError, RegisterStockItem, RequestBackInStockAlert,
    ReservationOutcome, StockLocation, alert_id, alert_list_subscription, alerts_of_customer,
    back_in_stock_subscription, load_stock_availability, migrations, pending_alerts, stock_item_id,
};

const PRODUCT: &str = "aoc-24g4xe";

#[tokio::test]
async fn reservations_are_bounded_and_idempotent() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    assert_eq!(id, stock_item_id(PRODUCT, &StockLocation::Warehouse));
    let duplicate = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await;
    assert!(matches!(duplicate, Err(InventoryError::AlreadyRegistered)));

    cmd.receive_stock(&id, 5).await?;
    assert_eq!(
        cmd.reserve_stock(&id, "order-1", 3).await?,
        ReservationOutcome::Reserved
    );
    assert_eq!(
        cmd.reserve_stock(&id, "order-2", 3).await?,
        ReservationOutcome::Rejected { available: 2 }
    );

    let view = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(view.product_id, PRODUCT);
    assert_eq!(view.on_hand, 5);
    assert_eq!(view.reserved, 3);
    assert_eq!(view.available, 2);
    assert_eq!(view.status, Availability::InStock);

    // Repeating a reservation for the same order writes nothing.
    assert_eq!(
        cmd.reserve_stock(&id, "order-1", 3).await?,
        ReservationOutcome::Reserved
    );
    let unchanged = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(unchanged, view);

    cmd.release_stock(&id, "order-1").await?;
    cmd.release_stock(&id, "order-1").await?; // no-op
    let released = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(released.available, 5);
    assert_eq!(released.reserved, 0);

    // A customer's return goes back into stock once, however often it is retried.
    cmd.restock_return(&id, "return-1", 2).await?;
    cmd.restock_return(&id, "return-1", 2).await?;
    cmd.restock_return(&id, "return-2", 1).await?;
    let restocked = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(restocked.on_hand, 8);
    assert_eq!(restocked.available, 8);

    Ok(())
}

#[tokio::test]
async fn back_in_stock_alert_fires_on_restock() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let item = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    let alert = cmd
        .request_back_in_stock_alert(RequestBackInStockAlert {
            product_id: PRODUCT.into(),
            customer_id: "customer-1".into(),
            email: "jonathan@example.test".into(),
        })
        .await?;
    assert_eq!(alert, alert_id(PRODUCT, "customer-1"));

    back_in_stock_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    assert_eq!(pending_alerts(&db, PRODUCT).await?, vec![alert.clone()]);

    cmd.receive_stock(&item, 3).await?;
    // First pass fires the alert (writes BackInStockAlertTriggered), second
    // pass folds that event into the SQL row.
    for _ in 0..2 {
        back_in_stock_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await?;
    }

    let state = cmd
        .load_alert(&alert)
        .await?
        .ok_or_else(|| anyhow::anyhow!("alert missing"))?;
    assert!(state.triggered);
    assert!(pending_alerts(&db, PRODUCT).await?.is_empty());

    // Triggering again is a no-op.
    cmd.trigger_back_in_stock_alert(&alert).await?;

    // The shopper's list shows when the product came back.
    let sync_list = || async {
        alert_list_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await
    };
    sync_list().await?;
    let mine = alerts_of_customer(&db, "customer-1").await?;
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].product_id, PRODUCT);
    assert!(mine[0].triggered_at.is_some());
    assert!(alerts_of_customer(&db, "customer-2").await?.is_empty());

    // An alert that fired can be asked for again; a pending one cannot.
    let again = cmd
        .request_back_in_stock_alert(RequestBackInStockAlert {
            product_id: PRODUCT.into(),
            customer_id: "customer-1".into(),
            email: "jonathan@example.test".into(),
        })
        .await?;
    assert_eq!(again, alert);
    let twice = cmd
        .request_back_in_stock_alert(RequestBackInStockAlert {
            product_id: PRODUCT.into(),
            customer_id: "customer-1".into(),
            email: "jonathan@example.test".into(),
        })
        .await;
    assert!(matches!(twice, Err(InventoryError::AlreadyRequested)));
    back_in_stock_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    sync_list().await?;
    assert_eq!(pending_alerts(&db, PRODUCT).await?, vec![alert.clone()]);
    let mine = alerts_of_customer(&db, "customer-1").await?;
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].triggered_at, None);

    // The customer changes their mind: the alert is gone and will not fire.
    // Nobody else can cancel it, and it can be asked for again later.
    let stranger = cmd.cancel_back_in_stock_alert(&alert, "customer-2").await;
    assert!(matches!(stranger, Err(InventoryError::AlertNotFound)));
    cmd.cancel_back_in_stock_alert(&alert, "customer-1").await?;
    cmd.cancel_back_in_stock_alert(&alert, "customer-1").await?;
    back_in_stock_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    sync_list().await?;
    assert!(pending_alerts(&db, PRODUCT).await?.is_empty());
    assert!(alerts_of_customer(&db, "customer-1").await?.is_empty());
    cmd.trigger_back_in_stock_alert(&alert).await?;
    let state = cmd
        .load_alert(&alert)
        .await?
        .ok_or_else(|| anyhow::anyhow!("alert missing"))?;
    assert!(state.cancelled && !state.triggered);
    cmd.request_back_in_stock_alert(RequestBackInStockAlert {
        product_id: PRODUCT.into(),
        customer_id: "customer-1".into(),
        email: "jonathan@example.test".into(),
    })
    .await?;
    back_in_stock_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    assert_eq!(pending_alerts(&db, PRODUCT).await?, vec![alert.clone()]);

    Ok(())
}

#[tokio::test]
async fn stock_levels_can_be_set_outright() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let unknown = cmd.sync_stock_level("nowhere", 4).await;
    assert!(matches!(unknown, Err(InventoryError::StockItemNotFound)));

    let id = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;

    // Ten arrived, three are promised to an order.
    cmd.receive_stock(&id, 10).await?;
    assert_eq!(
        cmd.reserve_stock(&id, "order-1", 3).await?,
        ReservationOutcome::Reserved
    );

    // The supplier says five can still be sold. The three put aside are not
    // theirs to move, so `on_hand` accounts for them as well.
    assert!(cmd.sync_stock_level(&id, 5).await?);
    let synced = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(synced.available, 5);
    assert_eq!(synced.reserved, 3);
    assert_eq!(synced.on_hand, 8);
    assert_eq!(synced.status, Availability::InStock);

    // Saying the same thing again writes nothing: a feed polled every hour
    // appends no event while nothing moves.
    assert!(!cmd.sync_stock_level(&id, 5).await?);
    let unchanged = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(unchanged, synced);

    // A sale between two polls is conservative: `available` drops at once.
    assert_eq!(
        cmd.reserve_stock(&id, "order-2", 2).await?,
        ReservationOutcome::Reserved
    );
    let sold = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(sold.available, 3);

    // Out of stock at the supplier, with five units still promised: nothing
    // left to sell, and no underflow.
    assert!(cmd.sync_stock_level(&id, 0).await?);
    let empty = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(empty.available, 0);
    assert_eq!(empty.reserved, 5);
    assert_eq!(empty.on_hand, 5);
    assert_eq!(empty.status, Availability::OutOfStock);

    // A cancellation gives the units back, which for a supplier-fed item
    // overshoots: the supplier still holds nothing. The level is a snapshot
    // of a moment, and only the next one can correct it — see the note on
    // `StockLevelSynced`.
    cmd.release_stock(&id, "order-1").await?;
    let released = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(released.available, 3);
    assert_eq!(released.reserved, 2);

    // Which the next sync does.
    assert!(cmd.sync_stock_level(&id, 0).await?);
    let corrected = load_stock_availability(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("availability missing"))?;
    assert_eq!(corrected.available, 0);

    Ok(())
}

#[tokio::test]
async fn back_in_stock_alert_fires_on_a_synced_level() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let warehouse = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    let shop = cmd
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Store {
                store_id: "lyon".into(),
            },
        })
        .await?;
    let alert = cmd
        .request_back_in_stock_alert(RequestBackInStockAlert {
            product_id: PRODUCT.into(),
            customer_id: "customer-1".into(),
            email: "jonathan@example.test".into(),
        })
        .await?;
    let sync = || async {
        back_in_stock_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await
    };
    sync().await?;
    assert_eq!(pending_alerts(&db, PRODUCT).await?, vec![alert.clone()]);

    // Shop stock is a shelf the web shop does not sell from: it fires nothing.
    assert!(cmd.sync_stock_level(&shop, 4).await?);
    sync().await?;
    assert_eq!(pending_alerts(&db, PRODUCT).await?, vec![alert.clone()]);

    // The warehouse coming back does.
    assert!(cmd.sync_stock_level(&warehouse, 2).await?);
    for _ in 0..2 {
        sync().await?;
    }
    assert!(pending_alerts(&db, PRODUCT).await?.is_empty());
    let state = cmd
        .load_alert(&alert)
        .await?
        .ok_or_else(|| anyhow::anyhow!("alert missing"))?;
    assert!(state.triggered);

    Ok(())
}
