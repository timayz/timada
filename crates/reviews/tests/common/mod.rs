#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_orders::aggregator::{Cancelled, Placed};
use timada_orders::{OrderLine, Payment, ShippingAddress};
use timada_reviews::{Command, ReviewPhoto, ReviewState, SubmitReview};

pub type Reviews = Command<evento::Sqlite>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_reviews::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Reviews {
    Command(executor_with_pool().await.0)
}

pub const ALICE: &str = "user-alice";
pub const SHOPPER: &str = "storefront";
pub const MUG: &str = "product-mug";
pub const TEA: &str = "product-tea";

/// A paid order shipped to Ada Lovelace: a beige mug and some tea.
pub async fn paid_order(executor: &evento::Sqlite, order_id: &str) {
    let line = |product_id: &str, name: &str, variant_label: Option<&str>| OrderLine {
        product_id: product_id.to_owned(),
        name: name.to_owned(),
        variant_label: variant_label.map(str::to_owned),
        unit_amount_cents: 1000,
        quantity: 1,
        ..Default::default()
    };
    evento::append(order_id)
        .original_version(0)
        .event(&Placed {
            reference: format!("REF-{order_id}"),
            cart_id: format!("cart-{order_id}"),
            email: "ada@example.com".to_owned(),
            shipping: ShippingAddress {
                name: "Ada Lovelace".to_owned(),
                line1: "1 rue de la Paix".to_owned(),
                postal_code: "75002".to_owned(),
                city: "Paris".to_owned(),
                country: "FR".to_owned(),
                ..Default::default()
            },
            note: None,
            lines: vec![line(MUG, "Mug", Some("Beige")), line(TEA, "Tea", None)],
            currency: "EUR".to_owned(),
            total_cents: 2000,
            payment: Payment {
                provider: "stripe".to_owned(),
                payment_id: format!("pi-{order_id}"),
                amount_cents: 2000,
                currency: "EUR".to_owned(),
            },
            placed_by: SHOPPER.to_owned(),
        })
        .commit(executor)
        .await
        .unwrap();
}

/// Cancels the order made by [`paid_order`].
pub async fn cancel(executor: &evento::Sqlite, order_id: &str) {
    evento::append(order_id)
        .original_version(1)
        .event(&Cancelled {
            reason: "changed my mind".to_owned(),
            cancelled_by: ALICE.to_owned(),
        })
        .commit(executor)
        .await
        .unwrap();
}

pub fn photo(id: &str) -> ReviewPhoto {
    ReviewPhoto {
        id: id.to_owned(),
        ext: "png".to_owned(),
        mime: "image/png".to_owned(),
        size: 68,
        width: Some(1),
        height: Some(1),
    }
}

pub fn draft(order_id: &str, product_id: &str, rating: u8) -> SubmitReview {
    SubmitReview {
        order_id: order_id.to_owned(),
        product_id: product_id.to_owned(),
        rating,
        title: Some("  Lovely  ".to_owned()),
        body: "  Holds my tea all day long.  ".to_owned(),
        photos: vec![photo("photo-1"), photo("photo-2")],
    }
}

pub async fn submit(
    reviews: &Reviews,
    order_id: &str,
    product_id: &str,
    rating: u8,
) -> ReviewState {
    reviews
        .submit_review(SHOPPER, draft(order_id, product_id, rating))
        .await
        .unwrap()
}
