//! The return state machine and the approval → refund → refunded flow, driven
//! end to end against a real SQLite database. Every order here walks the real
//! storefront route to `Delivered` first — a return against a faked order
//! would prove nothing about the eligibility rules.

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_catalog::{import_product, load_product, publish_product};
use timada_core::{Currency, Money, ServiceContext, new_id};
use timada_dropship::{MockSupplier, SupplierProduct, SupplierRegistry};
use timada_order::{Address, OrderStatus, OrderView, fulfillment_subscription, load_order};
use timada_payment::{FakePaymentProvider, PaymentStatus, load_payment};
use timada_return::{
    RequestReturnError, ReturnPolicy, ReturnState, ReturnStatus, approve_return, load_return,
    read_models_subscription, recent_returns, reject_return, request_return,
    return_flow_subscription, return_id,
};
use timada_shipping::{refresh_tracking, shipment_id};
use timada_tax::FixedRateVat;

/// A temp-file database plus everything a delivered order needs.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: ReturnState,
    registry: SupplierRegistry,
    tax: Arc<dyn timada_tax::TaxCalculator>,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-return-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        let mut migrations = timada_catalog::migrations();
        migrations.extend(timada_order::migrations());
        migrations.extend(timada_return::migrations());
        migrator.add_migrations(migrations)?;
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
            state: ReturnState {
                ctx,
                provider: Arc::new(FakePaymentProvider),
                policy: ReturnPolicy { window_days: 30 },
            },
            registry,
            tax: Arc::new(FixedRateVat::new(2000)),
        })
    }

    fn executor(&self) -> &timada_core::Executor {
        &self.state.ctx.executor
    }

    /// Walk a fresh order all the way to `Delivered` and return its id.
    async fn delivered_order(&self, price_cents: i64) -> anyhow::Result<String> {
        let cart_id = new_id();
        let product_id = import_product(
            self.executor(),
            "mock",
            SupplierProduct {
                supplier_product_ref: format!("MP-{price_cents}"),
                title: "Aurora Desk Lamp".to_owned(),
                description: "Warm dimmable LED lamp.".to_owned(),
                price: Money::new(price_cents, Currency::Eur),
                image_url: "https://placehold.co/400x400".to_owned(),
            },
        )
        .await?;
        publish_product(self.executor(), &product_id).await?;
        let product = load_product(self.executor(), &product_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no such product"))?;
        timada_cart::add_item(self.executor(), &cart_id, &product, product.base_price(), 1).await?;

        let order_id = timada_order::place_order(
            self.executor(),
            &self.tax,
            &self.pool,
            &cart_id,
            None,
            "ada@example.com".to_owned(),
            Address {
                full_name: "Ada Lovelace".to_owned(),
                street: "12 Analytical Way".to_owned(),
                city: "London".to_owned(),
                postal_code: "NW1 4RT".to_owned(),
                country: "GB".to_owned(),
            },
        )
        .await?;

        self.settle(&order_id).await?;
        let shipment = shipment_id(&order_id, "mock");
        refresh_tracking(self.executor(), &self.registry, &shipment).await?;
        self.settle(&order_id).await?;
        refresh_tracking(self.executor(), &self.registry, &shipment).await?;
        let order = self.settle(&order_id).await?;
        anyhow::ensure!(order.status == OrderStatus::Delivered, "not delivered");

        Ok(order_id)
    }

    /// Run the fulfillment saga until the order stops moving.
    async fn settle(&self, order_id: &str) -> anyhow::Result<OrderView> {
        let mut previous = None;
        for _ in 0..10 {
            fulfillment_subscription(
                self.executor().clone(),
                self.registry.clone(),
                self.state.provider.clone(),
            )
            .no_retry()
            .run_once(self.executor())
            .await?;

            let order = load_order(self.executor(), order_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no such order"))?;
            if previous == Some(order.status) {
                return Ok(order);
            }
            previous = Some(order.status);
        }
        anyhow::bail!("the saga never settled for order {order_id}")
    }

    /// Drain the return flow until the return stops moving.
    async fn drain_return_flow(&self) -> anyhow::Result<()> {
        for _ in 0..5 {
            return_flow_subscription(self.executor().clone(), self.state.provider.clone())
                .no_retry()
                .run_once(self.executor())
                .await?;
        }
        Ok(())
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn an_approved_return_refunds_and_the_order_stays_delivered() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = db.delivered_order(4200).await?;

    let rid = request_return(db.executor(), &db.state.policy, &order_id, "does not fit").await?;
    assert_eq!(rid, return_id(&order_id));

    // While a request is open, a second one is refused.
    assert!(matches!(
        request_return(db.executor(), &db.state.policy, &order_id, "again").await,
        Err(RequestReturnError::AlreadyOpen)
    ));

    approve_return(db.executor(), &rid).await?;
    db.drain_return_flow().await?;

    let current = load_return(db.executor(), &rid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return should exist"))?;
    assert_eq!(current.status, ReturnStatus::Refunded);

    // The charge is refunded, the order untouched.
    let order = load_order(db.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order should exist"))?;
    assert_eq!(
        order.status,
        OrderStatus::Delivered,
        "no rewrite of history"
    );
    let payment = load_payment(
        db.executor(),
        order
            .payment_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("order must know its payment"))?,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("payment should exist"))?;
    assert_eq!(payment.status, PaymentStatus::Refunded);

    // Terminal: a refunded order cannot open a new return.
    assert!(matches!(
        request_return(db.executor(), &db.state.policy, &order_id, "again").await,
        Err(RequestReturnError::AlreadyRefunded)
    ));

    // The admin list followed along.
    read_models_subscription(db.pool.clone())
        .no_retry()
        .run_once(db.executor())
        .await?;
    let rows = recent_returns(&db.pool, 10).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "refunded");

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn a_rejected_return_leaves_the_payment_alone_and_can_be_reargued() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = db.delivered_order(4300).await?;

    let rid = request_return(
        db.executor(),
        &db.state.policy,
        &order_id,
        "changed my mind",
    )
    .await?;
    reject_return(db.executor(), &rid, "outside our policy").await?;
    db.drain_return_flow().await?;

    let current = load_return(db.executor(), &rid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return should exist"))?;
    assert_eq!(current.status, ReturnStatus::Rejected);
    assert_eq!(current.reject_reason.as_deref(), Some("outside our policy"));

    // Nothing was refunded.
    let order = load_order(db.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order should exist"))?;
    let payment = load_payment(
        db.executor(),
        order.payment_id.as_deref().unwrap_or_default(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("payment should exist"))?;
    assert_eq!(payment.status, PaymentStatus::Captured);

    // A rejection is not the end of the conversation.
    request_return(
        db.executor(),
        &db.state.policy,
        &order_id,
        "it really is broken",
    )
    .await?;
    let current = load_return(db.executor(), &rid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return should exist"))?;
    assert_eq!(current.status, ReturnStatus::Requested);
    assert_eq!(current.reason, "it really is broken");

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn only_delivered_orders_within_the_window_can_return() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    // An order that is merely forwarded cannot return.
    let cart_id = new_id();
    let product_id = import_product(
        db.executor(),
        "mock",
        SupplierProduct {
            supplier_product_ref: "MP-early".to_owned(),
            title: "Aurora Desk Lamp".to_owned(),
            description: "Warm dimmable LED lamp.".to_owned(),
            price: Money::new(5100, Currency::Eur),
            image_url: "https://placehold.co/400x400".to_owned(),
        },
    )
    .await?;
    publish_product(db.executor(), &product_id).await?;
    let product = load_product(db.executor(), &product_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no such product"))?;
    timada_cart::add_item(db.executor(), &cart_id, &product, product.base_price(), 1).await?;
    let order_id = timada_order::place_order(
        db.executor(),
        &db.tax,
        &db.pool,
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        Address {
            full_name: "Ada Lovelace".to_owned(),
            street: "12 Analytical Way".to_owned(),
            city: "London".to_owned(),
            postal_code: "NW1 4RT".to_owned(),
            country: "GB".to_owned(),
        },
    )
    .await?;
    db.settle(&order_id).await?;

    assert!(matches!(
        request_return(db.executor(), &db.state.policy, &order_id, "too slow").await,
        Err(RequestReturnError::NotDelivered)
    ));
    assert!(matches!(
        request_return(db.executor(), &db.state.policy, "no-such-order", "x").await,
        Err(RequestReturnError::UnknownOrder)
    ));

    // A zero-day window closes immediately after delivery.
    let strict = ReturnPolicy { window_days: 0 };
    let delivered = db.delivered_order(5200).await?;
    // now > delivered_at + 0 needs a strictly later clock; delivery just
    // happened, so nudge via the smallest possible wait.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(matches!(
        request_return(db.executor(), &strict, &delivered, "late").await,
        Err(RequestReturnError::WindowClosed)
    ));

    db.close().await;
    Ok(())
}
