//! Buying what was sold: from a paid order to a tracking number on the
//! customer's own shipment.

use timada_core::{Address, Money, ShopCurrencies};
use timada_order::{DeliveryChoice, OrderLine, PaymentMode, PlaceOrder, Seller};
use timada_sourcing::{
    Command, ConnectorError, FakeConnector, ListPurchases, ManualConnector, PurchaseMode,
    PurchasePolicy, RegisterSupplier, SourceProduct, SourcingError, SupplierConnectors,
    SupplierItemRef, SupplierOffer, SupplierOrderStatus, count_purchases, enqueue_place,
    list_purchases, migrations, purchase_by_id, purchase_list_subscription, purchases_for_order,
    sourcing_list_subscription, sourcing_order_subscription, supplier_order_id,
    work_purchases_with,
};
use timada_tax::FixedRates;

const SCREEN: &str = "product-aoc-24g4xe";
const MOUSE: &str = "product-cor-m65";
const SCREEN_ITEM: &str = "1005006100001";
const MOUSE_ITEM: &str = "1005006100002";

fn address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "1 rue de l'Entrepôt".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

fn offer(item: &str, cost: i64, available: u32) -> SupplierOffer {
    SupplierOffer {
        item: SupplierItemRef::new(item, None),
        cost: Money::eur(cost),
        shipping: Money::eur(0),
        available,
        title: None,
        url: None,
    }
}

struct Shop {
    executor: evento::Sqlite,
    db: sqlx::SqlitePool,
    connectors: SupplierConnectors,
    supplier: String,
    other: String,
}

impl Shop {
    fn cmd(&self) -> Command<'_, evento::Sqlite> {
        Command::new(&self.executor, self.db.clone())
    }

    /// Pumps everything that has to follow: the lists, the queue, and the
    /// purchasing process manager.
    async fn sync(&self, mode: PurchaseMode) -> anyhow::Result<()> {
        for _ in 0..4 {
            sourcing_list_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
            sourcing_order_subscription()
                .data(self.db.clone())
                .data(mode)
                .run_once(&self.executor)
                .await?;
            purchase_list_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
        }
        Ok(())
    }

    async fn pass(&self) -> anyhow::Result<timada_sourcing::PurchasePass> {
        Ok(work_purchases_with(
            &self.executor,
            &self.db,
            &self.connectors,
            &PurchasePolicy::without_delays(),
        )
        .await?)
    }

    /// An order for the given lines, placed and paid.
    async fn order(&self, lines: &[(&str, u32)], paid: bool) -> anyhow::Result<String> {
        self.order_at(lines, 9_990, paid).await
    }

    /// The same, at a price of the test's choosing — nothing at all, for the
    /// order a voucher covered entirely.
    async fn order_at(
        &self,
        lines: &[(&str, u32)],
        unit_price: i64,
        paid: bool,
    ) -> anyhow::Result<String> {
        let cart = format!("cart-{}", lines.len());
        let order = timada_order::Command(&self.executor)
            .place_order(PlaceOrder {
                cart_id: cart,
                customer_id: "customer-1".into(),
                seller: Seller::default(),
                lines: lines
                    .iter()
                    .map(|(product_id, quantity)| OrderLine {
                        product_id: (*product_id).to_owned(),
                        name: (*product_id).to_owned(),
                        quantity: *quantity,
                        unit_price: Money::eur(unit_price),
                        warranty_months: 24,
                    })
                    .collect(),
                delivery_address: address(),
                billing_address: address(),
                delivery: DeliveryChoice {
                    method_code: "colissimo".into(),
                    pickup_store_id: None,
                },
                payment_mode: PaymentMode::Card,
                shipping_fee: Money::eur(0),
                handling_fee: Money::eur(0),
                promo_code: None,
                discount: None,
                order_number: None,
                tax: None,
                business: None,
                exchange_rate: None,
            })
            .await?;
        // The shop's own parcel, the way the fulfillment saga makes it.
        timada_shipping::Command(&self.executor)
            .create_shipment(timada_shipping::CreateShipment {
                order_id: order.clone(),
                method: timada_shipping::DeliveryMethod::resolve("colissimo", None)
                    .ok_or_else(|| anyhow::anyhow!("no such delivery method"))?,
                destination: address(),
                lines: lines
                    .iter()
                    .map(|(product_id, quantity)| timada_shipping::ShipmentLine {
                        product_id: (*product_id).to_owned(),
                        quantity: *quantity,
                    })
                    .collect(),
            })
            .await?;
        if paid {
            timada_order::Command(&self.executor)
                .mark_paid(&order, "payment-1")
                .await?;
        }
        Ok(order)
    }

    async fn shipment(&self, order_id: &str) -> anyhow::Result<timada_shipping::ShipmentView> {
        timada_shipping::load_shipment(&self.executor, timada_shipping::shipment_id(order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("no shipment"))
    }
}

/// A shop sourcing the screen from one supplier and the mouse from another.
async fn shop(scripted: std::sync::Arc<FakeConnector>) -> anyhow::Result<Shop> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let connectors = SupplierConnectors::default()
        .with(ManualConnector)
        .with(scripted);
    let cmd = Command::new(&executor, db.clone());
    let supplier = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "shenzhen-optics".into(),
                name: "Shenzhen Optics".into(),
                connector: "fake".into(),
                currency: "EUR".into(),
            },
            &connectors,
        )
        .await?;
    let other = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "guangzhou-inputs".into(),
                name: "Guangzhou Inputs".into(),
                connector: "fake".into(),
                currency: "EUR".into(),
            },
            &connectors,
        )
        .await?;
    for (product, item, from) in [
        (SCREEN, SCREEN_ITEM, &supplier),
        (MOUSE, MOUSE_ITEM, &other),
    ] {
        cmd.source_product(SourceProduct {
            product_id: product.into(),
            supplier_id: from.clone(),
            external_item_id: item.into(),
            external_sku: None,
        })
        .await?;
        timada_pricing::Command(&executor)
            .list_price(timada_pricing::ListPrice {
                product_id: product.into(),
                price_incl_tax: Money::eur(9_990),
                vat_rate_bp: 2000,
                eco_participation: Money::eur(0),
            })
            .await?;
    }
    let shop = Shop {
        executor,
        db,
        connectors,
        supplier,
        other,
    };
    shop.sync(PurchaseMode::OnConfirmation).await?;
    Ok(shop)
}

/// What the supplier calls a purchase, once it has taken it.
async fn reference(shop: &Shop, purchase_id: &str) -> anyhow::Result<String> {
    shop.cmd()
        .load_purchase(purchase_id)
        .await?
        .and_then(|state| state.external_order_id)
        .ok_or_else(|| anyhow::anyhow!("no reference"))
}

/// Gives the suppliers' costs, so the drafts are not priced at nothing.
async fn quote(shop: &Shop, scripted: &FakeConnector) -> anyhow::Result<()> {
    scripted.stock(offer(SCREEN_ITEM, 4_000, 10));
    scripted.stock(offer(MOUSE_ITEM, 2_000, 10));
    let rates = FixedRates::new("EUR", "test");
    let base = ShopCurrencies::default();
    for (product, item) in [(SCREEN, SCREEN_ITEM), (MOUSE, MOUSE_ITEM)] {
        shop.cmd()
            .apply_offer(
                product,
                &offer(item, if product == SCREEN { 4_000 } else { 2_000 }, 10),
                &rates,
                base.base(),
                timada_core::time::now_unix_secs()?,
            )
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_paid_order_is_drafted_once_per_supplier() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;

    let order = shop.order(&[(SCREEN, 1), (MOUSE, 2)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;

    // Two suppliers, two purchases — the split is structural, not a case.
    let purchases = purchases_for_order(&shop.db, &order).await?;
    assert_eq!(purchases.len(), 2);
    assert!(
        purchases
            .iter()
            .all(|p| p.status == SupplierOrderStatus::Drafted)
    );
    assert_eq!(
        purchases
            .iter()
            .find(|p| p.supplier_id == shop.supplier)
            .map(|p| (p.units, p.cost.clone())),
        Some((1, Money::eur(4_000)))
    );
    assert_eq!(
        purchases
            .iter()
            .find(|p| p.supplier_id == shop.other)
            .map(|p| (p.units, p.cost.clone())),
        Some((2, Money::eur(4_000)))
    );
    // Nothing was bought: placing spends the shop's money and waits for a click.
    assert!(scripted.placed().is_empty());

    // Pumped again, nothing new is drafted.
    shop.sync(PurchaseMode::OnConfirmation).await?;
    assert_eq!(purchases_for_order(&shop.db, &order).await?.len(), 2);

    // A line the shop holds itself is simply absent.
    let own = shop.order(&[("product-held-here", 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    assert!(purchases_for_order(&shop.db, &own).await?.is_empty());

    Ok(())
}

#[tokio::test]
async fn a_zero_total_order_is_bought_like_any_other() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;

    // A voucher covering the whole total leaves nothing to pay, and the
    // order is settled rather than paid.
    let order = shop.order_at(&[(SCREEN, 1)], 0, false).await?;
    timada_order::Command(&shop.executor)
        .settle_order(&order)
        .await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;

    assert_eq!(purchases_for_order(&shop.db, &order).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_supplier_tracking_dispatches_the_shops_own_parcel() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let purchase = supplier_order_id(&order, &shop.supplier);

    // The operator's click enqueues; the worker does the talking.
    enqueue_place(&shop.db, &purchase).await?;
    let pass = shop.pass().await?;
    assert_eq!(pass.placed, 1);
    assert_eq!(scripted.placed().len(), 1);
    // The purchase's own id is the idempotency key.
    assert_eq!(scripted.placed()[0].0, purchase);
    let state = shop
        .cmd()
        .load_purchase(&purchase)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no purchase"))?;
    assert_eq!(state.status, SupplierOrderStatus::Placed);
    let external = state
        .external_order_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no reference"))?;

    // Still at the supplier: asked about again, not given up on.
    let pass = shop.pass().await?;
    assert_eq!(pass.pending, 1);
    assert_eq!(
        shop.shipment(&order).await?.status,
        timada_shipping::ShipmentStatus::Created
    );

    // It ships, and the shop's own parcel goes with the supplier's tracking.
    scripted.mark_shipped(&external, "4PX", "4PX-99887766");
    let pass = shop.pass().await?;
    assert_eq!(pass.shipped, 1);
    shop.sync(PurchaseMode::OnConfirmation).await?;

    let shipment = shop.shipment(&order).await?;
    assert_eq!(shipment.status, timada_shipping::ShipmentStatus::Dispatched);
    assert_eq!(shipment.tracking_number.as_deref(), Some("4PX-99887766"));
    assert_eq!(shipment.carrier.as_deref(), Some("4PX"));

    // Redelivered, it neither fails nor dispatches twice.
    shop.sync(PurchaseMode::OnConfirmation).await?;
    assert_eq!(
        shop.shipment(&order).await?.status,
        timada_shipping::ShipmentStatus::Dispatched
    );

    Ok(())
}

#[tokio::test]
async fn an_order_split_in_two_waits_for_both_suppliers() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1), (MOUSE, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;

    let first = supplier_order_id(&order, &shop.supplier);
    let second = supplier_order_id(&order, &shop.other);
    for purchase in [&first, &second] {
        enqueue_place(&shop.db, purchase).await?;
    }
    shop.pass().await?;

    // One ships: the customer's parcel waits, because the other is still
    // coming and there is only one shipment per order.
    scripted.mark_shipped(&reference(&shop, &first).await?, "4PX", "4PX-1");
    shop.pass().await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    assert_eq!(
        shop.shipment(&order).await?.status,
        timada_shipping::ShipmentStatus::Created
    );

    // Both have: it goes, under the tracking of the one that finished it.
    scripted.mark_shipped(&reference(&shop, &second).await?, "YunExpress", "YT-2");
    shop.pass().await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let shipment = shop.shipment(&order).await?;
    assert_eq!(shipment.status, timada_shipping::ShipmentStatus::Dispatched);
    assert_eq!(shipment.tracking_number.as_deref(), Some("YT-2"));

    Ok(())
}

#[tokio::test]
async fn a_supplier_that_will_not_take_the_order_leaves_it_to_an_operator() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let purchase = supplier_order_id(&order, &shop.supplier);

    scripted.answer_place(Err(ConnectorError::Refused("out of stock".into())));
    enqueue_place(&shop.db, &purchase).await?;
    let pass = shop.pass().await?;
    assert_eq!(pass.refused, 1);
    shop.sync(PurchaseMode::OnConfirmation).await?;

    let row = purchase_by_id(&shop.db, &purchase)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no purchase"))?;
    assert_eq!(row.status, SupplierOrderStatus::Refused);
    assert_eq!(row.note.as_deref(), Some("out of stock"));
    // The customer's order is untouched: cancelling it is an operator's call,
    // and the existing compensation does the refunding when they make it.
    let order_view = timada_order::load_order_details(&shop.executor, &order)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no order"))?;
    assert_eq!(order_view.status, timada_order::OrderStatus::Paid);

    // Bought on the supplier's site after all: the reference is typed in.
    shop.cmd()
        .record_supplier_order_by_hand(&purchase, "AE-123456".into(), "acheté à la main".into())
        .await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let row = purchase_by_id(&shop.db, &purchase)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no purchase"))?;
    assert_eq!(row.status, SupplierOrderStatus::Placed);
    assert_eq!(row.external_order_id.as_deref(), Some("AE-123456"));

    Ok(())
}

#[tokio::test]
async fn being_throttled_costs_the_purchase_no_attempt() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let purchase = supplier_order_id(&order, &shop.supplier);

    scripted.answer_place(Err(ConnectorError::RateLimited { retry_after: 900 }));
    enqueue_place(&shop.db, &purchase).await?;
    let pass = shop.pass().await?;
    assert_eq!((pass.held, pass.placed, pass.failed), (1, 0, 0));
    // Nothing was bought, and the purchase still waits to be.
    assert!(scripted.placed().is_empty());
    assert_eq!(
        shop.cmd()
            .load_purchase(&purchase)
            .await?
            .map(|state| state.status),
        Some(SupplierOrderStatus::Drafted)
    );

    Ok(())
}

#[tokio::test]
async fn calling_the_order_off_calls_the_purchase_off() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let purchase = supplier_order_id(&order, &shop.supplier);
    enqueue_place(&shop.db, &purchase).await?;
    shop.pass().await?;

    timada_order::Command(&shop.executor)
        .cancel_order(&order, String::from("cancelled by customer"))
        .await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    assert_eq!(
        shop.cmd()
            .load_purchase(&purchase)
            .await?
            .map(|state| state.status),
        Some(SupplierOrderStatus::Cancelled)
    );
    // And the supplier is told, by the worker rather than by the handler.
    shop.pass().await?;
    assert_eq!(scripted.cancelled().len(), 1);

    Ok(())
}

#[tokio::test]
async fn a_parcel_already_gone_is_a_return_not_a_cancellation() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnConfirmation).await?;
    let purchase = supplier_order_id(&order, &shop.supplier);
    enqueue_place(&shop.db, &purchase).await?;
    shop.pass().await?;
    let external = shop
        .cmd()
        .load_purchase(&purchase)
        .await?
        .and_then(|state| state.external_order_id)
        .ok_or_else(|| anyhow::anyhow!("no reference"))?;
    scripted.mark_shipped(&external, "4PX", "4PX-9");
    shop.pass().await?;

    let refused = shop
        .cmd()
        .cancel_supplier_order(&purchase, "trop tard".into())
        .await;
    assert!(matches!(refused, Err(SourcingError::SupplierOrderShipped)));
    Ok(())
}

#[tokio::test]
async fn on_payment_mode_buys_without_waiting_for_a_click() -> anyhow::Result<()> {
    let scripted = std::sync::Arc::new(FakeConnector::new("fake"));
    let shop = shop(scripted.clone()).await?;
    quote(&shop, &scripted).await?;
    let order = shop.order(&[(SCREEN, 1)], true).await?;
    shop.sync(PurchaseMode::OnPayment).await?;

    let pass = shop.pass().await?;
    assert_eq!(pass.placed, 1);
    shop.sync(PurchaseMode::OnPayment).await?;
    assert_eq!(
        count_purchases(&shop.db, &ListPurchases::to_order(50, 0)).await?,
        0,
        "nothing should be left waiting to be ordered"
    );
    let all = list_purchases(
        &shop.db,
        &ListPurchases {
            limit: 50,
            ..ListPurchases::default()
        },
    )
    .await?;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].order_id, order);

    Ok(())
}
