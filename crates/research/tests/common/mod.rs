#![allow(dead_code)]

use timada_core::db::Db;
use timada_research::aggregator::KeywordRow;
use timada_research::{Command, CreateStudy, RESEARCH_ACTOR, StudyState};

pub type Research = Command<evento::Turso>;

pub const ALICE: &str = "user-alice";
pub const MALLORY: &str = "user-mallory";

/// A fresh in-memory event store with the read-model schema applied, and
/// the read-model database in it.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_research::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Research {
    Command(executor_with_pool().await.0)
}

pub fn draft(name: &str, seeds: &[&str]) -> CreateStudy {
    CreateStudy {
        name: name.to_owned(),
        seeds: seeds.iter().map(|seed| seed.to_string()).collect(),
        country: "fr".to_owned(),
        language: "FR".to_owned(),
    }
}

/// A study of Alice's with these seeds.
pub async fn study(research: &Research, seeds: &[&str]) -> StudyState {
    research
        .create_study(ALICE, draft("Arbres à chat", seeds))
        .await
        .unwrap()
}

pub fn row(keyword: &str, volume: Option<u64>) -> KeywordRow {
    KeywordRow {
        keyword: keyword.to_owned(),
        volume,
        cpc_usd_cents: Some(42),
        difficulty: Some(38),
        intent: Some("commercial".to_owned()),
    }
}

/// Asks for `seed`, takes the fetch on and records `rows` for it, as the
/// host's saga does.
pub async fn fetch(research: &Research, study_id: &str, seed: &str, rows: Vec<KeywordRow>) {
    research
        .request_fetch(ALICE, study_id, Some(seed))
        .await
        .unwrap();
    assert!(
        research
            .start_fetch(RESEARCH_ACTOR, study_id, seed)
            .await
            .unwrap()
    );
    assert!(
        research
            .record_keywords(RESEARCH_ACTOR, study_id, seed, rows, 12_600)
            .await
            .unwrap()
    );
}
