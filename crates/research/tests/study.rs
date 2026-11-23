mod common;

use common::*;
use timada_research::{FetchStatus, MAX_SEEDS, ResearchError};

#[tokio::test]
async fn a_study_starts_with_its_seeds_and_belongs_to_who_made_it() {
    let research = command().await;
    let study = research
        .create_study(
            ALICE,
            draft(
                "  Arbres  à chat ",
                &["Arbre à Chat", "", "griffoir", "arbre  à chat"],
            ),
        )
        .await
        .unwrap();
    assert_eq!(study.name, "Arbres à chat");
    assert_eq!(study.country, "FR");
    assert_eq!(study.language, "fr");
    assert_eq!(study.owner, ALICE);
    assert_eq!(
        study
            .seeds
            .iter()
            .map(|seed| seed.seed.as_str())
            .collect::<Vec<_>>(),
        ["arbre à chat", "griffoir"]
    );
    // Nothing is fetched until the owner asks.
    assert!(
        study
            .seeds
            .iter()
            .all(|seed| seed.status == FetchStatus::Never)
    );
    assert_eq!(study.keywords(), 0);

    // Nobody else opens it: it reads as one that does not exist.
    let err = research
        .require_owned(&study.id, MALLORY)
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::NotFound), "{err}");
    assert_eq!(
        research.require_owned(&study.id, ALICE).await.unwrap(),
        study
    );
}

#[tokio::test]
async fn a_study_needs_a_name_a_place_and_a_seed() {
    let research = command().await;
    let err = research
        .create_study(ALICE, draft(" ", &["griffoir"]))
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::InvalidName), "{err}");
    let err = research
        .create_study(ALICE, draft("Griffoirs", &[" ", ""]))
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::NoSeed), "{err}");
    let mut nowhere = draft("Griffoirs", &["griffoir"]);
    nowhere.country = "France".to_owned();
    let err = research.create_study(ALICE, nowhere).await.unwrap_err();
    assert!(matches!(err, ResearchError::InvalidPlace), "{err}");
    assert_eq!(err.code(), Some("invalid_study_place"));
    assert!(err.is_user_facing());
    assert!(!err.is_store_fault());
}

#[tokio::test]
async fn its_owner_renames_it_and_adds_seeds() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;

    research
        .rename_study(ALICE, &study.id, " Griffoirs ")
        .await
        .unwrap();
    research
        .add_seed(ALICE, &study.id, "Arbre à chat")
        .await
        .unwrap();
    // The same again changes nothing.
    research
        .rename_study(ALICE, &study.id, "Griffoirs")
        .await
        .unwrap();
    research
        .add_seed(ALICE, &study.id, "arbre  à CHAT")
        .await
        .unwrap();
    let after = research.load_study(&study.id).await.unwrap().unwrap();
    assert_eq!(after.name, "Griffoirs");
    assert_eq!(after.seeds.len(), 2);
    assert_eq!(after.seeds[1].seed, "arbre à chat");
    assert_eq!(after.seeds[1].status, FetchStatus::Never);

    let err = research
        .rename_study(ALICE, &study.id, "")
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::InvalidName), "{err}");

    // Somebody else's rename, seed or deletion finds no study.
    for err in [
        research
            .rename_study(MALLORY, &study.id, "Mine")
            .await
            .unwrap_err(),
        research
            .add_seed(MALLORY, &study.id, "litière")
            .await
            .unwrap_err(),
        research.delete_study(MALLORY, &study.id).await.unwrap_err(),
        research
            .request_fetch(MALLORY, &study.id, None)
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(err, ResearchError::NotFound), "{err}");
    }
    assert_eq!(
        research.load_study(&study.id).await.unwrap().unwrap(),
        after
    );
}

#[tokio::test]
async fn a_study_holds_twenty_seeds_at_most() {
    let research = command().await;
    let seeds: Vec<String> = (0..MAX_SEEDS).map(|n| format!("seed {n}")).collect();
    let seeds: Vec<&str> = seeds.iter().map(String::as_str).collect();
    let study = study(&research, &seeds).await;
    let err = research
        .add_seed(ALICE, &study.id, "one more")
        .await
        .unwrap_err();
    assert!(matches!(err, ResearchError::TooManySeeds), "{err}");
    // One it holds already is still a no-op.
    research.add_seed(ALICE, &study.id, "seed 3").await.unwrap();
}

#[tokio::test]
async fn a_deleted_study_is_gone() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    research.delete_study(ALICE, &study.id).await.unwrap();
    assert!(research.load_study(&study.id).await.unwrap().is_none());
    for err in [
        research.delete_study(ALICE, &study.id).await.unwrap_err(),
        research
            .rename_study(ALICE, &study.id, "Back")
            .await
            .unwrap_err(),
        research
            .request_fetch(ALICE, &study.id, None)
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(err, ResearchError::NotFound), "{err}");
    }
}
