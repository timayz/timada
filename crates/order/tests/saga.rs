//! The fulfillment saga, driven deterministically against a real SQLite
//! database.
//!
//! Every test walks the same route the storefront does — import a product,
//! publish it, add it to a cart, check out — and then drains the saga one pass
//! at a time instead of racing the background task `start_subscriptions` would
//! spawn. Mocking the event store here would prove nothing: what is under test
//! is precisely how four aggregates' events cascade through one subscription.

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_cart::{add_item, remove_item};
use timada_catalog::{import_product, load_product, publish_product};
use timada_core::{Currency, Money, ServiceContext, new_id};
use timada_dropship::{
    MockSupplier, SupplierOrderStatus, SupplierProduct, SupplierRegistry, load_supplier_order,
    supplier_order_id,
};
use timada_order::{
    Address, OrderState, OrderStatus, OrderView, PlaceOrderError, admin_subscription,
    fulfillment_subscription, load_order, migrations, place_order, recent_orders,
};
use timada_payment::{FakePaymentProvider, PaymentStatus, load_payment};
use timada_shipping::{load_shipment, refresh_tracking, shipment_id};
use timada_tax::{FixedRateVat, TaxCalculator};

/// A temp-file database plus the state every test needs, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: OrderState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-order-{}", new_id()));
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
        let customer = timada_customer::CustomerState {
            ctx: ctx.clone(),
            auth: timada_auth::AuthState {
                read_pool: pool.clone(),
                write_pool: pool.clone(),
            },
        };

        Ok(Self {
            dir,
            pool,
            state: OrderState {
                ctx,
                registry,
                provider: Arc::new(FakePaymentProvider),
                // 20 % everywhere, so a 4200-cent line splits 3500 + 700.
                tax: Arc::new(FixedRateVat::new(2000)),
                customer,
            },
        })
    }

    fn executor(&self) -> &timada_core::Executor {
        &self.state.ctx.executor
    }

    fn tax(&self) -> &Arc<dyn TaxCalculator> {
        &self.state.tax
    }

    /// One saga pass: drain whatever is pending right now and return.
    async fn saga_pass(&self) -> anyhow::Result<()> {
        fulfillment_subscription(
            self.executor().clone(),
            self.state.registry.clone(),
            self.state.provider.clone(),
        )
        .no_retry()
        .run_once(self.executor())
        .await
    }

    /// Run the saga until the order stops moving.
    ///
    /// One pass drains the events pending when it starts; the events its own
    /// handlers write are picked up by the next pass, so a cascade needs
    /// several. Two passes that leave the status untouched mean nothing is
    /// pending any more.
    async fn settle(&self, order_id: &str) -> anyhow::Result<OrderView> {
        let mut previous = None;

        for _ in 0..10 {
            self.saga_pass().await?;
            let order = self.order(order_id).await?;

            if previous == Some(order.status) {
                return Ok(order);
            }
            previous = Some(order.status);
        }

        anyhow::bail!("the saga never settled for order {order_id}")
    }

    async fn order(&self, order_id: &str) -> anyhow::Result<OrderView> {
        load_order(self.executor(), order_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no such order: {order_id}"))
    }

    /// Put one freshly imported, published product in a cart.
    ///
    /// The price doubles as the product reference so two products in one test
    /// cannot collide on the catalog's derived id.
    async fn add_to_cart(
        &self,
        cart_id: &str,
        supplier_id: &str,
        price_cents: i64,
    ) -> anyhow::Result<()> {
        let product_id = import_product(
            self.executor(),
            supplier_id,
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
            .ok_or_else(|| anyhow::anyhow!("no such product: {product_id}"))?;
        add_item(self.executor(), cart_id, &product, 1).await?;

        Ok(())
    }

    /// A cart holding one product from `supplier_id` at `price_cents`.
    async fn cart_with(&self, supplier_id: &str, price_cents: i64) -> anyhow::Result<String> {
        let cart_id = new_id();
        self.add_to_cart(&cart_id, supplier_id, price_cents).await?;
        Ok(cart_id)
    }

    fn address(&self) -> Address {
        Address {
            full_name: "Ada Lovelace".to_owned(),
            street: "12 Analytical Way".to_owned(),
            city: "London".to_owned(),
            postal_code: "NW1 4RT".to_owned(),
            country: "United Kingdom".to_owned(),
        }
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn an_order_walks_from_placed_to_delivered() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = db.cart_with("mock", 4200).await?;

    let order_id = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await?;

    // Charging and forwarding cascade on their own: nothing else is driving
    // this but the saga reacting to payment and dropship events.
    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Forwarded);
    assert_eq!(order.total, Money::new(4200, Currency::Eur));

    let payment_id = order
        .payment_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("a paid order must know its payment"))?;
    let payment = load_payment(db.executor(), &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no such payment"))?;
    assert_eq!(payment.status, PaymentStatus::Captured);
    assert_eq!(payment.order_id, order_id);

    let supplier_order = load_supplier_order(db.executor(), &supplier_order_id(&order_id, "mock"))
        .await?
        .ok_or_else(|| anyhow::anyhow!("no such supplier order"))?;
    assert_eq!(supplier_order.status, SupplierOrderStatus::Confirmed);
    assert_eq!(order.supplier_order_ids, vec![supplier_order.id.clone()]);

    // The mock supplier dispatches on the first tracking poll and delivers on
    // the next; each poll is what the admin's "Refresh tracking" button does.
    let shipment_id = shipment_id(&order_id, "mock");
    refresh_tracking(db.executor(), &db.state.registry, &shipment_id).await?;
    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Shipped);

    let shipment = load_shipment(db.executor(), &shipment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no such shipment"))?;
    assert_eq!(
        order.tracking_number, shipment.tracking_number,
        "the order must quote the shipment's own tracking number"
    );
    assert!(
        order
            .tracking_number
            .as_deref()
            .is_some_and(|tracking| tracking.starts_with("TRK-"))
    );

    refresh_tracking(db.executor(), &db.state.registry, &shipment_id).await?;
    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Delivered);
    assert!(order.cancel_reason.is_none());

    // The admin list follows the same events, one row per order.
    admin_subscription(db.pool.clone())
        .no_retry()
        .run_once(db.executor())
        .await?;
    let rows = recent_orders(&db.pool, 100).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, order_id);
    assert_eq!(rows[0].status, "delivered");
    assert_eq!(rows[0].email, "ada@example.com");
    assert_eq!(rows[0].total().amount_cents, 4200);
    assert_eq!(rows[0].tracking_number, order.tracking_number);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn a_declined_charge_cancels_the_order() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    // `FakePaymentProvider` declines totals whose minor units end in 99.
    let cart_id = db.cart_with("mock", 1999).await?;

    let order_id = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await?;

    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert!(
        order
            .cancel_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("declined")),
        "the cancellation must carry the provider's reason, got {:?}",
        order.cancel_reason
    );

    // Nothing was captured, so there is nothing to compensate — and nothing was
    // forwarded to a supplier either.
    assert!(order.payment_id.is_none());
    assert!(order.supplier_order_ids.is_empty());
    assert!(
        load_supplier_order(db.executor(), &supplier_order_id(&order_id, "mock"))
            .await?
            .is_none(),
        "a declined order must never reach a supplier"
    );

    // (The failed `Payment` aggregate is not asserted here: its id is derived
    // inside `timada-payment` and never surfaces on a cancelled order. The
    // provider's own decline message reaching `cancel_reason` is the proof the
    // charge was attempted and refused.)

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn a_supplier_rejection_refunds_and_cancels() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    // "ghost" is not in the registry, which `forward_order` records as a
    // rejection rather than raising — the same path a supplier refusal takes.
    let cart_id = db.cart_with("ghost", 5000).await?;

    let order_id = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await?;

    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert!(
        order
            .cancel_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("ghost")),
        "the cancellation must name the supplier that refused, got {:?}",
        order.cancel_reason
    );

    // Compensation: the charge was captured before the supplier refused, so it
    // has to come back.
    let payment_id = order
        .payment_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the order was paid before the rejection"))?;
    let payment = load_payment(db.executor(), &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no such payment"))?;
    assert_eq!(payment.status, PaymentStatus::Refunded);

    // Replaying the whole saga must not refund twice or cancel twice.
    let settled = db.settle(&order_id).await?;
    assert_eq!(settled, order);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn checkout_snapshots_the_tax_split_of_a_tax_inclusive_total() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = db.cart_with("mock", 4200).await?;

    let order_id = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await?;
    let order = db.order(&order_id).await?;

    // Tax-inclusive: the customer still pays the price they saw.
    assert_eq!(order.total, Money::new(4200, Currency::Eur));
    assert_eq!(order.total_net.add(order.total_tax)?, order.total);
    // 42.00 gross at 20 % inclusive → 35.00 net + 7.00 tax.
    assert_eq!(order.total_tax, Money::new(700, Currency::Eur));
    assert_eq!(order.total_net, Money::new(3500, Currency::Eur));

    assert_eq!(order.lines.len(), 1);
    for line in &order.lines {
        assert_eq!(line.tax_rate_bps, 2000);
        assert_eq!(
            line.net.add(line.tax)?,
            line.unit_price.multiply(line.quantity),
            "line {} must reconcile against its gross",
            line.product_id
        );
    }

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn checkout_refuses_carts_it_cannot_turn_into_an_order() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let unknown = place_order(
        db.executor(),
        db.tax(),
        &new_id(),
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await;
    assert!(matches!(unknown, Err(PlaceOrderError::UnknownCart)));

    // A cart everything was removed from still exists, but is empty.
    let cart_id = db.cart_with("mock", 4200).await?;
    remove_item(
        db.executor(),
        &cart_id,
        &timada_catalog::product_id("mock", "MP-4200"),
    )
    .await?;
    let empty = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await;
    assert!(matches!(empty, Err(PlaceOrderError::EmptyCart)));

    let cart_id = db.cart_with("mock", 3300).await?;
    let bad_email = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "not-an-email".to_owned(),
        db.address(),
    )
    .await;
    assert!(matches!(bad_email, Err(PlaceOrderError::Invalid(_))));

    let blank_city = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        Address {
            city: "  ".to_owned(),
            ..db.address()
        },
    )
    .await;
    assert!(matches!(blank_city, Err(PlaceOrderError::Invalid(_))));

    // The valid checkout closes the cart, so a second one is refused.
    place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await?;
    let again = place_order(
        db.executor(),
        db.tax(),
        &cart_id,
        None,
        "ada@example.com".to_owned(),
        db.address(),
    )
    .await;
    assert!(matches!(again, Err(PlaceOrderError::CartAlreadyCheckedOut)));

    db.close().await;
    Ok(())
}
