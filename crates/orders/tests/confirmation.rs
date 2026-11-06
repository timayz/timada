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
async fn each_mail_is_sent_once_per_order() {
    let (orders, _) = store().await;
    assert!(
        orders
            .claim_order_mail("order-1", "order_confirmation", "ada@example.org", "node-a")
            .await
            .unwrap()
    );
    assert!(
        !orders
            .claim_order_mail("order-1", "order_confirmation", "ada@example.org", "node-b")
            .await
            .unwrap()
    );
    // Another mail of the same order is its own.
    assert!(
        orders
            .claim_order_mail("order-1", "order_cancelled", "ada@example.org", "node-b")
            .await
            .unwrap()
    );
    let mailed = orders
        .load_order_mail("order-1", "order_confirmation")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mailed.email, "ada@example.org");
    assert_eq!(mailed.mail, "order_confirmation");
    assert!(
        orders
            .load_order_mail("order-2", "order_confirmation")
            .await
            .unwrap()
            .is_none()
    );
}
