//! End-to-end cover for the write side and the admin read model, against a
//! real SQLite database — mocking the event store here would prove nothing.

use std::sync::Arc;

use evento::cursor::Args;
use evento::{Aggregate as _, EventFilter, Executor as _};
use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::{ServiceContext, new_id};
use timada_dropship::{
    DropshipState, MockSupplier, SupplierLine, SupplierOrder, SupplierOrderStatus,
    SupplierRegistry, admin_subscription, forward_order, load_supplier_order, migrations,
    recent_supplier_orders, supplier_order_id,
};

/// A temp-file database plus the state every test needs, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: DropshipState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-dropship-{}", new_id()));
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
            state: DropshipState { ctx, registry },
        })
    }

    /// Every event ever written for one supplier order, oldest first.
    async fn events(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let page = self
            .state
            .ctx
            .executor
            .read(
                Some(vec![EventFilter::by_id(
                    SupplierOrder::aggregate_type(),
                    id,
                )]),
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

fn line() -> SupplierLine {
    SupplierLine {
        supplier_product_ref: "MP-1001".to_owned(),
        title: "Aurora Desk Lamp".to_owned(),
        quantity: 2,
    }
}

#[tokio::test]
async fn forwarding_confirms_the_order_and_is_idempotent() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = new_id();

    let id = forward_order(
        &db.state.ctx.executor,
        &db.state.registry,
        &order_id,
        "mock",
        vec![line()],
    )
    .await?;

    assert_eq!(
        id,
        supplier_order_id(&order_id, "mock"),
        "the aggregate id must be derivable from (order_id, supplier_id)"
    );
    assert_eq!(
        db.events(&id).await?,
        vec!["SupplierOrderPlaced", "SupplierOrderConfirmed"]
    );

    // Replaying the command — as the saga will on a redelivered event — must
    // not re-contact the supplier or write anything new.
    let replayed = forward_order(
        &db.state.ctx.executor,
        &db.state.registry,
        &order_id,
        "mock",
        vec![line()],
    )
    .await?;

    assert_eq!(replayed, id);
    assert_eq!(
        db.events(&id).await?,
        vec!["SupplierOrderPlaced", "SupplierOrderConfirmed"]
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn the_view_maps_a_supplier_order_back_to_its_order() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = new_id();

    let id = forward_order(
        &db.state.ctx.executor,
        &db.state.registry,
        &order_id,
        "mock",
        vec![line()],
    )
    .await?;

    // This is the lookup the fulfillment saga does: a `SupplierOrderConfirmed`
    // event carries only the aggregate id, so the order it belongs to has to
    // come from replaying the aggregate.
    let Some(view) = load_supplier_order(&db.state.ctx.executor, &id).await? else {
        panic!("a forwarded order must be loadable");
    };

    assert_eq!(view.id, id);
    assert_eq!(view.order_id, order_id);
    assert_eq!(view.supplier_id, "mock");
    assert_eq!(view.status, SupplierOrderStatus::Confirmed);
    assert!(
        view.external_ref
            .as_deref()
            .is_some_and(|external_ref| external_ref.starts_with("MOCK-"))
    );
    assert!(view.reason.is_none());

    assert!(
        load_supplier_order(&db.state.ctx.executor, "no-such-supplier-order")
            .await?
            .is_none()
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn an_unregistered_supplier_is_recorded_as_a_rejection() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = new_id();

    let id = forward_order(
        &db.state.ctx.executor,
        &db.state.registry,
        &order_id,
        "aliexpress",
        vec![line()],
    )
    .await?;

    assert_eq!(
        db.events(&id).await?,
        vec!["SupplierOrderPlaced", "SupplierOrderRejected"],
        "a misconfigured registry must not surface as a command error"
    );

    let Some(view) = load_supplier_order(&db.state.ctx.executor, &id).await? else {
        panic!("a rejected order must still be loadable");
    };
    assert_eq!(view.status, SupplierOrderStatus::Rejected);
    assert_eq!(view.order_id, order_id);
    assert!(view.external_ref.is_none());
    assert!(view.reason.is_some());

    db.drain_admin_subscription().await?;
    let rows = recent_supplier_orders(&db.pool, 50).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "rejected");
    assert!(
        rows[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("aliexpress"))
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn the_admin_list_records_every_confirmed_supplier_order() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    for _ in 0..3 {
        forward_order(
            &db.state.ctx.executor,
            &db.state.registry,
            &new_id(),
            "mock",
            vec![line()],
        )
        .await?;
    }

    db.drain_admin_subscription().await?;
    let rows = recent_supplier_orders(&db.pool, 50).await?;

    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.status, "confirmed");
        assert_eq!(row.supplier_id, "mock");
        assert!(
            row.external_ref
                .as_deref()
                .is_some_and(|external_ref| external_ref.starts_with("MOCK-"))
        );
        assert!(row.reason.is_none());
    }

    // A second pass resumes from the persisted cursor and adds nothing.
    db.drain_admin_subscription().await?;
    assert_eq!(recent_supplier_orders(&db.pool, 50).await?.len(), 3);

    db.close().await;
    Ok(())
}
