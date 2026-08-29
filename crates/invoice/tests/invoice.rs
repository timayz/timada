//! Invoicing, driven end to end against a real SQLite database.
//!
//! Every test walks the storefront's own route — import a product, publish it,
//! add it to a cart, check out — then drains the order context's fulfillment
//! saga one pass at a time and finally runs this crate's issuance subscription.
//! Nothing here fakes an `OrderPaid`: what is under test is precisely that an
//! invoice appears because an order was paid, and that a credit note appears
//! because a paid order was later cancelled.

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_catalog::{import_product, load_product, publish_product};
use timada_core::{Currency, Money, ServiceContext, new_id};
use timada_dropship::{MockSupplier, SupplierProduct, SupplierRegistry};
use timada_invoice::{
    InvoiceConfig, InvoiceState, InvoiceView, Party, admin_subscription, invoice_id,
    issuance_subscription, load_invoice, recent_invoices,
};
use timada_order::{
    Address, OrderState, OrderStatus, OrderView, fulfillment_subscription, load_order, place_order,
};
use timada_payment::FakePaymentProvider;
use timada_tax::FixedRateVat;

/// A temp-file database plus both contexts' state, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    order: OrderState,
    invoice: InvoiceState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-invoice-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        // Both contexts' read-side tables: the invoice list and counter, and
        // the order list the fulfillment saga's own admin model needs.
        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(timada_invoice::migrations())?;
        migrator.add_migrations(timada_order::migrations())?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        let ctx = ServiceContext::new(pool.clone(), pool.clone()).await?;
        let registry = SupplierRegistry::builder()
            .register(Arc::new(MockSupplier::new()))
            .build();

        Ok(Self {
            dir,
            pool: pool.clone(),
            order: OrderState {
                ctx: ctx.clone(),
                registry,
                provider: Arc::new(FakePaymentProvider),
                // 20 % everywhere, so a 4200-cent line splits 3500 + 700.
                tax: Arc::new(FixedRateVat::new(2000)),
                customer: timada_customer::CustomerState {
                    ctx: ctx.clone(),
                    auth: timada_auth::AuthState {
                        read_pool: pool.clone(),
                        write_pool: pool.clone(),
                    },
                },
            },
            invoice: InvoiceState {
                ctx,
                config: InvoiceConfig::new(Party {
                    name: "Timada SAS".to_owned(),
                    street: "1 Rue du Commerce".to_owned(),
                    city: "Paris".to_owned(),
                    postal_code: "75001".to_owned(),
                    country: "France".to_owned(),
                    email: String::new(),
                }),
            },
        })
    }

    fn executor(&self) -> &timada_core::Executor {
        &self.order.ctx.executor
    }

    /// Run the order context's saga until the order stops moving.
    ///
    /// One pass drains the events pending when it starts; the events its own
    /// handlers write are picked up by the next pass, so a cascade needs
    /// several. Two passes that leave the status untouched mean nothing is
    /// pending any more.
    async fn settle(&self, order_id: &str) -> anyhow::Result<OrderView> {
        let mut previous = None;

        for _ in 0..10 {
            fulfillment_subscription(
                self.executor().clone(),
                self.order.registry.clone(),
                self.order.provider.clone(),
            )
            .no_retry()
            .run_once(self.executor())
            .await?;

            let order = self.order(order_id).await?;
            if previous == Some(order.status) {
                return Ok(order);
            }
            previous = Some(order.status);
        }

        anyhow::bail!("the saga never settled for order {order_id}")
    }

    /// One issuance pass: drain whatever order events are pending and return.
    async fn issue(&self) -> anyhow::Result<()> {
        issuance_subscription(
            self.executor().clone(),
            self.pool.clone(),
            self.invoice.config.clone(),
        )
        .no_retry()
        .run_once(self.executor())
        .await
    }

    /// One admin read-model pass.
    async fn project_admin(&self) -> anyhow::Result<()> {
        admin_subscription(self.pool.clone())
            .no_retry()
            .run_once(self.executor())
            .await
    }

    async fn order(&self, order_id: &str) -> anyhow::Result<OrderView> {
        load_order(self.executor(), order_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no such order: {order_id}"))
    }

    async fn invoice_of(&self, order_id: &str) -> anyhow::Result<Option<InvoiceView>> {
        load_invoice(self.executor(), &invoice_id(order_id)).await
    }

    /// Place an order for one freshly imported, published product.
    ///
    /// The price doubles as the product reference so two products in one test
    /// cannot collide on the catalog's derived id.
    async fn place(&self, supplier_id: &str, price_cents: i64) -> anyhow::Result<String> {
        let cart_id = new_id();
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
        timada_cart::add_item(self.executor(), &cart_id, &product, 1).await?;

        let order_id = place_order(
            self.executor(),
            &self.order.tax,
            &cart_id,
            None,
            "ada@example.com".to_owned(),
            Address {
                full_name: "Ada Lovelace".to_owned(),
                street: "12 Analytical Way".to_owned(),
                city: "London".to_owned(),
                postal_code: "NW1 4RT".to_owned(),
                country: "United Kingdom".to_owned(),
            },
        )
        .await?;

        Ok(order_id)
    }

    /// The counter's current position, or `None` if nothing was ever allocated.
    async fn sequence(&self, kind: &str) -> anyhow::Result<Option<i64>> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT next_value FROM invoice_sequences WHERE kind = ?")
                .bind(kind)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(next,)| next))
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn a_paid_order_is_invoiced_once_and_numbers_run_in_sequence() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let order_id = db.place("mock", 4200).await?;

    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Forwarded);

    db.issue().await?;

    let invoice = db
        .invoice_of(&order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("a paid order must be invoiced"))?;

    assert_eq!(invoice.invoice_number, "INV-000001");
    assert_eq!(invoice.order_id, order_id);
    assert_eq!(invoice.seller.name, "Timada SAS");
    assert_eq!(invoice.buyer.name, "Ada Lovelace");
    assert_eq!(invoice.buyer.email, "ada@example.com");
    assert_eq!(invoice.buyer.country, "United Kingdom");
    assert!(invoice.credit_note_number.is_none());

    // The invoice quotes the order, cent for cent — it is billing what was
    // actually charged, not recomputing it.
    assert_eq!(invoice.total_gross, order.total);
    assert_eq!(invoice.total_net, order.total_net);
    assert_eq!(invoice.total_tax, order.total_tax);
    assert_eq!(
        invoice.total_net.add(invoice.total_tax)?,
        invoice.total_gross
    );

    assert_eq!(invoice.lines.len(), order.lines.len());
    for line in &invoice.lines {
        assert_eq!(line.description, "Aurora Desk Lamp");
        assert_eq!(line.tax_rate_bps, 2000);
        assert_eq!(line.gross, line.unit_price_gross.multiply(line.quantity));
        assert_eq!(
            line.net.add(line.tax)?,
            line.gross,
            "line `{}` must reconcile against its gross",
            line.description
        );
    }

    // Replaying the whole subscription must not issue a second invoice, and
    // must not burn a number either.
    db.issue().await?;
    let replayed = db
        .invoice_of(&order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the invoice must survive a replay"))?;
    assert_eq!(replayed, invoice);
    assert_eq!(db.sequence("invoice").await?, Some(2));

    // A second order takes the next number, not the same one.
    let second_order_id = db.place("mock", 3300).await?;
    db.settle(&second_order_id).await?;
    db.issue().await?;

    let second = db
        .invoice_of(&second_order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the second order must be invoiced too"))?;
    assert_eq!(second.invoice_number, "INV-000002");

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn an_order_cancelled_after_payment_is_reversed_by_a_credit_note() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    // "ghost" is not in the registry, which the dropship context records as a
    // rejection — so the charge is captured and then refunded, and the order is
    // cancelled *after* it was paid. That is the refund path.
    let order_id = db.place("ghost", 5000).await?;

    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert!(
        order.payment_id.is_some(),
        "the order must have been paid before the supplier refused it"
    );

    // One pass sees `OrderPaid` and then `OrderCancelled`, in that order —
    // which is why both handlers live on the same subscription.
    db.issue().await?;

    let invoice = db
        .invoice_of(&order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the order was paid, so it must have been invoiced"))?;

    assert_eq!(invoice.invoice_number, "INV-000001");
    assert_eq!(
        invoice.credit_note_number.as_deref(),
        Some("CN-000001"),
        "a paid order that was cancelled must be credited"
    );
    assert_eq!(
        invoice.credit_note_reason.as_deref(),
        order.cancel_reason.as_deref(),
        "the credit note must carry the cancellation's own reason"
    );
    // The invoice itself is untouched: a credit note reverses, it does not edit.
    assert_eq!(invoice.total_gross, order.total);

    // Replaying must not issue a second credit note.
    db.issue().await?;
    let replayed = db
        .invoice_of(&order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the invoice must survive a replay"))?;
    assert_eq!(replayed, invoice);
    assert_eq!(db.sequence("credit_note").await?, Some(2));

    // The admin list follows the same two events, one row per invoice.
    db.project_admin().await?;
    let rows = recent_invoices(&db.pool, 100).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, invoice_id(&order_id));
    assert_eq!(rows[0].order_id, order_id);
    assert_eq!(rows[0].number, "INV-000001");
    assert_eq!(rows[0].status, "credited");
    assert!(rows[0].is_credited());
    assert_eq!(rows[0].credit_note_number.as_deref(), Some("CN-000001"));
    assert_eq!(rows[0].total(), order.total);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn an_order_that_never_paid_is_never_invoiced() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    // `FakePaymentProvider` declines totals whose minor units end in 99, so
    // this order is cancelled without ever being paid.
    let order_id = db.place("mock", 1999).await?;

    let order = db.settle(&order_id).await?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert!(order.payment_id.is_none());

    // The cancellation still reaches the issuance subscription; it just has
    // nothing to credit.
    db.issue().await?;

    assert!(
        db.invoice_of(&order_id).await?.is_none(),
        "an order that was never paid must have no invoice"
    );
    assert_eq!(
        db.sequence("invoice").await?,
        None,
        "a cancelled-before-payment order must not burn an invoice number"
    );
    assert_eq!(db.sequence("credit_note").await?, None);

    db.project_admin().await?;
    assert!(recent_invoices(&db.pool, 100).await?.is_empty());

    db.close().await;
    Ok(())
}
