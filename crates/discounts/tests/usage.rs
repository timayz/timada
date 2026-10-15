mod common;

use common::*;
use timada_discounts::{CreateDiscount, DiscountError, RESERVATION_SECS};

#[tokio::test]
async fn a_cap_is_never_exceeded_by_checkouts_at_once() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(2),
            ..percent("TWO", 1000)
        },
    )
    .await;
    let reserve = |cart: &'static str| {
        let cmd = &cmd;
        let id = discount.id.clone();
        async move {
            cmd.reserve_use(
                SHOP,
                &id,
                cart,
                &format!("{cart}@example.com"),
                RESERVATION_SECS,
            )
            .await
        }
    };
    let (a, b, c) = tokio::join!(reserve("cart-a"), reserve("cart-b"), reserve("cart-c"));
    let results = [a, b, c];
    let granted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(granted, 2, "{results:?}");
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(DiscountError::Exhausted)))
    );
    let usage = cmd.load_usage(&discount.id).await.unwrap().unwrap();
    assert_eq!(usage.reserved.len(), 2);
}

#[tokio::test]
async fn a_cart_reserving_again_keeps_its_use_and_a_release_frees_it() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(1),
            ..percent("ONE", 1000)
        },
    )
    .await;
    let id = &discount.id;
    cmd.reserve_use(SHOP, id, "cart-a", "a@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.reserve_use(SHOP, id, "cart-a", "a@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    assert!(matches!(
        cmd.reserve_use(SHOP, id, "cart-b", "b@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::Exhausted)
    ));

    cmd.release_use(SHOP, id, "cart-a", "a@example.com")
        .await
        .unwrap();
    cmd.release_use(SHOP, id, "cart-a", "a@example.com")
        .await
        .unwrap();
    cmd.reserve_use(SHOP, id, "cart-b", "b@example.com", RESERVATION_SECS)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_lapsed_reservation_no_longer_counts() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(1),
            ..percent("LAPSE", 1000)
        },
    )
    .await;
    // Held for no time at all: lapsed as soon as it is written.
    cmd.reserve_use(SHOP, &discount.id, "cart-a", "a@example.com", 0)
        .await
        .unwrap();
    cmd.reserve_use(
        SHOP,
        &discount.id,
        "cart-b",
        "b@example.com",
        RESERVATION_SECS,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_spent_use_stays_spent_and_is_told_once() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(1),
            ..percent("SPEND", 1000)
        },
    )
    .await;
    let id = &discount.id;
    cmd.reserve_use(SHOP, id, "cart-a", "a@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, id, "cart-a", "a@example.com", "order-a")
        .await
        .unwrap();
    cmd.consume_use(SHOP, id, "cart-a", "a@example.com", "order-a")
        .await
        .unwrap();
    // Released afterwards (a late answer): the order keeps it.
    cmd.release_use(SHOP, id, "cart-a", "a@example.com")
        .await
        .unwrap();
    let usage = cmd.load_usage(id).await.unwrap().unwrap();
    assert_eq!(usage.consumed.len(), 1);
    assert!(usage.reserved.is_empty());
    assert!(matches!(
        cmd.reserve_use(SHOP, id, "cart-b", "b@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::Exhausted)
    ));
    // The same cart asking again is fine.
    cmd.reserve_use(SHOP, id, "cart-a", "a@example.com", RESERVATION_SECS)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_order_whose_reservation_lapsed_is_still_counted() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(1),
            ..percent("LATE", 1000)
        },
    )
    .await;
    let id = &discount.id;
    cmd.reserve_use(SHOP, id, "cart-a", "a@example.com", 0)
        .await
        .unwrap();
    cmd.reserve_use(SHOP, id, "cart-b", "b@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, id, "cart-a", "a@example.com", "order-a")
        .await
        .unwrap();
    cmd.consume_use(SHOP, id, "cart-b", "b@example.com", "order-b")
        .await
        .unwrap();
    let usage = cmd.load_usage(id).await.unwrap().unwrap();
    assert_eq!(usage.consumed.len(), 2);
}

#[tokio::test]
async fn once_per_email_refuses_a_second_checkout_of_the_same_shopper() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            once_per_email: true,
            ..percent("WELCOME", 1000)
        },
    )
    .await;
    let id = &discount.id;
    cmd.reserve_use(SHOP, id, "cart-a", "ann@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    // Another checkout of hers while the first one holds it.
    assert!(matches!(
        cmd.reserve_use(SHOP, id, "cart-b", "ann@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::AlreadyUsed)
    ));
    // Someone else is fine.
    cmd.reserve_use(SHOP, id, "cart-c", "bob@example.com", RESERVATION_SECS)
        .await
        .unwrap();

    // The first payment fails: the second checkout may have it.
    cmd.release_use(SHOP, id, "cart-a", "ann@example.com")
        .await
        .unwrap();
    cmd.reserve_use(SHOP, id, "cart-b", "ann@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, id, "cart-b", "ann@example.com", "order-b")
        .await
        .unwrap();
    assert!(matches!(
        cmd.reserve_use(SHOP, id, "cart-d", "ann@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::AlreadyUsed)
    ));
}

#[tokio::test]
async fn a_full_cap_gives_the_shoppers_use_back() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            max_uses: Some(1),
            once_per_email: true,
            ..percent("BOTH", 1000)
        },
    )
    .await;
    let id = &discount.id;
    cmd.reserve_use(SHOP, id, "cart-a", "ann@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    assert!(matches!(
        cmd.reserve_use(SHOP, id, "cart-b", "bob@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::Exhausted)
    ));
    let bob = cmd
        .load_buyer(id, "bob@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob.cart_id, None);
}

#[tokio::test]
async fn a_code_without_limits_holds_nothing() {
    let cmd = command().await;
    let discount = create(&cmd, percent("OPEN", 1000)).await;
    cmd.reserve_use(
        SHOP,
        &discount.id,
        "cart-a",
        "a@example.com",
        RESERVATION_SECS,
    )
    .await
    .unwrap();
    cmd.consume_use(SHOP, &discount.id, "cart-a", "a@example.com", "order-a")
        .await
        .unwrap();
    assert!(cmd.load_usage(&discount.id).await.unwrap().is_none());
}
