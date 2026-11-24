mod common;

use common::*;
use timada_writer::{
    Call, DRAFT_STALE_SECS, Room, Spend, SpendCap, SpendCaps, spend_day, spend_day_ends,
};

const CAPS: SpendCaps = SpendCaps {
    shop_usd_micros: 1_000_000,
    platform_usd_micros: 1_500_000,
};

fn call(tenant_id: &'static str) -> Call<'static> {
    Call {
        tenant_id,
        draft_id: "draft-1",
        most_usd_micros: 400_000,
    }
}

async fn held(spend: &Spend<evento::Turso>, tenant: &'static str, now: u64) -> timada_writer::Hold {
    match spend
        .hold_spend("saga", call(tenant), CAPS, now)
        .await
        .unwrap()
    {
        Room::Held(hold) => hold,
        refused => panic!("{refused:?}"),
    }
}

#[tokio::test]
async fn a_call_is_held_then_charged_what_it_cost() {
    let spend = Spend(executor_with_pool().await.0);
    let day = spend_day(NOW);
    assert_eq!(spend.load_spend_day(&day).await.unwrap().charged(), 0);

    let hold = held(&spend, "acme", NOW).await;
    assert_eq!(hold.day, day);
    assert!(spend.charge_spend("saga", &hold, 21_500).await.unwrap());
    // Settled: neither charged twice nor released after.
    assert!(!spend.charge_spend("saga", &hold, 21_500).await.unwrap());
    assert!(!spend.release_spend("saga", &hold).await.unwrap());
    let state = spend.load_spend_day(&day).await.unwrap();
    assert_eq!(state.charged_to("acme").usd_micros, 21_500);
    assert_eq!(state.charged_to("acme").calls, 1);
    assert_eq!(state.charged_to("other").calls, 0);
    assert_eq!(state.charged(), 21_500);

    // A call that gave nothing frees its room.
    let nothing = held(&spend, "acme", NOW).await;
    assert!(spend.release_spend("saga", &nothing).await.unwrap());
    let state = spend.load_spend_day(&day).await.unwrap();
    assert!(state.holds.is_empty());
    assert_eq!(state.charged(), 21_500);
}

#[tokio::test]
async fn the_caps_count_what_is_held_and_are_never_passed() {
    let spend = Spend(executor_with_pool().await.0);
    // A hold is as old as its event, which the store stamps with the
    // clock: the test reads the clock too.
    let now = timada_core::unix_now();
    // Two calls on their way hold 0.80 of the shop's 1.00: no room for a
    // third, which may cost 0.40.
    let first = held(&spend, "acme", now).await;
    held(&spend, "acme", now).await;
    let refused = spend
        .hold_spend("saga", call("acme"), CAPS, now)
        .await
        .unwrap();
    assert_eq!(
        refused,
        Room::Refused {
            cap: SpendCap::Shop,
            frees_at: spend_day_ends(now),
        }
    );
    // Charged at what it cost, the first leaves room again.
    spend.charge_spend("saga", &first, 20_000).await.unwrap();
    held(&spend, "acme", now).await;

    // Another shop has its own cap, inside the platform's: 0.82 is taken,
    // one more call fits, a second would pass 1.50.
    held(&spend, "other", now).await;
    let refused = spend
        .hold_spend("saga", call("other"), CAPS, now)
        .await
        .unwrap();
    assert!(matches!(
        refused,
        Room::Refused {
            cap: SpendCap::Platform,
            ..
        }
    ));

    // A hold whose node stopped no longer counts once its call cannot be
    // on its way: read off the day, from when the last hold was taken.
    let day = spend.load_spend_day(&spend_day(now)).await.unwrap();
    let last = day.holds.iter().map(|hold| hold.held_at).max().unwrap();
    assert_eq!(
        day.refusal(CAPS, "other", 400_000, last),
        Some(SpendCap::Platform)
    );
    assert_eq!(
        day.refusal(CAPS, "other", 400_000, last + DRAFT_STALE_SECS),
        None
    );
    // The next day starts empty.
    held(&spend, "acme", now + 86_400).await;
    held(&spend, "acme", now + 86_400).await;
}
