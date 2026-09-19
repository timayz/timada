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

    Ok(())
}
