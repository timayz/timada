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
    // Told again: the order's spend says so before any generation is read.
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &id).await, 0, "nothing was written");
    // Over the roll, the new generation still remembers the cart spent it.
    cmd.reserve_use(SHOP, &id, "cart-1", "x@example.com", RESERVATION_SECS)
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

/// An order cancelled before it was paid gives its use back: the cap counts
/// it no more, over a roll too, and told twice it counts once.
#[tokio::test]
async fn a_use_given_back_counts_no_more() {
    let cmd = command().await;
    let id = capped(&cmd, "BACK", 1).await.id;
    cmd.reserve_use(SHOP, &id, "cart-1", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    let full = cmd
        .reserve_use(SHOP, &id, "cart-2", "y@example.com", RESERVATION_SECS)
        .await;
    assert!(matches!(full, Err(DiscountError::Exhausted)), "{full:?}");

    cmd.return_use(SHOP, "order-1").await.unwrap();
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.consumed_count, 0);
    assert!(usage.returned_by("order-1"));
    assert_eq!(usage.consumed_by_cart("cart-1"), None);
    assert!(cmd.load_spend("order-1").await.unwrap().unwrap().returned);
    // Told again: nothing more.
    cmd.return_use(SHOP, "order-1").await.unwrap();
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 0);
    // An order that spent nothing: nothing to give back.
    cmd.return_use(SHOP, "order-none").await.unwrap();

    // The use serves another order; the generation (three events) rolls.
    cmd.reserve_use(SHOP, &id, "cart-2", "y@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    assert_eq!(generation_of(&cmd, &id).await, 1);
    let usage = usage_of(&cmd, &id).await;
    assert!(usage.returned_by("order-1"), "carried over the roll");
    assert_eq!(usage.taken_at(unix_now()), 1);
}

/// A delivery that stopped after the usage was given back, before the
/// order's spend said so: told again, the generation remembers it.
#[tokio::test]
async fn a_use_given_back_twice_counts_once() {
    let cmd = command().await;
    let id = capped(&cmd, "TWICE", 5).await.id;
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    cmd.consume_use(SHOP, &id, "cart-2", "x@example.com", "order-2")
        .await
        .unwrap();
    cmd.return_usage(SHOP, &id, "cart-1", "order-1", &None)
        .await
        .unwrap();
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 1);
    cmd.return_use(SHOP, "order-1").await.unwrap();
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 1);
}

/// A node behind the others hears of the order after another node gave its
/// use back: told to spend it again, nothing is spent, and the e-mail stays
/// free.
#[tokio::test]
async fn a_use_given_back_is_not_spent_again_by_a_late_delivery() {
    let cmd = command().await;
    let discount = cmd
        .create_discount(
            ALICE,
            CreateDiscount {
                code: "LATE".to_owned(),
                value: DiscountValue::Percent(1000),
                max_uses: Some(1),
                once_per_email: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = discount.id;
    cmd.reserve_use(SHOP, &id, "cart-1", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    cmd.return_use(SHOP, "order-1").await.unwrap();
    assert_eq!(usage_of(&cmd, &id).await.consumed_count, 0);

    // The late node: the order's placing, then its cancellation.
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    cmd.return_use(SHOP, "order-1").await.unwrap();
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.consumed_count, 0, "the use stays given back");
    assert_eq!(usage.consumed_by_cart("cart-1"), None);
    assert!(
        cmd.load_buyer(&id, "x@example.com")
            .await
            .unwrap()
            .is_none_or(|buyer| buyer.order_id.is_none()),
        "the e-mail stays free"
    );
    cmd.reserve_use(SHOP, &id, "cart-2", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
}

/// Two nodes catching up at once: node B finds no spend, then node A spends
/// the use and gives it back before B reads the streams. B, reading the
/// spend again before each write, spends nothing.
#[tokio::test]
async fn a_use_given_back_meanwhile_is_not_spent_again() {
    let cmd = command().await;
    let discount = cmd
        .create_discount(
            ALICE,
            CreateDiscount {
                code: "RACE".to_owned(),
                value: DiscountValue::Percent(1000),
                max_uses: Some(1),
                once_per_email: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = discount.id;
    cmd.reserve_use(SHOP, &id, "cart-1", "x@example.com", RESERVATION_SECS)
        .await
        .unwrap();
    // Node A, all the way through.
    cmd.consume_use(SHOP, &id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    cmd.return_use(SHOP, "order-1").await.unwrap();
    // Node B, past its first look at the spend.
    cmd.consume_unspent(SHOP, &id, "cart-1", "x@example.com", "order-1", None)
        .await
        .unwrap();
    let usage = usage_of(&cmd, &id).await;
    assert_eq!(usage.consumed_count, 0, "the use stays given back");
    assert!(
        cmd.load_buyer(&id, "x@example.com")
            .await
            .unwrap()
            .is_none_or(|buyer| buyer.order_id.is_none()),
        "the e-mail stays free"
    );
}

/// A code good once per e-mail, given back: the e-mail may use it again.
#[tokio::test]
async fn a_use_given_back_frees_the_email() {
    let cmd = command().await;
    let discount = cmd
        .create_discount(
            ALICE,
            CreateDiscount {
                code: "ONCE".to_owned(),
                value: DiscountValue::Percent(1000),
                once_per_email: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    cmd.reserve_use(
        SHOP,
        &discount.id,
        "cart-1",
        "x@example.com",
        RESERVATION_SECS,
    )
    .await
    .unwrap();
    cmd.consume_use(SHOP, &discount.id, "cart-1", "x@example.com", "order-1")
        .await
        .unwrap();
    let used = cmd
        .reserve_use(
            SHOP,
            &discount.id,
            "cart-2",
            "x@example.com",
            RESERVATION_SECS,
        )
        .await;
    assert!(matches!(used, Err(DiscountError::AlreadyUsed)), "{used:?}");

    cmd.return_use(SHOP, "order-1").await.unwrap();
    cmd.reserve_use(
        SHOP,
        &discount.id,
        "cart-2",
        "x@example.com",
        RESERVATION_SECS,
    )
    .await
    .unwrap();
}
