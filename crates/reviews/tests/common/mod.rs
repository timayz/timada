#![allow(dead_code)]

use timada_core::db::Db;
use timada_orders::aggregator::{Cancelled, Fulfilled, Paid, Placed};
use timada_orders::{OrderLine, Payment, ShippingAddress};
use timada_reviews::{Command, ReviewPhoto, ReviewState, SubmitReview};

pub type Reviews = Command<evento::Turso>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_reviews::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Reviews {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";
pub const ALICE: &str = "user-alice";
pub const SHOPPER: &str = "storefront";
pub const MUG: &str = "product-mug";
pub const TEA: &str = "product-tea";

/// A paid order shipped to Ada Lovelace, in the tenant: a beige mug and
/// some tea.
pub async fn paid_order(executor: &evento::Turso, order_id: &str) {
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
                region: None,
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
                capture_before: None,
            },
            supplier: None,
            placed_by: SHOPPER.to_owned(),
        })
        .event(&Paid {
            payment_id: format!("pi-{order_id}"),
            amount_cents: 2000,
            paid_by: "timada-dashboard".to_owned(),
        })
        .tenant(TENANT)
        .commit(executor)
        .await
        .unwrap();
}

/// Ships the order made by [`paid_order`]; returns when it left (unix
/// seconds).
pub async fn fulfil(executor: &evento::Turso, order_id: &str) -> u64 {
    evento::append(order_id)
        .original_version(2)
        .event(&Fulfilled {
            tracking: None,
            tracking_url: None,
            fulfilled_by: ALICE.to_owned(),
        })
        .tenant(TENANT)
        .commit(executor)
        .await
        .unwrap();
    timada_orders::Command::new(executor.clone(), TENANT)
        .load_order(order_id)
        .await
        .unwrap()
        .unwrap()
        .fulfilled_at
        .unwrap()
}

/// Cancels the order made by [`paid_order`].
pub async fn cancel(executor: &evento::Turso, order_id: &str) {
    evento::append(order_id)
        .original_version(2)
        .event(&Cancelled {
            reason: "changed my mind".to_owned(),
            cancelled_by: ALICE.to_owned(),
        })
        .tenant(TENANT)
        .commit(executor)
        .await
        .unwrap();
}

pub fn photo(id: &str) -> ReviewPhoto {
    let rendition = timada_core::RenditionFile {
        width: 1,
        height: 1,
        size: 68,
        sha256: [1; 32],
    };
    ReviewPhoto {
        id: id.to_owned(),
        picture: timada_core::Picture {
            width: 1,
            height: 1,
            listing: rendition,
            page: rendition,
        },
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
