//! The generations: a stock generation holds three events here
//! (`chunk_size()`), and remembers two orders (`recent_orders()`).

use super::*;
use crate::{Command, CreateProduct};

const TENANT: &str = "acme";
const ALICE: &str = "user-alice";
const HOUR: u64 = 3600;

async fn command() -> Command<evento::Turso> {
    Command::new(evento::Turso::in_memory_concurrent().await.unwrap(), TENANT)
}

async fn new_product(cmd: &Command<evento::Turso>) -> String {
    cmd.create_product(
        ALICE,
        CreateProduct {
            name: "Mug".into(),
            slug: None,
        },
    )
    .await
    .unwrap()
    .id
}

fn line(product_id: &str, quantity: u32) -> [StockLine; 1] {
    [StockLine {
        product_id: product_id.to_owned(),
        variant_id: None,
        quantity,
    }]
}

/// The generation the pointer names.
async fn generation_of(cmd: &Command<evento::Turso>, product_id: &str) -> u32 {
    stock_pointer_projection()
        .load(stock_pointer_id(product_id, None))
        .tenant(TENANT)
        .execute(&cmd.executor)
        .await
        .unwrap()
        .map_or(0, |pointer| pointer.generation)
}

/// How many events generation `generation` holds; `None` without a stream.
async fn version_of(
    cmd: &Command<evento::Turso>,
    product_id: &str,
    generation: u32,
) -> Option<u16> {
    stock_projection()
        .load(stock_item_id(product_id, None, generation))
        .tenant(TENANT)
        .execute(&cmd.executor)
        .await
        .unwrap()
        .map(|stock| stock.aggregate_version)
}

async fn stock_of(cmd: &Command<evento::Turso>, product_id: &str) -> StockState {
    cmd.load_stock(product_id, None).await.unwrap().unwrap()
}

#[tokio::test]
async fn selling_past_a_generation_keeps_the_count_and_the_holds() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 10).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-1", &line(&mug, 2), HOUR)
        .await
        .unwrap();
    cmd.reserve_stock(ALICE, "cart-2", &line(&mug, 3), HOUR)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &mug).await, 0);
    assert_eq!(version_of(&cmd, &mug, 0).await, Some(3));

    // The first generation is full: the order's take opens the second.
    cmd.consume_stock(ALICE, "cart-1", "order-1", &line(&mug, 2))
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &mug).await, 1);
    assert_eq!(
        version_of(&cmd, &mug, 0).await,
        Some(3),
        "nothing more goes to a full one"
    );
    assert_eq!(
        version_of(&cmd, &mug, 1).await,
        Some(2),
        "opened, then consumed"
    );
    let stock = stock_of(&cmd, &mug).await;
    assert_eq!(stock.on_hand, 8);
    assert_eq!(stock.holds.keys().collect::<Vec<_>>(), ["cart-2"]);
    assert_eq!(stock.available_at(unix_now()), Some(5));
    assert!(stock.consumed_by_cart("cart-1"));

    // And again: the second fills up, the third carries on.
    cmd.release_stock(ALICE, "cart-2", &line(&mug, 3))
        .await
        .unwrap();
    cmd.reserve_stock(ALICE, "cart-3", &line(&mug, 4), HOUR)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &mug).await, 2);
    let stock = stock_of(&cmd, &mug).await;
    assert_eq!(stock.on_hand, 8);
    assert_eq!(stock.holds.keys().collect::<Vec<_>>(), ["cart-3"]);
    assert!(
        stock.consumed_by_cart("cart-1"),
        "the recent orders are carried too"
    );
}

#[tokio::test]
async fn two_checkouts_at_a_full_generation_take_the_last_one_once() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 1).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-0", &line(&mug, 1), HOUR)
        .await
        .unwrap();
    cmd.release_stock(ALICE, "cart-0", &line(&mug, 1))
        .await
        .unwrap();
    assert_eq!(version_of(&cmd, &mug, 0).await, Some(3));

    let reserve = |cart: &'static str| {
        let cmd = &cmd;
        let mug = mug.clone();
        async move { cmd.reserve_stock(ALICE, cart, &line(&mug, 1), HOUR).await }
    };
    let (a, b) = tokio::join!(reserve("cart-a"), reserve("cart-b"));
    let results = [a, b];
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(CatalogError::OutOfStock { available: 0, .. }))),
        "{results:?}"
    );
    assert_eq!(generation_of(&cmd, &mug).await, 1);
    assert_eq!(
        version_of(&cmd, &mug, 1).await,
        Some(2),
        "opened once, held once"
    );
    assert_eq!(stock_of(&cmd, &mug).await.holds.len(), 1);
}

#[tokio::test]
async fn an_order_takes_its_share_once_however_long_ago() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 9).await.unwrap();
    for (cart, order, quantity) in [
        ("cart-1", "order-1", 1),
        ("cart-2", "order-2", 2),
        ("cart-3", "order-3", 3),
    ] {
        cmd.consume_stock(ALICE, cart, order, &line(&mug, quantity))
            .await
            .unwrap();
    }
    assert_eq!(generation_of(&cmd, &mug).await, 1);
    let stock = stock_of(&cmd, &mug).await;
    assert_eq!(stock.on_hand, 3);
    assert!(
        stock.recent_take("order-1").is_none(),
        "the generation remembers two orders"
    );
    assert!(stock.recent_take("order-3").is_some());

    // Delivered again, long after the generation forgot it.
    cmd.consume_stock(ALICE, "cart-1", "order-1", &line(&mug, 1))
        .await
        .unwrap();
    assert_eq!(stock_of(&cmd, &mug).await.on_hand, 3);
    let take = cmd.load_take("order-1").await.unwrap().unwrap();
    assert_eq!(take.cart_id, "cart-1");
    assert_eq!(take.lines.len(), 1);
    assert_eq!(take.lines[0].quantity, 1);
    assert!(!take.restored);
}

#[tokio::test]
async fn a_cancelled_order_gives_its_share_back_once() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 5).await.unwrap();
    cmd.consume_stock(ALICE, "cart-1", "order-1", &line(&mug, 2))
        .await
        .unwrap();
    assert_eq!(stock_of(&cmd, &mug).await.on_hand, 3);
    cmd.restore_stock(ALICE, "order-1").await.unwrap();
    assert_eq!(stock_of(&cmd, &mug).await.on_hand, 5);
    assert!(cmd.load_take("order-1").await.unwrap().unwrap().restored);
    assert_eq!(version_of(&cmd, &mug, 0).await, Some(3));

    // Over a roll, told again, and taken again: nothing more happens.
    cmd.reserve_stock(ALICE, "cart-9", &line(&mug, 1), HOUR)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &mug).await, 1);
    cmd.restore_stock(ALICE, "order-1").await.unwrap();
    cmd.consume_stock(ALICE, "cart-1", "order-1", &line(&mug, 2))
        .await
        .unwrap();
    assert_eq!(stock_of(&cmd, &mug).await.on_hand, 5);

    // An order that took nothing counted has nothing to give back.
    cmd.restore_stock(ALICE, "order-none").await.unwrap();
    assert!(cmd.load_take("order-none").await.unwrap().is_none());
}

#[tokio::test]
async fn a_live_hold_is_carried_over_a_roll_and_a_lapsed_one_is_not() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 1).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-1", &line(&mug, 1), HOUR)
        .await
        .unwrap();
    cmd.set_stock(ALICE, &mug, None, 2).await.unwrap();
    assert_eq!(version_of(&cmd, &mug, 0).await, Some(3));
    let err = cmd
        .reserve_stock(ALICE, "cart-3", &line(&mug, 2), HOUR)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CatalogError::OutOfStock { available: 1, .. }),
        "{err:?}"
    );
    assert_eq!(generation_of(&cmd, &mug).await, 1);

    let cup = new_product(&cmd).await;
    cmd.set_stock(ALICE, &cup, None, 1).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-lapsed", &line(&cup, 1), 0)
        .await
        .unwrap();
    cmd.set_stock(ALICE, &cup, None, 2).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-3", &line(&cup, 2), HOUR)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &cup).await, 1);
    let stock = stock_of(&cmd, &cup).await;
    assert_eq!(stock.holds.keys().collect::<Vec<_>>(), ["cart-3"]);
}

#[tokio::test]
async fn an_untracked_thing_rolls_untracked() {
    let cmd = command().await;
    let mug = new_product(&cmd).await;
    cmd.set_stock(ALICE, &mug, None, 1).await.unwrap();
    cmd.untrack_stock(ALICE, &mug, None).await.unwrap();
    cmd.set_stock(ALICE, &mug, None, 4).await.unwrap();
    cmd.untrack_stock(ALICE, &mug, None).await.unwrap();
    assert_eq!(generation_of(&cmd, &mug).await, 1);
    let stock = stock_of(&cmd, &mug).await;
    assert!(!stock.tracked);
    assert_eq!(stock.available_at(0), None);
    cmd.reserve_stock(ALICE, "cart-1", &line(&mug, 500), HOUR)
        .await
        .unwrap();
    assert!(stock_of(&cmd, &mug).await.holds.is_empty());
}
