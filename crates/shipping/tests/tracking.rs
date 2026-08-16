//! End-to-end cover for the write side and the admin read model, against a
//! real SQLite database and a real `MockSupplier` — mocking either here would
//! prove nothing about the tracking state machine.

use std::sync::Arc;

use evento::cursor::Args;
use evento::{Aggregate as _, EventFilter, Executor as _};
use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::{ServiceContext, new_id};
use timada_dropship::{MockSupplier, SupplierRegistry};
use timada_shipping::{
    Shipment, ShipmentView, ShippingState, admin_subscription, create_shipment, load_shipment,
    migrations, recent_shipments, refresh_tracking, shipment_id,
};

/// A temp-file database plus the state every test needs, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: ShippingState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-shipping-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(migrations())?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        let ctx = ServiceContext::new(pool.clone(), pool.clone()).await?;
        let registry = SupplierRegistry::builder()
            .register(Arc::new(MockSupplier::new()))
            .build();

        Ok(Self {
            dir,
            pool,
            state: ShippingState { ctx, registry },
        })
    }

    async fn create(&self, order_id: &str) -> anyhow::Result<String> {
        create_shipment(&self.state.ctx.executor, order_id, "mock", "MOCK-REF-1").await
    }

    async fn refresh(&self, id: &str) -> anyhow::Result<()> {
        refresh_tracking(&self.state.ctx.executor, &self.state.registry, id).await
    }

    async fn view(&self, id: &str) -> anyhow::Result<ShipmentView> {
        load_shipment(&self.state.ctx.executor, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("shipment {id} not found"))
    }

    /// Every event ever written for one shipment, oldest first.
    async fn events(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let page = self
            .state
            .ctx
            .executor
            .read(
                Some(vec![EventFilter::by_id(Shipment::aggregate_type(), id)]),
                None,
                Args::forward(50, None),
            )
            .await?;

        Ok(page.edges.into_iter().map(|edge| edge.node.name).collect())
    }

    /// Drain the admin subscription deterministically instead of racing the
    /// background task `start_subscriptions` would spawn.
    async fn drain_admin_subscription(&self) -> anyhow::Result<()> {
        admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(&self.state.ctx.executor)
            .await
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn creating_a_shipment_is_idempotent() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = new_id();

    let id = db.create(&order_id).await?;
    assert_eq!(
        id,
        shipment_id(&order_id, "mock"),
        "the aggregate id must be derivable from (order_id, supplier_id)"
    );
    assert_eq!(db.events(&id).await?, vec!["ShipmentCreated"]);

    // Replaying the command — as the saga will on a redelivered event — must
    // not write a second creation event.
    assert_eq!(db.create(&order_id).await?, id);
    assert_eq!(db.events(&id).await?, vec!["ShipmentCreated"]);

    let view = db.view(&id).await?;
    assert_eq!(view.order_id, order_id);
    assert_eq!(view.supplier_id, "mock");
    assert_eq!(view.external_ref, "MOCK-REF-1");
    assert!(!view.dispatched);
    assert!(!view.delivered);
    assert!(view.tracking_number.is_none());
    assert!(view.carrier.is_none());

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn refreshing_walks_the_shipment_to_delivered_then_stops() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = db.create(&new_id()).await?;

    // The mock supplier reports Dispatched on the first poll for a reference.
    db.refresh(&id).await?;
    let view = db.view(&id).await?;
    assert!(view.dispatched);
    assert!(!view.delivered);
    assert_eq!(view.tracking_number.as_deref(), Some("TRK-MOCK-REF-1"));
    assert_eq!(view.carrier.as_deref(), Some("MockExpress"));

    db.refresh(&id).await?;
    let view = db.view(&id).await?;
    assert!(view.delivered);
    assert_eq!(
        db.events(&id).await?,
        vec!["ShipmentCreated", "ShipmentDispatched", "ShipmentDelivered"]
    );

    // A delivered shipment is never polled again, so no further event can land.
    db.refresh(&id).await?;
    db.refresh(&id).await?;
    assert_eq!(
        db.events(&id).await?,
        vec!["ShipmentCreated", "ShipmentDispatched", "ShipmentDelivered"]
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn refreshing_an_unknown_shipment_is_not_an_error() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    db.refresh("no-such-shipment").await?;
    assert!(
        load_shipment(&db.state.ctx.executor, "no-such-shipment")
            .await?
            .is_none()
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn the_admin_list_tracks_every_shipment_status() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = db.create(&new_id()).await?;

    db.drain_admin_subscription().await?;
    let rows = recent_shipments(&db.pool, 100).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].status, "created");
    assert_eq!(rows[0].external_ref, "MOCK-REF-1");
    assert!(rows[0].tracking_number.is_none());
    assert!(rows[0].carrier.is_none());
    assert!(rows[0].is_refreshable());

    db.refresh(&id).await?;
    db.drain_admin_subscription().await?;
    let rows = recent_shipments(&db.pool, 100).await?;
    assert_eq!(rows.len(), 1, "an update must not insert a second row");
    assert_eq!(rows[0].status, "dispatched");
    assert_eq!(rows[0].tracking_number.as_deref(), Some("TRK-MOCK-REF-1"));
    assert_eq!(rows[0].carrier.as_deref(), Some("MockExpress"));
    assert!(rows[0].is_refreshable());

    db.refresh(&id).await?;
    db.drain_admin_subscription().await?;
    let rows = recent_shipments(&db.pool, 100).await?;
    assert_eq!(rows[0].status, "delivered");
    assert!(
        !rows[0].is_refreshable(),
        "a delivered shipment hides its refresh button"
    );

    // A second pass resumes from the persisted cursor and changes nothing.
    db.drain_admin_subscription().await?;
    let rows = recent_shipments(&db.pool, 100).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "delivered");

    db.close().await;
    Ok(())
}
