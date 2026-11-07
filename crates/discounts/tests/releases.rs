mod common;

use common::*;
use evento::{AggregateEvent, Executor};
use timada_core::claim::CLAIM_GRACE_SECS;
use timada_discounts::aggregator::Claimed;
use timada_discounts::{DiscountError, discount_code_id};

/// Writes `data` as the first event of the index stream `id`, stamped
/// `age_secs` ago: what a node that died after claiming leaves behind.
async fn claimed_ago<D>(store: &evento::Turso, id: &str, data: &D, age_secs: u64)
where
    D: AggregateEvent + bitcode::Encode,
{
    store
        .replicate(vec![evento::Event {
            id: ulid::Ulid::generate(),
            aggregate_id: id.to_owned(),
            aggregate_type: D::aggregate_type().to_owned(),
            version: 1,
            name: D::event_name().to_owned(),
            tenant: TENANT.to_owned(),
            data: bitcode::encode(data),
            timestamp: timada_core::unix_now() - age_secs,
            ..Default::default()
        }])
        .await
        .unwrap();
}

#[tokio::test]
async fn the_code_of_an_archived_discount_is_claimed_at_once() {
    let cmd = command().await;
    let first = create(&cmd, percent("REUSE", 1000)).await;
    cmd.archive_discount(ALICE, &first.id).await.unwrap();
    assert!(
        cmd.load_code_index("REUSE").await.unwrap().is_some(),
        "the entry waits for the release subscription"
    );

    let second = create(&cmd, percent("REUSE", 2000)).await;
    run_releases(&cmd).await;
    let index = cmd.load_code_index("REUSE").await.unwrap().unwrap();
    assert_eq!(
        index.discount_id, second.id,
        "the release that comes later leaves the new holder alone"
    );
}

#[tokio::test]
async fn a_retried_archive_leaves_the_code_free_once_the_releases_ran() {
    let cmd = command().await;
    let first = create(&cmd, percent("REUSE", 1000)).await;
    cmd.archive_discount(ALICE, &first.id).await.unwrap();
    // Asked again, as after a node died: nothing more is written.
    cmd.archive_discount(ALICE, &first.id).await.unwrap();

    run_releases(&cmd).await;
    assert!(cmd.load_code_index("REUSE").await.unwrap().is_none());
    // Told twice, nothing more happens.
    run_releases(&cmd).await;
    assert!(cmd.load_code_index("REUSE").await.unwrap().is_none());
}

#[tokio::test]
async fn a_code_whose_discount_was_never_written_is_held_while_the_claim_is_fresh() {
    let cmd = command().await;
    let claimed = Claimed {
        discount_id: "never-written".to_owned(),
    };
    claimed_ago(cmd.executor(), &discount_code_id("SUMMER"), &claimed, 1).await;
    let taken = cmd.create_discount(ALICE, percent("SUMMER", 1000)).await;
    assert!(matches!(taken, Err(DiscountError::CodeTaken)), "{taken:?}");

    claimed_ago(
        cmd.executor(),
        &discount_code_id("WINTER"),
        &claimed,
        CLAIM_GRACE_SECS + 1,
    )
    .await;
    let winter = cmd
        .create_discount(ALICE, percent("WINTER", 1000))
        .await
        .expect("a node died between the claim and the discount: the code is free");
    assert_eq!(winter.code, "WINTER");
}
