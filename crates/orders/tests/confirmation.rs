mod common;

use common::*;
use timada_orders::OrderError;

#[tokio::test]
async fn the_storefront_address_is_set_and_forgotten() {
    let (orders, _) = store().await;
    assert_eq!(orders.storefront_address().await.unwrap(), None);

    let err = orders
        .set_storefront_address(ALICE, Some("shop.example"))
        .await
        .unwrap_err();
    assert!(matches!(err, OrderError::InvalidStorefrontUrl));

    orders
        .set_storefront_address(ALICE, Some("https://shop.example/"))
        .await
        .unwrap();
    // The same address again writes nothing.
    orders
        .set_storefront_address(ALICE, Some("https://shop.example"))
        .await
        .unwrap();
    assert_eq!(
        orders.storefront_address().await.unwrap().as_deref(),
        Some("https://shop.example")
    );

    orders.set_storefront_address(ALICE, None).await.unwrap();
    assert_eq!(orders.storefront_address().await.unwrap(), None);
}

#[tokio::test]
async fn one_confirmation_is_mailed_per_order() {
    let (orders, _) = store().await;
    assert!(
        orders
            .claim_confirmation("order-1", "ada@example.org", "node-a")
            .await
            .unwrap()
    );
    assert!(
        !orders
            .claim_confirmation("order-1", "ada@example.org", "node-b")
            .await
            .unwrap()
    );
    let mailed = orders
        .load_order_confirmation("order-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mailed.email, "ada@example.org");
    assert!(
        orders
            .load_order_confirmation("order-2")
            .await
            .unwrap()
            .is_none()
    );
}
