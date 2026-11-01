//! The generations: a usage generation holds three events here
//! (`chunk_size()`), and remembers two spent uses (`recent_uses()`).

use super::*;
use crate::{Command, CreateDiscount, DiscountState, DiscountValue, RESERVATION_SECS};

const TENANT: &str = "acme";
const ALICE: &str = "user-alice";
const SHOP: &str = "timada-dashboard";

async fn command() -> Command<evento::Turso> {
    Command::new(evento::Turso::in_memory_concurrent().await.unwrap(), TENANT)
}

async fn capped(cmd: &Command<evento::Turso>, code: &str, max_uses: u32) -> DiscountState {
    cmd.create_discount(
        ALICE,
        CreateDiscount {
            code: code.to_owned(),
            value: DiscountValue::Percent(1000),
            max_uses: Some(max_uses),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// The generation the pointer names.
async fn generation_of(cmd: &Command<evento::Turso>, discount_id: &str) -> u32 {
    usage_pointer_projection()
        .load(discount_usage_pointer_id(discount_id))
        .tenant(TENANT)
        .execute(&cmd.executor)
        .await
        .unwrap()
        .map_or(0, |pointer| pointer.generation)
}

/// How many events generation `generation` holds; `None` without a stream.
async fn version_of(
    cmd: &Command<evento::Turso>,
    discount_id: &str,
    generation: u32,
) -> Option<u16> {
    usage_projection()
        .load(discount_usage_id(discount_id, generation))
        .tenant(TENANT)
        .execute(&cmd.executor)
        .await
        .unwrap()
        .map(|usage| usage.aggregate_version)
}

async fn usage_of(cmd: &Command<evento::Turso>, discount_id: &str) -> UsageState {
    cmd.load_usage(discount_id).await.unwrap().unwrap()
}

#[tokio::test]
async fn a_cap_holds_across_generations() {
    let cmd = command().await;
    let id = capped(&cmd, "FIVE", 5).await.id;
    for cart in ["cart-1", "cart-2", "cart-3"] {
        cmd.reserve_use(SHOP, &id, cart, "x@example.com", RESERVATION_SECS)
            .await
            .unwrap();
    }
    assert_eq!(generation_of(&cmd, &id).await, 0);
    assert_eq!(version_of(&cmd, &id, 0).await, Some(3));

    // The first generation is full: the order's spend opens the second.
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &id).await, 1);
    assert_eq!(
        version_of(&cmd, &id, 0).await,
        Some(3),
        "nothing more goes to a full one"
    );
    assert_eq!(
        version_of(&cmd, &id, 1).await,
        Some(2),
        "opened, then consumed"
    );
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.consumed_count, 1);
    assert_eq!(usage.reserved.len(), 2, "the live reservations are carried");
    assert_eq!(usage.taken_at(unix_now()), 3);
    assert_eq!(usage.consumed_by_cart("cart-1"), Some("order-1"));

    // Two more checkouts fill the cap, over another roll.
    cmd.reserve_use(SHOP, &id, "cart-4", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.reserve_use(SHOP, &id, "cart-5", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &id).await, 2);
    assert!(matches!(
        cmd.reserve_use(SHOP, &id, "cart-6", "x@example.com", RESERVATION_SECS)
            .await,
        Err(DiscountError::Exhausted)
    ));
    // A cart that holds one may ask again, whatever generation it held it in.
    cmd.reserve_use(SHOP, &id, "cart-2", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
}

#[tokio::test]
async fn two_checkouts_at_a_full_generation_take_the_last_use_once() {
    let cmd = command().await;
    let id = capped(&cmd, "ONE", 1).await.id;
    cmd.reserve_use(SHOP, &id, "cart-0", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.release_use(SHOP, &id, "cart-0", "x@example.com")
        .await
        .unwrap();
    cmd.reserve_use(SHOP, &id, "cart-0", "x@example.com", 0)
        .await
        .unwrap();
    assert_eq!(version_of(&cmd, &id, 0).await, Some(3));

    let reserve = |cart: &'static str| {
        let cmd = &cmd;
        let id = id.clone();
        async move {
            cmd.reserve_use(SHOP, &id, cart, "x@example.com", RESERVATION_SECS)
                .await
        }
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
            .any(|r| matches!(r, Err(DiscountError::Exhausted))),
        "{results:?}"
    );
    assert_eq!(generation_of(&cmd, &id).await, 1);
    assert_eq!(
        version_of(&cmd, &id, 1).await,
        Some(2),
        "opened once, held once"
    );
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.reserved.len(), 1, "the lapsed one was not carried");
}

#[tokio::test]
async fn a_spent_use_is_counted_once_over_a_roll() {
    let cmd = command().await;
    let id = capped(&cmd, "SPEND", 9).await.id;
    cmd.reserve_use(SHOP, &id, "cart-1", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    cmd.consume_use(SHOP, &id, "cart-2", "x@example.com", "order-2")
        .await
        .unwrap();
    assert_eq!(version_of(&cmd, &id, 0).await, Some(3));
    // Told again over the roll: the new generation still remembers it.
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &id).await, 1);
    assert_eq!(
        version_of(&cmd, &id, 1).await,
        Some(1),
        "opened, and nothing to write"
    );
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 2);
    cmd.consume_use(SHOP, &id, "cart-3", "x@example.com", "order-3")
        .await
        .unwrap();
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.consumed_count, 3);
    assert_eq!(
        usage.consumed_by_cart("cart-1"),
        None,
        "two spent uses are remembered"
    );
    assert_eq!(usage.consumed_by_cart("cart-3"), Some("order-3"));
    cmd.consume_use(SHOP, &id, "cart-2", "x@example.com", "order-2")
        .await
        .unwrap();
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 3);
}
