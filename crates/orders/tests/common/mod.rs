#![allow(dead_code)]

use timada_catalog::{CreateProduct, DefineVariant, OptionSelection};
use timada_core::db::Db;
use timada_orders::{
    CancelPath, CartItem, Checkout, Command, OpenedPayment, PROVIDER_STRIPE, Payment, PlacedOrder,
    STOREFRONT_ACTOR, ShippingAddress,
};

pub type Orders = Command<evento::Turso>;
pub type Catalog = timada_catalog::Command<evento::Turso>;

/// A fresh in-memory event store with the catalog's and the discounts' read
/// models in it, and the orders' read-model database (still of its own).
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_orders::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

/// The two gateways over one store.
pub async fn store() -> (Orders, Catalog) {
    let (executor, _) = executor_with_pool().await;
    (
        Command::new(executor.clone(), TENANT),
        timada_catalog::Command::new(executor, TENANT),
    )
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";
pub const ALICE: &str = "user-alice";
pub const SHOPPER: &str = STOREFRONT_ACTOR;

/// A published product at `amount_cents` of `currency`.
pub async fn product(catalog: &Catalog, name: &str, amount_cents: u64, currency: &str) -> String {
    let product = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &product.id, amount_cents, currency)
        .await
        .unwrap();
    catalog.publish_product(ALICE, &product.id).await.unwrap();
    product.id
}

/// A published product sold in two colours, `Beige` at 10.00 € (code
/// `MAT-B`) and `Blue` at 12.00 €; returns the product and both variants.
pub async fn varied_product(catalog: &Catalog, name: &str) -> (String, String, String) {
    let product = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    let colour = catalog
        .define_option(
            ALICE,
            &product.id,
            "Colour",
            vec!["Beige".into(), "Blue".into()],
        )
        .await
        .unwrap();
    let variant = |value: &str, amount_cents: u64, sku: Option<&str>| DefineVariant {
        selections: vec![OptionSelection {
            option_id: colour.clone(),
            value: value.to_owned(),
        }],
        sku: sku.map(str::to_owned),
        amount_cents,
        currency: "EUR".into(),
        image_id: None,
    };
    let beige = catalog
        .define_variant(ALICE, &product.id, variant("Beige", 1000, Some("MAT-B")))
        .await
        .unwrap();
    let blue = catalog
        .define_variant(ALICE, &product.id, variant("Blue", 1200, None))
        .await
        .unwrap();
    catalog.publish_product(ALICE, &product.id).await.unwrap();
    (product.id, beige, blue)
}

pub fn item(product_id: &str, variant_id: Option<&str>, quantity: u32) -> CartItem {
    CartItem {
        product_id: product_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        quantity,
    }
}

pub fn checkout() -> Checkout {
    Checkout {
        email: "ada@example.org".into(),
        shipping: ShippingAddress {
            name: "Ada Lovelace".into(),
            line1: "1 rue de la Paix".into(),
            line2: None,
            postal_code: "75002".into(),
            city: "Paris".into(),
            region: None,
            country: "FR".into(),
            phone: Some("06 12 34 56 78".into()),
        },
        note: None,
        expected_total_cents: None,
        vat_number: None,
        billing: None,
        shipping_method: None,
        language: None,
        newsletter: false,
        time_zone: None,
    }
}

/// A company's address elsewhere than the parcel, as typed.
pub fn billing() -> ShippingAddress {
    ShippingAddress {
        name: " Analytical Engines GmbH ".into(),
        line1: "Unter den Linden 1".into(),
        line2: Some(" ".into()),
        postal_code: "10117".into(),
        city: "Berlin".into(),
        region: None,
        country: "de".into(),
        phone: Some("+49 30 123456".into()),
    }
}

/// A cart holding `quantity` of the product.
pub async fn cart_with(orders: &Orders, product_id: &str, quantity: u32) -> String {
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();
    orders
        .add_to_cart(SHOPPER, &cart_id, item(product_id, None, quantity))
        .await
        .unwrap();
    cart_id
}

/// Who answers requests to pay, as the dashboard does.
pub const DASHBOARD: &str = "timada-dashboard";

/// Asks to pay for the cart and opens a payment for it as the dashboard
/// would; returns the request id and the payment.
pub async fn open_payment(orders: &Orders, cart_id: &str, checkout: Checkout) -> (String, Payment) {
    let request_id = orders
        .request_checkout(SHOPPER, cart_id, checkout)
        .await
        .unwrap();
    let pending = orders
        .pending_payment(cart_id, &request_id)
        .await
        .unwrap()
        .expect("a request waiting for its answer");
    let payment = Payment {
        provider: PROVIDER_STRIPE.into(),
        payment_id: format!("pi_{request_id}"),
        amount_cents: pending.total_cents,
        currency: pending.currency.clone(),
        capture_before: None,
    };
    orders
        .record_payment_request(
            DASHBOARD,
            cart_id,
            pending,
            OpenedPayment {
                provider: PROVIDER_STRIPE.into(),
                payment_id: payment.payment_id.clone(),
                client_secret: format!("{}_secret", payment.payment_id),
                publishable_key: "pk_test".into(),
            },
        )
        .await
        .unwrap();
    (request_id, payment)
}

/// From a cart to its order, the payment held on the shopper's card.
pub async fn place(orders: &Orders, cart_id: &str, checkout: Checkout) -> PlacedOrder {
    let (_, payment) = open_payment(orders, cart_id, checkout).await;
    orders
        .place_order(DASHBOARD, cart_id, payment)
        .await
        .unwrap()
}

/// Takes the held money of an order, as the dashboard does once the
/// provider said it did.
pub async fn capture(orders: &Orders, order_id: &str) {
    orders.request_capture(DASHBOARD, order_id).await.unwrap();
    let order = orders.load_order(order_id).await.unwrap().unwrap();
    orders
        .record_payment(
            DASHBOARD,
            order_id,
            &order.payment.payment_id,
            order.payment.amount_cents,
        )
        .await
        .unwrap();
}

/// The whole way from a cart to its paid order.
pub async fn pay(orders: &Orders, cart_id: &str, checkout: Checkout) -> PlacedOrder {
    let placed = place(orders, cart_id, checkout).await;
    capture(orders, &placed.order_id).await;
    placed
}

/// Cancels an order as the dashboard does once the provider answered: the
/// hold released, or what was taken refunded first.
pub async fn cancel(orders: &Orders, order_id: &str, reason: &str) {
    match orders
        .request_cancellation(ALICE, order_id, reason.to_owned(), false)
        .await
        .unwrap()
    {
        CancelPath::Release { payment_id } => orders
            .record_release(DASHBOARD, order_id, &payment_id, "")
            .await
            .unwrap(),
        CancelPath::Refund => {
            let outstanding = orders
                .load_order(order_id)
                .await
                .unwrap()
                .unwrap()
                .outstanding_cents();
            if outstanding > 0 {
                let pending = orders
                    .request_refund(ALICE, order_id, outstanding)
                    .await
                    .unwrap();
                orders
                    .record_refund(
                        DASHBOARD,
                        order_id,
                        &format!("re_{}", pending.idempotency_key),
                        outstanding,
                    )
                    .await
                    .unwrap();
            }
            orders.complete_cancellation(order_id).await.unwrap();
        }
        CancelPath::Done => {}
    }
}
