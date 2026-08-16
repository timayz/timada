//! End-to-end vertical slice: browse → cart → checkout → paid (fake provider)
//! → forwarded to the mock supplier → shipment tracked → delivered — plus the
//! payment-decline compensation path.
//!
//! Every subscription is driven with `.no_retry()` + `.run_once(..)` between
//! steps, so saga progression is deterministic instead of racing spawned
//! workers.

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::{Executor, ServiceContext, new_id};
use timada_dropship::{MockSupplier, Supplier as _, SupplierRegistry};
use timada_order::{Address, OrderStatus, fulfillment_subscription, load_order, place_order};
use timada_payment::{FakePaymentProvider, PaymentProvider};
use timada_tax::{FixedRateVat, TaxCalculator};

struct TestApp {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    ctx: ServiceContext,
    registry: SupplierRegistry,
    provider: Arc<dyn PaymentProvider>,
    tax: Arc<dyn TaxCalculator>,
}

impl TestApp {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-slice-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("demo.db").display());
        let pool = timada_core::db::create_pool(&url, 2).await?;

        let mut migrations = timada_catalog::migrations();
        migrations.extend(timada_order::migrations());
        migrations.extend(timada_payment::migrations());
        migrations.extend(timada_shipping::migrations());
        migrations.extend(timada_dropship::migrations());
        let mut migrator = Migrator::<sqlx::Sqlite>::default();
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
            ctx,
            registry,
            provider: Arc::new(FakePaymentProvider),
            tax: Arc::new(FixedRateVat::new(2000)),
        })
    }

    fn executor(&self) -> &Executor {
        &self.ctx.executor
    }

    /// Import + publish one mock-supplier product, selected by price predicate.
    async fn seed_product(&self, want: impl Fn(i64) -> bool) -> anyhow::Result<String> {
        let supplier = MockSupplier::new();
        let product = supplier
            .search_products("")
            .await
            .map_err(anyhow::Error::from)?
            .into_iter()
            .find(|p| want(p.price.amount_cents))
            .ok_or_else(|| anyhow::anyhow!("no mock product matches the price predicate"))?;
        let id = timada_catalog::import_product(self.executor(), "mock", product).await?;
        timada_catalog::publish_product(self.executor(), &id).await?;
        Ok(id)
    }

    /// Run saga passes until the order status stops changing, then return it.
    async fn settle(&self, order_id: &str) -> anyhow::Result<OrderStatus> {
        let mut saga = fulfillment_subscription(
            self.executor().clone(),
            self.registry.clone(),
            self.provider.clone(),
        )
        .no_retry();

        let mut previous = None;
        for _ in 0..10 {
            saga.run_once(self.executor()).await?;
            let order = load_order(self.executor(), order_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("order disappeared"))?;
            if previous == Some(order.status) {
                return Ok(order.status);
            }
            previous = Some(order.status);
        }
        anyhow::bail!("the saga never settled for order {order_id}")
    }

    async fn checkout(&self, product_id: &str, quantity: u32) -> anyhow::Result<String> {
        let cart_id = new_id();
        let product = timada_catalog::load_product(self.executor(), product_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("product not found"))?;
        timada_cart::add_item(self.executor(), &cart_id, &product, quantity).await?;
        let order_id = place_order(
            self.executor(),
            &self.tax,
            &cart_id,
            "customer@example.com".into(),
            Address {
                full_name: "Ada Lovelace".into(),
                street: "1 Analytical Engine Way".into(),
                city: "London".into(),
                postal_code: "N1 9GU".into(),
                country: "GB".into(),
            },
        )
        .await?;
        Ok(order_id)
    }

    /// Drain every admin read-model subscription once.
    async fn refresh_admin_tables(&self) -> anyhow::Result<()> {
        timada_catalog::read_models_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await?;
        timada_order::admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await?;
        timada_payment::admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await?;
        timada_shipping::admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await?;
        timada_dropship::admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await?;
        Ok(())
    }

    async fn admin_order_status(&self, id: &str) -> anyhow::Result<String> {
        let (status,): (String,) =
            sqlx::query_as("SELECT status FROM admin_order_list WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(status)
    }

    async fn admin_shipment_status(&self, id: &str) -> anyhow::Result<String> {
        let (status,): (String,) =
            sqlx::query_as("SELECT status FROM admin_shipment_list WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(status)
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn the_full_journey_ends_with_a_delivered_order() -> anyhow::Result<()> {
    let app = TestApp::new().await?;

    // Seed a product whose price does NOT trigger the fake decline (…99).
    let product_id = app.seed_product(|cents| cents % 100 != 99).await?;

    // The storefront read model sees the published product.
    app.refresh_admin_tables().await?;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM store_product_list WHERE id = ?")
        .bind(&product_id)
        .fetch_one(&app.pool)
        .await?;
    assert_eq!(count, 1, "published product must appear in the storefront");

    // Cart → checkout → saga: charge captured, forwarded to the supplier.
    let order_id = app.checkout(&product_id, 2).await?;
    assert_eq!(app.settle(&order_id).await?, OrderStatus::Forwarded);

    // First tracking poll dispatches the mock shipment.
    let shipment_id = timada_shipping::shipment_id(&order_id, "mock");
    timada_shipping::refresh_tracking(app.executor(), &app.registry, &shipment_id).await?;
    assert_eq!(app.settle(&order_id).await?, OrderStatus::Shipped);

    // Second poll delivers it.
    timada_shipping::refresh_tracking(app.executor(), &app.registry, &shipment_id).await?;
    assert_eq!(app.settle(&order_id).await?, OrderStatus::Delivered);

    let order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    assert!(
        order.tracking_number.is_some(),
        "tracking number propagated"
    );
    assert!(order.payment_id.is_some(), "payment recorded on the order");

    // Admin read models agree end-to-end.
    app.refresh_admin_tables().await?;
    assert_eq!(app.admin_order_status(&order_id).await?, "delivered");
    assert_eq!(app.admin_shipment_status(&shipment_id).await?, "delivered");
    let (payments,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM admin_payment_list WHERE order_id = ? AND status = 'captured'",
    )
    .bind(&order_id)
    .fetch_one(&app.pool)
    .await?;
    assert_eq!(payments, 1, "captured payment visible in admin");
    let (confirmed,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM admin_supplier_order_list WHERE order_id = ? AND status = 'confirmed'",
    )
    .bind(&order_id)
    .fetch_one(&app.pool)
    .await?;
    assert_eq!(confirmed, 1, "confirmed supplier order visible in admin");

    Ok(())
}

#[tokio::test]
async fn a_declined_charge_cancels_the_order() -> anyhow::Result<()> {
    let app = TestApp::new().await?;

    // The mock catalog deliberately contains one …99 price — the fake
    // provider's decline trigger.
    let product_id = app.seed_product(|cents| cents % 100 == 99).await?;
    let order_id = app.checkout(&product_id, 1).await?;

    assert_eq!(app.settle(&order_id).await?, OrderStatus::Cancelled);
    let order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    let reason = order
        .cancel_reason
        .ok_or_else(|| anyhow::anyhow!("cancelled order must carry a reason"))?;
    assert!(reason.contains("declined"), "reason was: {reason}");

    Ok(())
}
