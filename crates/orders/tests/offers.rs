mod common;

use common::*;
use timada_discounts::{CreateDiscount, CreateOffer, DiscountValue};
use timada_orders::{Checkout, OrderError};

type Discounts = timada_discounts::Command<evento::Sqlite>;

/// A tracker at 30.00 €, a bowl at 20.00 €, and an offer of one
/// tracker at 12.00 € shaped by `offer`.
async fn shop(
    offer: impl FnOnce(&str, &str) -> CreateOffer,
) -> (Orders, Discounts, String, String, String) {
    let (orders, catalog) = store().await;
    let discounts = timada_discounts::Command::new(orders.executor().clone(), TENANT);
    let tracker = product(&catalog, "Tracker", 3000, "EUR").await;
    let bowl = product(&catalog, "Bowl", 2000, "EUR").await;
    let offer_id = discounts
        .create_offer(ALICE, offer(&tracker, &bowl))
        .await
        .unwrap();
    (orders, discounts, tracker, bowl, offer_id)
}

fn tracker_offer(tracker: &str) -> CreateOffer {
    CreateOffer {
        product_id: tracker.to_owned(),
        price_cents: 1200,
        currency: "EUR".to_owned(),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_offer_is_suggested_then_prices_one_unit() {
    let (orders, _, tracker, bowl, offer_id) = shop(|tracker, _| CreateOffer {
        title: Some("A tracker at 60 % off".into()),
        ..tracker_offer(tracker)
    })
    .await;
    let cart_id = cart_with(&orders, &bowl, 1).await;

    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.total_cents, 2000);
    let [suggested] = quote.offers.as_slice() else {
        panic!("one offer: {:?}", quote.offers);
    };
    assert_eq!(suggested.offer_id, offer_id);
    assert_eq!(suggested.name, "Tracker");
    assert_eq!(suggested.title.as_deref(), Some("A tracker at 60 % off"));
    assert_eq!(
        (suggested.unit_amount_cents, suggested.offer_amount_cents),
        (3000, 1200)
    );

    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 1))
        .await
        .unwrap();
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert!(quote.offers.is_empty());
    assert_eq!(quote.subtotal_cents, 3200);
    assert_eq!(quote.total_cents, 3200);
    let tracker_line = &quote.lines[1];
    assert_eq!(
        (tracker_line.unit_amount_cents, tracker_line.quantity),
        (1200, 1)
    );
    let line_offer = quote.line_offers[1].as_ref().unwrap();
    assert_eq!(line_offer.offer_id, offer_id);
    assert_eq!(line_offer.full_unit_amount_cents, 3000);
    assert_eq!(quote.line_offers[0], None);
}

#[tokio::test]
async fn extra_units_pay_full_price() {
    let (orders, _, tracker, bowl, _) = shop(|tracker, _| tracker_offer(tracker)).await;
    let cart_id = cart_with(&orders, &bowl, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 3))
        .await
        .unwrap();

    let quote = orders.quote_cart(&cart_id).await.unwrap();
    let lines: Vec<(u64, u32)> = quote
        .lines
        .iter()
        .map(|line| (line.unit_amount_cents, line.quantity))
        .collect();
    assert_eq!(lines, vec![(2000, 1), (1200, 1), (3000, 2)]);
    assert!(
        quote
            .lines
            .iter()
            .all(|line| line.tax_category() == "standard")
    );
    assert_eq!(quote.line_offers.len(), 3);
    assert!(quote.line_offers[1].is_some() && quote.line_offers[2].is_none());
    assert_eq!(quote.item_count(), 4);
    assert_eq!(quote.subtotal_cents, 2000 + 1200 + 6000);
}

#[tokio::test]
async fn the_price_is_dropped_when_a_condition_breaks() {
    let (orders, _, tracker, bowl, _) = shop(|tracker, bowl| CreateOffer {
        requires_product_id: Some(bowl.to_owned()),
        min_subtotal_cents: Some(3000),
        ..tracker_offer(tracker)
    })
    .await;
    // One bowl is not enough.
    let cart_id = cart_with(&orders, &bowl, 1).await;
    assert!(orders.quote_cart(&cart_id).await.unwrap().offers.is_empty());
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 1))
        .await
        .unwrap();
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 5000);

    // Two bowls are.
    orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&bowl, None, 2))
        .await
        .unwrap();
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 5200);

    // Without any bowl, the tracker is full price again, even two of them
    // (which are worth the minimum on their own).
    orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&bowl, None, 0))
        .await
        .unwrap();
    orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&tracker, None, 2))
        .await
        .unwrap();
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.total_cents, 6000);
    assert!(quote.line_offers.iter().all(Option::is_none));
}

#[tokio::test]
async fn the_window_and_the_status_are_respected() {
    let now = timada_discounts::unix_now();
    let (orders, discounts, tracker, bowl, offer_id) = shop(|tracker, _| CreateOffer {
        starts_at: Some(now + 3600),
        ..tracker_offer(tracker)
    })
    .await;
    let cart_id = cart_with(&orders, &bowl, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 1))
        .await
        .unwrap();
    // Not started yet.
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 5000);

    // Over already.
    discounts.archive_offer(ALICE, &offer_id).await.unwrap();
    let over = discounts
        .create_offer(
            ALICE,
            CreateOffer {
                starts_at: Some(now - 7200),
                ends_at: Some(now - 3600),
                ..tracker_offer(&tracker)
            },
        )
        .await
        .unwrap();
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 5000);

    // Running, then deactivated.
    discounts.archive_offer(ALICE, &over).await.unwrap();
    let running = discounts
        .create_offer(
            ALICE,
            CreateOffer {
                starts_at: Some(now - 3600),
                ends_at: Some(now + 3600),
                ..tracker_offer(&tracker)
            },
        )
        .await
        .unwrap();
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 3200);
    discounts.deactivate_offer(ALICE, &running).await.unwrap();
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 5000);
}

#[tokio::test]
async fn the_order_holds_the_offer_line_and_its_total_matches() {
    let (orders, discounts, tracker, bowl, _) = shop(|tracker, _| tracker_offer(tracker)).await;
    discounts
        .create_discount(
            ALICE,
            CreateDiscount {
                code: "TEN".into(),
                value: DiscountValue::Percent(1000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let cart_id = cart_with(&orders, &bowl, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 2))
        .await
        .unwrap();
    orders
        .apply_discount(SHOPPER, &cart_id, "TEN")
        .await
        .unwrap();

    // 20.00 + 12.00 + 30.00, less 10 %.
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.subtotal_cents, 6200);
    assert_eq!(quote.discount.as_ref().unwrap().amount_cents, 620);
    assert_eq!(quote.total_cents, 5580);

    let placed = pay(
        &orders,
        &cart_id,
        Checkout {
            expected_total_cents: Some(5580),
            ..checkout()
        },
    )
    .await;
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.lines.len(), 3);
    assert_eq!(order.subtotal_cents(), 6200);
    assert_eq!(order.discount.as_ref().unwrap().subtotal_cents, 6200);
    assert_eq!(order.total_cents, 5580);
    assert_eq!(order.payment.amount_cents, 5580);
}

#[tokio::test]
async fn a_total_shown_before_the_offer_ended_is_refused() {
    let (orders, discounts, tracker, bowl, offer_id) =
        shop(|tracker, _| tracker_offer(tracker)).await;
    let cart_id = cart_with(&orders, &bowl, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&tracker, None, 1))
        .await
        .unwrap();
    let request_id = orders
        .request_checkout(
            SHOPPER,
            &cart_id,
            Checkout {
                expected_total_cents: Some(3200),
                ..checkout()
            },
        )
        .await
        .unwrap();
    discounts.deactivate_offer(ALICE, &offer_id).await.unwrap();
    let pending = orders.pending_payment(&cart_id, &request_id).await;
    assert!(matches!(
        pending,
        Err(OrderError::PriceChanged { total_cents: 5000 })
    ));
}
