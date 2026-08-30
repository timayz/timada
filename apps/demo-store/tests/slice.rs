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
use timada_invoice::{InvoiceConfig, invoice_id, load_invoice};
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

        let mut migrations = timada_auth::migrations();
        migrations.extend(timada_customer::migrations());
        migrations.extend(timada_catalog::migrations());
        migrations.extend(timada_promotion::migrations());
        migrations.extend(timada_order::migrations());
        migrations.extend(timada_invoice::migrations());
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

    async fn checkout(
        &self,
        product_id: &str,
        quantity: u32,
        customer_id: Option<String>,
    ) -> anyhow::Result<String> {
        let cart_id = new_id();
        let product = timada_catalog::load_product(self.executor(), product_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("product not found"))?;
        timada_cart::add_item(
            self.executor(),
            &cart_id,
            &product,
            product.base_price(),
            quantity,
        )
        .await?;
        let order_id = place_order(
            self.executor(),
            &self.tax,
            &self.pool,
            &cart_id,
            customer_id,
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

    /// Drain the invoice-issuance subscription once (invoices are issued by
    /// reacting to order events, exactly as in the served app).
    async fn issue_invoices(&self) -> anyhow::Result<()> {
        timada_invoice::issuance_subscription(
            self.executor().clone(),
            self.pool.clone(),
            InvoiceConfig::default(),
        )
        .no_retry()
        .run_once(self.executor())
        .await
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
        timada_invoice::admin_subscription(self.pool.clone())
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
    let order_id = app.checkout(&product_id, 2, None).await?;
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

    // The paid order was invoiced, with a reconciling VAT breakdown.
    app.issue_invoices().await?;
    let invoice = load_invoice(app.executor(), &invoice_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("paid order must have an invoice"))?;
    assert_eq!(invoice.invoice_number, "INV-000001");
    assert_eq!(invoice.order_id, order_id);
    assert_eq!(invoice.buyer.email, "customer@example.com");
    assert_eq!(
        invoice.total_net.add(invoice.total_tax)?,
        invoice.total_gross,
        "net + VAT must equal the charged gross"
    );
    assert_eq!(invoice.total_gross, order.total);
    assert!(!invoice.is_credited());

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
async fn a_signed_in_customers_order_lands_in_their_history() -> anyhow::Result<()> {
    let app = TestApp::new().await?;

    let customer_id = timada_customer::register_customer(
        app.executor(),
        &app.pool,
        "jane@example.com",
        "Jane Doe",
        "long-enough-pass",
    )
    .await?;

    let product_id = app.seed_product(|cents| cents % 100 != 99).await?;
    let order_id = app
        .checkout(&product_id, 1, Some(customer_id.clone()))
        .await?;
    assert_eq!(app.settle(&order_id).await?, OrderStatus::Forwarded);

    let order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    assert_eq!(order.customer_id.as_deref(), Some(customer_id.as_str()));

    // The account history (an admin_order_list query) shows exactly this order.
    app.refresh_admin_tables().await?;
    let history = timada_order::orders_for_customer(&app.pool, &customer_id, 10).await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, order_id);

    // A guest order never leaks into anyone's history.
    let guest_order = app.checkout(&product_id, 1, None).await?;
    app.settle(&guest_order).await?;
    app.refresh_admin_tables().await?;
    let history = timada_order::orders_for_customer(&app.pool, &customer_id, 10).await?;
    assert_eq!(history.len(), 1, "guest orders belong to nobody");

    Ok(())
}

#[tokio::test]
async fn a_discount_flows_from_cart_through_checkout_to_the_invoice() -> anyhow::Result<()> {
    let app = TestApp::new().await?;
    let product_id = app.seed_product(|cents| cents % 100 != 99).await?;

    // A 10 % code with a single redemption slot: enough to prove the flow and
    // the exhaustion in one journey.
    timada_promotion::create_discount(
        app.executor(),
        "WELCOME10",
        timada_promotion::DiscountKind::Percentage { bps: 1000 },
        timada_core::now_millis() - 1000,
        None,
        Some(1),
    )
    .await?;

    // Cart with the code applied (case-insensitively), then checkout.
    let cart_id = new_id();
    let product = timada_catalog::load_product(app.executor(), &product_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("product not found"))?;
    timada_cart::add_item(app.executor(), &cart_id, &product, product.base_price(), 2).await?;
    timada_cart::apply_discount(app.executor(), &cart_id, "welcome10").await?;

    let gross = product.base_price().multiply(2);
    let expected_discount = timada_core::Money::new(gross.amount_cents / 10, gross.currency);
    let order_id = place_order(
        app.executor(),
        &app.tax,
        &app.pool,
        &cart_id,
        None,
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

    let order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    assert_eq!(order.discount_code.as_deref(), Some("WELCOME10"));
    assert_eq!(order.discount_amount, Some(expected_discount));
    assert_eq!(
        order.total,
        gross.subtract(expected_discount)?,
        "the charged total is the discounted one"
    );
    let allocated: i64 = order.lines.iter().map(|l| l.discount.amount_cents).sum();
    assert_eq!(
        allocated, expected_discount.amount_cents,
        "allocation is exact"
    );

    // The payment charges the discounted amount, and the invoice prints it.
    assert_eq!(app.settle(&order_id).await?, OrderStatus::Forwarded);
    let paid_order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    let payment = timada_payment::load_payment(
        app.executor(),
        paid_order
            .payment_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("paid order must know its payment"))?,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("payment not found"))?;
    assert_eq!(payment.amount, order.total);

    app.issue_invoices().await?;
    let invoice = load_invoice(app.executor(), &invoice_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("paid order must have an invoice"))?;
    assert_eq!(invoice.discount_code.as_deref(), Some("WELCOME10"));
    assert_eq!(invoice.discount_amount, Some(expected_discount));
    assert_eq!(invoice.total_gross, order.total);
    assert_eq!(
        invoice.total_net.add(invoice.total_tax)?,
        invoice.total_gross
    );

    // The single redemption slot is spent: a second checkout with the same
    // code refuses with a readable reason.
    let cart_id = new_id();
    timada_cart::add_item(app.executor(), &cart_id, &product, product.base_price(), 1).await?;
    timada_cart::apply_discount(app.executor(), &cart_id, "WELCOME10").await?;
    let refused = place_order(
        app.executor(),
        &app.tax,
        &app.pool,
        &cart_id,
        None,
        "customer@example.com".into(),
        Address {
            full_name: "Ada Lovelace".into(),
            street: "1 Analytical Engine Way".into(),
            city: "London".into(),
            postal_code: "N1 9GU".into(),
            country: "GB".into(),
        },
    )
    .await;
    match refused {
        Err(timada_order::PlaceOrderError::Invalid(reason)) => {
            assert!(reason.contains("fully used"), "reason was: {reason}");
        }
        other => anyhow::bail!("expected a fully-used refusal, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn a_declined_charge_cancels_the_order() -> anyhow::Result<()> {
    let app = TestApp::new().await?;

    // The mock catalog deliberately contains one …99 price — the fake
    // provider's decline trigger.
    let product_id = app.seed_product(|cents| cents % 100 == 99).await?;
    let order_id = app.checkout(&product_id, 1, None).await?;

    assert_eq!(app.settle(&order_id).await?, OrderStatus::Cancelled);
    let order = load_order(app.executor(), &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not found"))?;
    let reason = order
        .cancel_reason
        .ok_or_else(|| anyhow::anyhow!("cancelled order must carry a reason"))?;
    assert!(reason.contains("declined"), "reason was: {reason}");

    // Never paid → never invoiced.
    app.issue_invoices().await?;
    assert!(
        load_invoice(app.executor(), &invoice_id(&order_id))
            .await?
            .is_none(),
        "an order that never got paid must not be invoiced"
    );

    Ok(())
}
