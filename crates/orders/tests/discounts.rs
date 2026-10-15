mod common;

use common::*;
use timada_discounts::{CreateDiscount, DiscountValue};
use timada_orders::{
    OrdersError, discount_usage_subscription, order_by_id, read_model_subscription,
};

type Discounts = timada_discounts::Command<evento::Sqlite>;

async fn discount(discounts: &Discounts, create: CreateDiscount) -> String {
    discounts.create_discount(ALICE, create).await.unwrap().id
}

fn percent(code: &str, bps: u32) -> CreateDiscount {
    CreateDiscount {
        code: code.into(),
        value: DiscountValue::Percent(bps),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_code_on_the_cart_takes_its_share_off_the_quote() {
    let (orders, catalog) = store().await;
    let discounts = timada_discounts::Command(orders.0.clone());
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    discount(&discounts, percent("TEN", 1000)).await;
    let cart_id = cart_with(&orders, &mug, 2).await;

    let unknown = orders.apply_discount(SHOPPER, &cart_id, "NOPE").await;
    assert!(matches!(
        unknown,
        Err(OrdersError::DiscountRefused {
            code: "discount_unknown"
        })
    ));
    let cart = orders
        .apply_discount(SHOPPER, &cart_id, " ten ")
        .await
        .unwrap();
    assert_eq!(cart.discount_code.as_deref(), Some("TEN"));

    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.subtotal_cents, 2500);
    let off = quote.discount.as_ref().unwrap();
    assert_eq!((off.code.as_str(), off.amount_cents), ("TEN", 250));
    assert_eq!(quote.total_cents, 2250);

    let cart = orders.remove_discount(SHOPPER, &cart_id).await.unwrap();
    assert_eq!(cart.discount_code, None);
    assert_eq!(orders.quote_cart(&cart_id).await.unwrap().total_cents, 2500);
}

#[tokio::test]
async fn a_discounted_payment_makes_a_discounted_order_and_spends_the_use() {
    let (executor, pool) = executor_with_pool().await;
    let orders = timada_orders::Command(executor.clone());
    let catalog = timada_catalog::Command(executor.clone());
    let discounts = timada_discounts::Command(executor.clone());
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let id = discount(
        &discounts,
        CreateDiscount {
            max_uses: Some(5),
            ..percent("TEN", 1000)
        },
    )
    .await;
    let cart_id = cart_with(&orders, &mug, 2).await;
    orders
        .apply_discount(SHOPPER, &cart_id, "TEN")
        .await
        .unwrap();

    let (request_id, payment) = open_payment(
        &orders,
        &cart_id,
        timada_orders::Checkout {
            expected_total_cents: Some(2250),
            ..checkout()
        },
    )
    .await;
    assert_eq!(payment.amount_cents, 2250);
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    let snapshot = cart
        .request(&request_id)
        .unwrap()
        .payment()
        .unwrap()
        .clone();
    assert_eq!(snapshot.total_cents, 2250);
    let granted = snapshot.discount.unwrap();
    assert_eq!((granted.subtotal_cents, granted.amount_cents), (2500, 250));

    let placed = orders
        .place_paid_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap();
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 2250);
    assert_eq!(order.subtotal_cents(), 2500);
    let off = order.discount.unwrap();
    assert_eq!(
        (off.discount_id.as_str(), off.amount_cents),
        (id.as_str(), 250)
    );

    read_model_subscription::<evento::Sqlite>(pool.clone(), "acme")
        .run_once(&executor)
        .await
        .unwrap();
    let row = order_by_id(&pool, "acme", &placed.order_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.discount_code.as_deref(), Some("TEN"));
    assert_eq!(row.discount_cents, 250);
    let counts = timada_orders::discount_use_counts(&pool, "acme")
        .await
        .unwrap();
    assert_eq!(counts.get(&id), Some(&1));

    // The order spends the use (told twice, it counts once).
    for _ in 0..2 {
        discount_usage_subscription::<evento::Sqlite>(evento::Evento::new(executor.clone()))
            .run_once(&executor)
            .await
            .unwrap();
    }
    let usage = discounts.load_usage(&id).await.unwrap().unwrap();
    assert_eq!(usage.consumed.get(&cart_id), Some(&placed.order_id));
}

#[tokio::test]
async fn a_code_that_no_longer_works_is_never_dropped_silently() {
    let (orders, catalog) = store().await;
    let discounts = timada_discounts::Command(orders.0.clone());
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let id = discount(&discounts, percent("TEN", 1000)).await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    orders
        .apply_discount(SHOPPER, &cart_id, "TEN")
        .await
        .unwrap();
    discounts.deactivate_discount(ALICE, &id).await.unwrap();

    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.discount, None);
    assert_eq!(quote.discount_refused, Some("discount_inactive"));
    assert_eq!(quote.total_cents, 1250);

    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();
    let refused = orders.pending_payment(&cart_id, &request_id).await;
    assert!(
        matches!(
            refused,
            Err(OrdersError::DiscountRefused {
                code: "discount_inactive"
            })
        ),
        "{refused:?}"
    );
    // The refusal as the dashboard records it reads back as the same code.
    orders
        .refuse_payment_request(DASHBOARD, &cart_id, &request_id, "discount_inactive")
        .await
        .unwrap();
    let err = OrdersError::PaymentRefused {
        code: "discount_inactive".into(),
    };
    assert_eq!(err.code(), Some("discount_inactive"));
}

#[tokio::test]
async fn an_amount_off_in_another_currency_takes_nothing_off() {
    let (orders, catalog) = store().await;
    let discounts = timada_discounts::Command(orders.0.clone());
    let mug = product(&catalog, "Mug", 1250, "USD").await;
    discount(
        &discounts,
        CreateDiscount {
            code: "FIVE".into(),
            value: DiscountValue::Amount {
                cents: 500,
                currency: "EUR".into(),
            },
            ..Default::default()
        },
    )
    .await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    orders
        .apply_discount(SHOPPER, &cart_id, "five")
        .await
        .unwrap();
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.discount_refused, Some("discount_currency"));
    assert_eq!(quote.total_cents, 1250);
}
