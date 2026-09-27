//! The sync pass: what the suppliers are asked, what the shop does with the
//! answers, and what it does when the answers do not come.

use std::time::Duration;

use timada_core::{Money, ShopCurrencies};
use timada_inventory::{StockLocation, load_stock_availability, stock_item_id};
use timada_pricing::ListPrice;
use timada_sourcing::{
    Command, ConnectorError, ConnectorLimits, FakeConnector, ListReviews, ManualConnector,
    PricingRule, RegisterSupplier, ReviewReason, RuleScope, Settled, SourceProduct,
    SupplierConnectors, SupplierItemRef, SupplierOffer, SyncPolicy, count_reviews, list_reviews,
    migrations, poll_status, sourcing_list_subscription, sourcing_poll_subscription,
    sync_offers_with,
};
use timada_tax::{ExchangeRates, FixedRates};

const PRODUCT: &str = "product-aoc-24g4xe";
const OTHER: &str = "product-lg-27gp";
const ITEM: &str = "1005006123456789";

fn rates() -> impl ExchangeRates {
    FixedRates::new("EUR", "test").with("USD", 1_085_000)
}

fn offer(item: &str, cost: i64, available: u32) -> SupplierOffer {
    SupplierOffer {
        item: SupplierItemRef::new(item, None),
        cost: Money::new(cost, "USD"),
        shipping: Money::new(200, "USD"),
        available,
        title: Some("A monitor".into()),
        url: None,
    }
}

struct Shop {
    executor: evento::Sqlite,
    db: sqlx::SqlitePool,
    connectors: SupplierConnectors,
    supplier: String,
}

impl Shop {
    fn cmd(&self) -> Command<'_, evento::Sqlite> {
        Command::new(&self.executor, self.db.clone())
    }

    /// Brings the read model and the queue up to date.
    async fn sync_subscriptions(&self) -> anyhow::Result<()> {
        for _ in 0..3 {
            sourcing_list_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
            sourcing_poll_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
        }
        Ok(())
    }

    async fn pass(&self, rates: &dyn ExchangeRates) -> anyhow::Result<timada_sourcing::SyncPass> {
        Ok(sync_offers_with(
            &self.executor,
            &self.db,
            &self.connectors,
            rates,
            &ShopCurrencies::default(),
            &SyncPolicy::without_delays(),
        )
        .await?)
    }

    /// Makes everything due, then runs a pass — for the tests that are
    /// about what a pass *does* rather than about when it happens. Left to
    /// itself a product waits six hours, which is the point of the queue.
    async fn pass_now(
        &self,
        rates: &dyn ExchangeRates,
    ) -> anyhow::Result<timada_sourcing::SyncPass> {
        sqlx::query("UPDATE sourcing_poll SET next_poll_at = 0")
            .execute(&self.db)
            .await?;
        self.pass(rates).await
    }

    async fn price_of(&self, product_id: &str) -> anyhow::Result<Option<Money>> {
        Ok(
            timada_pricing::load_product_price(
                &self.executor,
                timada_pricing::price_id(product_id),
            )
            .await?
            .map(|price| price.price_incl_tax),
        )
    }

    async fn available(&self, product_id: &str) -> anyhow::Result<u32> {
        Ok(load_stock_availability(
            &self.executor,
            stock_item_id(product_id, &StockLocation::Warehouse),
        )
        .await?
        .map(|stock| stock.available)
        .unwrap_or_default())
    }
}

#[tokio::test]
async fn a_pass_prices_what_it_may_and_asks_about_the_rest() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();

    // $34,50 + $2,00 at 1,0850 → 33,64 € → 64,90 €, a euro up from 63,90.
    scripted.stock(offer(ITEM, 3_450, 12));
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.asked, pass.repriced, pass.queued), (1, 1, 0));
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_490)));
    // Twelve held, one kept back so the shop is never the one taking the last.
    assert_eq!(pass.restocked, 1);
    assert_eq!(shop.available(PRODUCT).await?, 11);
    assert_eq!(count_reviews(&shop.db, &ListReviews::open(50, 0)).await?, 0);

    // Asked again with nothing moved: nothing is written at all.
    let quiet = shop.pass_now(&rates).await?;
    assert_eq!((quiet.asked, quiet.repriced, quiet.restocked), (1, 0, 0));

    // The cost doubles: too far to take by itself.
    scripted.stock(offer(ITEM, 6_900, 12));
    let pass = shop.pass_now(&rates).await?;
    assert_eq!((pass.repriced, pass.queued), (0, 1));
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_490)));
    let open = list_reviews(&shop.db, &ListReviews::open(50, 0)).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason, ReviewReason::Jump);
    assert_eq!(open[0].product_id, PRODUCT);

    // Asked again and again before anybody looks: still one row, refreshed.
    scripted.stock(offer(ITEM, 7_000, 12));
    shop.pass_now(&rates).await?;
    shop.pass_now(&rates).await?;
    let open = list_reviews(&shop.db, &ListReviews::open(50, 0)).await?;
    assert_eq!(open.len(), 1);
    assert!(open[0].proposed.minor > 6_490);

    // The operator agrees: the price moves and the question is settled.
    let proposed = open[0].proposed.clone();
    assert!(shop.cmd().approve_price_change(&open[0].review_id).await?);
    assert_eq!(shop.price_of(PRODUCT).await?, Some(proposed));
    assert_eq!(count_reviews(&shop.db, &ListReviews::open(50, 0)).await?, 0);
    // Answering it twice is harmless.
    assert!(!shop.cmd().approve_price_change(&open[0].review_id).await?);

    Ok(())
}

#[tokio::test]
async fn refusing_a_change_settles_the_matter() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();

    scripted.stock(offer(ITEM, 6_900, 12));
    shop.pass(&rates).await?;
    let open = list_reviews(&shop.db, &ListReviews::open(50, 0)).await?;
    assert_eq!(open.len(), 1);

    assert!(shop.cmd().reject_price_change(&open[0].review_id).await?);
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_390)));
    let settled = list_reviews(
        &shop.db,
        &ListReviews {
            settled: Some(true),
            limit: 50,
            ..ListReviews::default()
        },
    )
    .await?;
    assert_eq!(settled[0].settled_as, Some(Settled::Rejected));

    // And the price is locked, so the same question is not put again in six
    // hours: the queue stays empty however often the cost moves.
    scripted.stock(offer(ITEM, 9_900, 12));
    shop.pass_now(&rates).await?;
    shop.pass_now(&rates).await?;
    assert_eq!(count_reviews(&shop.db, &ListReviews::open(50, 0)).await?, 0);
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_390)));
    // The level still follows: a locked price is not a locked shelf.
    assert_eq!(shop.available(PRODUCT).await?, 11);

    Ok(())
}

#[tokio::test]
async fn a_supplier_that_will_not_answer_is_left_alone_for_a_while() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();
    let now = timada_core::time::now_unix_secs()? as i64;

    // Throttled: the whole supplier waits, and it costs the row no attempt —
    // being told to slow down is not the row's fault.
    scripted.answer_offers(Err(ConnectorError::RateLimited { retry_after: 600 }));
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.held, pass.postponed, pass.failed), (1, 0, 0));
    let status = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?;
    assert_eq!(status.attempts, 0);
    assert!(status.next_poll_at >= now + 600);

    // Unreachable: that does count, and the row backs off.
    timada_sourcing::hurry(&shop.db, PRODUCT, 0).await?;
    scripted.answer_offers(Err(ConnectorError::Unavailable("connection reset".into())));
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.held, pass.postponed), (0, 1));
    let status = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?;
    assert_eq!(status.attempts, 1);
    assert_eq!(
        status.last_error.as_deref(),
        Some("unavailable: connection reset")
    );

    // And an answer puts it right back on its ordinary rhythm.
    timada_sourcing::hurry(&shop.db, PRODUCT, 0).await?;
    scripted.stock(offer(ITEM, 3_450, 12));
    shop.pass(&rates).await?;
    let status = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?;
    assert_eq!(status.attempts, 0);
    assert_eq!(status.last_error, None);
    assert!(status.last_polled_at.is_some());

    Ok(())
}

#[tokio::test]
async fn an_item_the_supplier_stopped_listing_is_said_not_delisted() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();

    // The supplier answers about nothing: the item is gone from its side.
    let pass = shop.pass(&rates).await?;
    assert_eq!(pass.postponed, 1);
    let open = list_reviews(&shop.db, &ListReviews::open(50, 0)).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason, ReviewReason::NoListedPrice);
    // Still sourced, still on sale: one API call is a poor reason to take a
    // product off the shop.
    assert!(
        shop.cmd()
            .load_sourced_product(PRODUCT)
            .await?
            .is_some_and(|sourced| sourced.active)
    );
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_390)));

    // There is no price here to apply, only something to see to.
    let refused = shop.cmd().approve_price_change(&open[0].review_id).await;
    assert!(matches!(
        refused,
        Err(timada_sourcing::SourcingError::NothingToApply)
    ));
    assert!(shop.cmd().dismiss_review(&open[0].review_id).await?);

    Ok(())
}

#[tokio::test]
async fn a_level_too_large_to_believe_is_left_alone() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();

    scripted.stock(offer(ITEM, 3_450, 900_000));
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.failed, pass.restocked, pass.repriced), (1, 0, 0));
    assert_eq!(shop.available(PRODUCT).await?, 0);
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_390)));

    Ok(())
}

#[tokio::test]
async fn a_suspended_supplier_is_not_asked_at_all() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();
    scripted.stock(offer(ITEM, 3_450, 12));

    shop.cmd()
        .suspend_supplier(&shop.supplier, "papers out of date".into())
        .await?;
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.asked, pass.repriced), (0, 0));
    assert!(scripted.asked().is_empty());
    assert_eq!(shop.price_of(PRODUCT).await?, Some(Money::eur(6_390)));

    shop.cmd().resume_supplier(&shop.supplier).await?;
    timada_sourcing::hurry(&shop.db, PRODUCT, 0).await?;
    let pass = shop.pass(&rates).await?;
    assert_eq!((pass.asked, pass.repriced), (1, 1));

    Ok(())
}

#[tokio::test]
async fn one_supplier_is_asked_once_a_pass_in_batches_of_its_own_choosing() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake").with_limits(ConnectorLimits {
        batch: 1,
        min_interval: Duration::ZERO,
    }));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();
    let cmd = shop.cmd();
    cmd.source_product(SourceProduct {
        product_id: OTHER.into(),
        supplier_id: shop.supplier.clone(),
        external_item_id: "1005009999".into(),
        external_sku: None,
    })
    .await?;
    timada_pricing::Command(&shop.executor)
        .list_price(ListPrice {
            product_id: OTHER.into(),
            price_incl_tax: Money::eur(19_990),
            vat_rate_bp: 2000,
            eco_participation: Money::eur(0),
        })
        .await?;
    shop.sync_subscriptions().await?;
    scripted.stock(offer(ITEM, 3_450, 12));
    scripted.stock(offer("1005009999", 9_900, 4));

    let pass = shop.pass(&rates).await?;
    assert_eq!(pass.asked, 2);
    // Two items, a batch of one: two calls, never two suppliers' worth of
    // questions crammed into one.
    assert_eq!(scripted.asked().len(), 2);
    assert!(scripted.asked().iter().all(|batch| batch.len() == 1));
    assert_eq!(shop.available(OTHER).await?, 3);

    Ok(())
}

#[tokio::test]
async fn a_sale_brings_the_next_question_forward() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let rates = rates();
    scripted.stock(offer(ITEM, 3_450, 12));
    shop.pass(&rates).await?;

    let far = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?
        .next_poll_at;

    // Somebody buys one of something the shop does not hold.
    timada_inventory::Command(&shop.executor)
        .reserve_stock(
            stock_item_id(PRODUCT, &StockLocation::Warehouse),
            "order-1",
            1,
        )
        .await?;
    shop.sync_subscriptions().await?;
    let after_sale = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?
        .next_poll_at;
    assert!(after_sale < far, "a sale should shorten the wait");

    // And so does a cancellation, which puts units back on sale that the
    // supplier may no longer hold.
    timada_sourcing::hurry(&shop.db, PRODUCT, far).await?;
    timada_inventory::Command(&shop.executor)
        .release_stock(stock_item_id(PRODUCT, &StockLocation::Warehouse), "order-1")
        .await?;
    shop.sync_subscriptions().await?;
    let after_release = poll_status(&shop.db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not queued"))?
        .next_poll_at;
    assert!(after_release < far, "a cancellation should shorten it too");

    Ok(())
}

#[tokio::test]
async fn giving_up_a_supplier_takes_its_products_out_of_the_queue() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = scripted_shop(scripted.clone()).await?;
    let sourced = timada_sourcing::sourced_product_id(PRODUCT);

    shop.cmd()
        .stop_sourcing(&sourced, "bought elsewhere now".into())
        .await?;
    shop.sync_subscriptions().await?;
    assert!(poll_status(&shop.db, PRODUCT).await?.is_none());

    Ok(())
}

/// The registry takes a connector by value, but a test wants to go on
/// scripting the one it built: this hands out a shared handle instead.
struct ScriptedConnector(std::sync::Arc<FakeConnector>);

impl timada_sourcing::SupplierConnector for ScriptedConnector {
    fn key(&self) -> &str {
        self.0.key()
    }

    fn does(&self, task: timada_sourcing::ConnectorTask) -> bool {
        self.0.does(task)
    }

    fn limits(&self) -> ConnectorLimits {
        self.0.limits()
    }

    fn offers<'a>(
        &'a self,
        items: &'a [SupplierItemRef],
    ) -> timada_sourcing::connector::ConnectorFuture<'a, Vec<SupplierOffer>> {
        self.0.offers(items)
    }

    fn place<'a>(
        &'a self,
        order: &'a timada_sourcing::PlaceOrder<'a>,
    ) -> timada_sourcing::connector::ConnectorFuture<'a, timada_sourcing::PlacedOrder> {
        self.0.place(order)
    }

    fn standing<'a>(
        &'a self,
        external_order_id: &'a str,
    ) -> timada_sourcing::connector::ConnectorFuture<'a, timada_sourcing::SupplierOrderStanding>
    {
        self.0.standing(external_order_id)
    }

    fn cancel<'a>(
        &'a self,
        external_order_id: &'a str,
    ) -> timada_sourcing::connector::ConnectorFuture<'a, ()> {
        self.0.cancel(external_order_id)
    }
}

/// A shop whose supplier is the connector the test holds on to.
async fn scripted_shop(scripted: std::sync::Arc<FakeConnector>) -> anyhow::Result<Shop> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let connectors = SupplierConnectors::default()
        .with(ManualConnector)
        .with(ScriptedConnector(scripted));
    let cmd = Command::new(&executor, db.clone());
    let supplier = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "shenzhen-optics".into(),
                name: "Shenzhen Optics".into(),
                connector: "fake".into(),
                currency: "USD".into(),
            },
            &connectors,
        )
        .await?;
    cmd.source_product(SourceProduct {
        product_id: PRODUCT.into(),
        supplier_id: supplier.clone(),
        external_item_id: ITEM.into(),
        external_sku: None,
    })
    .await?;
    timada_pricing::Command(&executor)
        .list_price(ListPrice {
            product_id: PRODUCT.into(),
            price_incl_tax: Money::eur(6_390),
            vat_rate_bp: 2000,
            eco_participation: Money::eur(0),
        })
        .await?;
    timada_sourcing::save_rule(
        &db,
        &RuleScope::Default,
        &PricingRule {
            markup_bp: 6_000,
            min_margin_bp: 4_000,
            safety_stock: 1,
            ..PricingRule::default()
        },
    )
    .await?;
    let shop = Shop {
        executor,
        db,
        connectors,
        supplier,
    };
    shop.sync_subscriptions().await?;
    Ok(shop)
}
