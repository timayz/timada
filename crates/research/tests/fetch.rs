mod common;

use common::*;
use timada_research::aggregator::KeywordRow;
use timada_research::{
    FETCH_STALE_SECS, FetchStatus, MAX_FETCH_ROWS, RESEARCH_ACTOR, ResearchError, SeedState,
};

#[tokio::test]
async fn a_fetch_is_asked_for_taken_on_once_and_recorded() {
    let research = command().await;
    let study = study(&research, &["griffoir", "arbre à chat"]).await;

    // Without a seed, every seed that wants its keywords is asked for.
    assert_eq!(
        research
            .request_fetch(ALICE, &study.id, None)
            .await
            .unwrap(),
        2
    );
    let asked = research.load_study(&study.id).await.unwrap().unwrap();
    assert!(
        asked
            .seeds
            .iter()
            .all(|seed| seed.status == FetchStatus::Requested && seed.requested_at > 0)
    );
    // Asked again while on its way: one seed is refused, all of them leave
    // nothing to ask for.
    let err = research
        .request_fetch(ALICE, &study.id, Some("Griffoir"))
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::Fetching), "{err}");
    let err = research
        .request_fetch(ALICE, &study.id, None)
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::NothingToFetch), "{err}");
    let err = research
        .request_fetch(ALICE, &study.id, Some("litière"))
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::UnknownSeed), "{err}");

    // The first node takes it on; the second finds it taken.
    assert!(
        research
            .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
            .await
            .unwrap()
    );
    assert!(
        !research
            .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
            .await
            .unwrap()
    );
    let taken = research.load_study(&study.id).await.unwrap().unwrap();
    assert_eq!(taken.seeds[0].status, FetchStatus::Fetching);
    assert_eq!(taken.seeds[1].status, FetchStatus::Requested);

    assert!(
        research
            .record_keywords(
                RESEARCH_ACTOR,
                &study.id,
                "griffoir",
                vec![
                    row("griffoir", Some(12_100)),
                    row("griffoir carton", None),
                    row("  ", Some(1)),
                ],
                12_240,
            )
            .await
            .unwrap()
    );
    let fetched = research.load_study(&study.id).await.unwrap().unwrap();
    assert_eq!(fetched.seeds[0].status, FetchStatus::Fetched);
    assert_eq!(fetched.seeds[0].keywords, 2, "a blank keyword is no row");
    assert!(fetched.seeds[0].fetched_at.is_some());
    assert_eq!(fetched.cost_usd_micros, 12_240);
    assert_eq!(fetched.keywords(), 2);

    // Fetched, it is no longer owed, and may be fetched again later.
    assert_eq!(
        research
            .request_fetch(ALICE, &study.id, Some("griffoir"))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn a_failure_is_kept_with_its_reason_and_asked_again() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    fetch(
        &research,
        &study.id,
        "griffoir",
        vec![row("griffoir", Some(10))],
    )
    .await;

    research
        .request_fetch(ALICE, &study.id, Some("griffoir"))
        .await
        .unwrap();
    research
        .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
        .await
        .unwrap();
    assert!(
        research
            .record_fetch_failure(
                RESEARCH_ACTOR,
                &study.id,
                "griffoir",
                "The keyword source refused: Insufficient Funds.",
            )
            .await
            .unwrap()
    );
    let failed = research.load_study(&study.id).await.unwrap().unwrap();
    let seed = &failed.seeds[0];
    assert_eq!(seed.status, FetchStatus::Failed);
    assert_eq!(
        seed.last_error.as_deref(),
        Some("The keyword source refused: Insufficient Funds.")
    );
    // What the earlier fetch gave stays.
    assert_eq!(seed.keywords, 1);
    assert!(seed.fetched_at.is_some());
    // A failure with no fetch on its way records nothing.
    assert!(
        !research
            .record_fetch_failure(RESEARCH_ACTOR, &study.id, "griffoir", "late")
            .await
            .unwrap()
    );

    // Failed, it is owed again: asking for all asks for it, and the request
    // clears the reason.
    assert_eq!(
        research
            .request_fetch(ALICE, &study.id, None)
            .await
            .unwrap(),
        1
    );
    let asked = research.load_study(&study.id).await.unwrap().unwrap();
    assert_eq!(asked.seeds[0].status, FetchStatus::Requested);
    assert_eq!(asked.seeds[0].last_error, None);
}

#[tokio::test]
async fn a_fetch_records_a_thousand_rows_at_most_as_the_crate_reads_them() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    let mut rows: Vec<KeywordRow> = (0..MAX_FETCH_ROWS + 50)
        .map(|n| row(&format!("griffoir {n}"), Some(n as u64)))
        .collect();
    rows[0] = KeywordRow {
        keyword: format!(" griffoir\n{} ", "x".repeat(400)),
        volume: Some(1),
        cpc_usd_cents: None,
        difficulty: Some(250),
        intent: Some("curious".to_owned()),
    };
    fetch(&research, &study.id, "griffoir", rows).await;
    let fetched = research.load_study(&study.id).await.unwrap().unwrap();
    assert_eq!(fetched.seeds[0].keywords as usize, MAX_FETCH_ROWS);

    let recorded = evento::read::<timada_research::aggregator::Study>(&study.id)
        .decode(&research.0)
        .await
        .unwrap()
        .into_iter()
        .find_map(|event| match event {
            timada_research::aggregator::StudyEvent::KeywordsFetched(fetched) => Some(fetched),
            _ => None,
        })
        .unwrap();
    let first = &recorded.rows[0];
    assert_eq!(first.keyword.chars().count(), 200);
    assert!(first.keyword.starts_with("griffoir xxx"));
    assert_eq!(first.difficulty, Some(100));
    assert_eq!(first.intent, None);
}

#[tokio::test]
async fn a_fetch_of_a_study_that_is_gone_records_nothing() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    research
        .request_fetch(ALICE, &study.id, None)
        .await
        .unwrap();
    research
        .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
        .await
        .unwrap();
    research.delete_study(ALICE, &study.id).await.unwrap();
    assert!(
        !research
            .record_keywords(
                RESEARCH_ACTOR,
                &study.id,
                "griffoir",
                vec![row("griffoir", Some(1))],
                12_120,
            )
            .await
            .unwrap()
    );
    assert!(
        !research
            .record_fetch_failure(RESEARCH_ACTOR, &study.id, "griffoir", "gone")
            .await
            .unwrap()
    );
    assert!(
        !research
            .start_fetch(RESEARCH_ACTOR, &study.id, "griffoir")
            .await
            .unwrap()
    );
}

/// The owner and the nodes write one stream at once: a write that lost to
/// another is decided again, so none of them is dropped.
#[tokio::test]
async fn writes_that_cross_on_a_study_all_land() {
    let research = command().await;
    let study = study(&research, &["one", "two", "three"]).await;
    research
        .request_fetch(ALICE, &study.id, None)
        .await
        .unwrap();
    let id = study.id.as_str();
    let (one, two, three, renamed, added) = tokio::join!(
        research.start_fetch(RESEARCH_ACTOR, id, "one"),
        research.start_fetch(RESEARCH_ACTOR, id, "two"),
        research.start_fetch(RESEARCH_ACTOR, id, "three"),
        research.rename_study(ALICE, id, "Renamed"),
        research.add_seed(ALICE, id, "four"),
    );
    assert!(one.unwrap() && two.unwrap() && three.unwrap());
    renamed.unwrap();
    added.unwrap();
    let after = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(after.name, "Renamed");
    assert_eq!(after.seeds.len(), 4);
    assert!(
        after.seeds[..3]
            .iter()
            .all(|seed| seed.status == FetchStatus::Fetching)
    );
}

#[test]
fn a_fetch_left_there_may_be_asked_again_once_stale() {
    let asked = SeedState {
        seed: "griffoir".to_owned(),
        status: FetchStatus::Fetching,
        requested_at: 1_000,
        ..SeedState::default()
    };
    assert!(asked.is_fetching(1_000 + FETCH_STALE_SECS - 1));
    assert!(!asked.is_owed(1_000 + FETCH_STALE_SECS - 1));
    assert!(!asked.is_fetching(1_000 + FETCH_STALE_SECS));
    assert!(asked.is_owed(1_000 + FETCH_STALE_SECS));

    let fetched = SeedState {
        status: FetchStatus::Fetched,
        ..asked.clone()
    };
    assert!(!fetched.is_fetching(1_001));
    assert!(
        !fetched.is_owed(1_000 + FETCH_STALE_SECS),
        "it has its keywords"
    );
    let never = SeedState::default();
    assert!(never.is_owed(1_000) && !never.is_fetching(1_000));
}
