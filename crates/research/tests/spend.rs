mod common;

use common::*;
use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_core::unix_now;
use timada_research::aggregator::{Charged, Held};
use timada_research::{
    Call, FetchStatus, RESEARCH_ACTOR, Room, SpendCap, SpendCaps, keywords_of_study, last_fetch,
    read_model_subscription, spend_by_user, spend_day, spend_day_ends, spend_in_all,
};

/// The most a call may cost, as the host says it.
const MOST: u64 = 132_000;

/// Room for two calls a user, three for everyone.
const CAPS: SpendCaps = SpendCaps {
    user_usd_micros: 2 * MOST,
    platform_usd_micros: 3 * MOST,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

async fn hold(research: &Research, user_id: &str, caps: SpendCaps) -> Room {
    research
        .hold_spend(
            RESEARCH_ACTOR,
            Call {
                user_id,
                study_id: "study",
                query: "griffoir",
                most_usd_micros: MOST,
            },
            caps,
        )
        .await
        .unwrap()
}

fn held(room: Room) -> timada_research::Hold {
    match room {
        Room::Held(hold) => hold,
        Room::Refused { cap, .. } => panic!("refused by {cap:?}"),
    }
}

#[tokio::test]
async fn a_call_is_held_charged_what_it_cost_and_counted_against_the_caps() {
    let research = command().await;
    let before = unix_now();
    let first = held(hold(&research, ALICE, CAPS).await);
    assert_eq!(first.day, spend_day(before));
    let second = held(hold(&research, ALICE, CAPS).await);

    // Two on their way hold all of Alice's day.
    let Room::Refused { cap, frees_at } = hold(&research, ALICE, CAPS).await else {
        panic!("a third call was held");
    };
    assert_eq!(cap, SpendCap::User);
    assert_eq!(frees_at, spend_day_ends(before));

    // Charged what they cost, far under their most, there is room again.
    for each in [&first, &second] {
        assert!(
            research
                .charge_spend(RESEARCH_ACTOR, each, 12_600)
                .await
                .unwrap()
        );
    }
    // A charge delivered twice counts once.
    assert!(
        !research
            .charge_spend(RESEARCH_ACTOR, &first, 12_600)
            .await
            .unwrap()
    );
    let day = research.load_spend_day(&first.day).await.unwrap();
    assert_eq!(day.charged_to(ALICE), 25_200);
    assert_eq!(day.charged(), 25_200);
    assert!(day.holds.is_empty());
    assert_eq!(day.users[ALICE].calls, 2);

    // A call that gave nothing gives its room back.
    let failed = held(hold(&research, ALICE, CAPS).await);
    assert!(
        research
            .release_spend(RESEARCH_ACTOR, &failed)
            .await
            .unwrap()
    );
    assert!(
        !research
            .release_spend(RESEARCH_ACTOR, &failed)
            .await
            .unwrap()
    );
    let day = research.load_spend_day(&first.day).await.unwrap();
    assert_eq!(day.charged_to(ALICE), 25_200);
    assert!(day.holds.is_empty());
    assert_eq!(day.held, 3);
}

#[tokio::test]
async fn the_platforms_cap_is_over_every_user() {
    let research = command().await;
    held(hold(&research, ALICE, CAPS).await);
    held(hold(&research, ALICE, CAPS).await);
    held(hold(&research, MALLORY, CAPS).await);
    // Mallory has room of her own left; the platform has none.
    let Room::Refused { cap, .. } = hold(&research, MALLORY, CAPS).await else {
        panic!("a fourth call was held");
    };
    assert_eq!(cap, SpendCap::Platform);
}

/// Two nodes ask for the last room of the day at once: the day's stream
/// takes one write, and the other decides again on what it left.
#[tokio::test]
async fn two_nodes_cannot_both_take_the_last_room() {
    let caps = SpendCaps {
        user_usd_micros: 10 * MOST,
        platform_usd_micros: MOST,
    };
    for _ in 0..10 {
        let research = command().await;
        let other = timada_research::Command(research.0.clone());
        let (one, two) = tokio::join!(hold(&research, ALICE, caps), hold(&other, MALLORY, caps));
        let taken = [&one, &two]
            .iter()
            .filter(|room| matches!(room, Room::Held(_)))
            .count();
        assert_eq!(taken, 1, "{one:?} {two:?}");
    }
}

/// Appends a charged call of `user_id` to the stream of `day`, at `version`.
async fn charge_on(executor: &evento::Turso, day: &str, version: u16, user_id: &str, cost: u64) {
    let hold = format!("{day}-{version}");
    evento::append(day)
        .original_version(version)
        .event(&Held {
            hold: hold.clone(),
            user_id: user_id.to_owned(),
            study_id: "study".to_owned(),
            query: "griffoir".to_owned(),
            most_usd_micros: MOST,
            held_by: RESEARCH_ACTOR.to_owned(),
        })
        .event(&Charged {
            hold,
            user_id: user_id.to_owned(),
            study_id: "study".to_owned(),
            query: "griffoir".to_owned(),
            cost_usd_micros: cost,
            charged_by: RESEARCH_ACTOR.to_owned(),
        })
        .commit(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_spend_reads_back_by_user_for_the_day_and_its_month() {
    let (executor, db) = executor_with_pool().await;
    charge_on(&executor, "2023-10-31", 0, ALICE, 99_000).await;
    charge_on(&executor, "2023-11-01", 0, ALICE, 10_000).await;
    charge_on(&executor, "2023-11-01", 2, MALLORY, 50_000).await;
    charge_on(&executor, "2023-11-14", 0, ALICE, 12_000).await;
    charge_on(&executor, "2023-11-14", 2, ALICE, 12_600).await;
    charge_on(&executor, "2023-11-15", 0, MALLORY, 77_000).await;

    catch_up(&executor, &db).await;

    let rows = spend_by_user(&db, "2023-11-14", 10).await.unwrap();
    let read: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.user_id.as_str(),
                row.day_usd_micros,
                row.day_calls,
                row.month_usd_micros,
                row.month_calls,
            )
        })
        .collect();
    assert_eq!(
        read,
        [(MALLORY, 0, 0, 50_000, 1), (ALICE, 24_600, 2, 34_600, 3)]
    );
    assert_eq!(
        spend_in_all(&db, "2023-11-14").await.unwrap(),
        (24_600, 84_600)
    );
    assert_eq!(spend_in_all(&db, "2024-01-01").await.unwrap(), (0, 0));
}

#[tokio::test]
async fn a_seed_fetched_not_long_ago_answers_another_study() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let first = study(&research, &["griffoir"]).await;
    fetch(
        &research,
        &first.id,
        "griffoir",
        vec![
            row("griffoir", Some(12_100)),
            row("griffoir carton", Some(880)),
        ],
    )
    .await;
    catch_up(&executor, &db).await;

    // Found by seed, country and language, whoever fetched it.
    let found = last_fetch(&db, "FR", "fr", "griffoir", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.study_id, first.id);
    for (country, language, seed) in [
        ("US", "fr", "griffoir"),
        ("FR", "en", "griffoir"),
        ("FR", "fr", "litière"),
    ] {
        assert_eq!(
            last_fetch(&db, country, language, seed, 0).await.unwrap(),
            None
        );
    }
    let since = found.fetched_at as u64;
    assert_eq!(
        last_fetch(&db, "FR", "fr", "griffoir", since + 1)
            .await
            .unwrap(),
        None,
        "too old"
    );
    let rows = research
        .fetched_rows(&first.id, "griffoir", since)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.rows.len(), 2);
    assert_eq!(rows.fetched_at, since);
    assert_eq!(
        research
            .fetched_rows(&first.id, "griffoir", since + 1)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        research
            .fetched_rows(&first.id, "litière", 0)
            .await
            .unwrap(),
        None
    );

    // Recorded on Mallory's study, at no cost and at the age it has.
    let theirs = research
        .create_study(MALLORY, draft("Griffoirs", &["griffoir"]))
        .await
        .unwrap();
    research
        .request_fetch(MALLORY, &theirs.id, None)
        .await
        .unwrap();
    assert!(
        research
            .start_fetch(RESEARCH_ACTOR, &theirs.id, "griffoir")
            .await
            .unwrap()
    );
    assert!(
        research
            .record_reuse(RESEARCH_ACTOR, &theirs.id, "griffoir", rows, &first.id)
            .await
            .unwrap()
    );
    let after = research.load_study(&theirs.id).await.unwrap().unwrap();
    assert_eq!(after.seeds[0].status, FetchStatus::Fetched);
    assert_eq!(after.seeds[0].fetched_at, Some(since));
    assert_eq!(after.seeds[0].keywords, 2);
    assert_eq!(after.cost_usd_micros, 0);

    // The first study deleted, its fetch still answers.
    research.delete_study(ALICE, &first.id).await.unwrap();
    catch_up(&executor, &db).await;
    let listed = keywords_of_study(&db, &theirs.id, PageFrom::First, 10)
        .await
        .unwrap()
        .rows;
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|row| row.fetched_at as u64 == since));
    // A reuse is no fetch: the one to reuse stays the paid one.
    let found = last_fetch(&db, "FR", "fr", "griffoir", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.study_id, first.id);
    assert!(
        research
            .fetched_rows(&first.id, "griffoir", since)
            .await
            .unwrap()
            .is_some()
    );
}
