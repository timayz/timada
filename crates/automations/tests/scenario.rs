mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_automations::{
    AutomationError, Definition, ScenarioStatus,
    catalog::{NOTIFY, WAIT},
};

#[tokio::test]
async fn a_scenario_is_drafted_then_published_as_numbered_versions() {
    let automations = command().await;
    let id = automations
        .create_scenario(ALICE, "  Big orders ")
        .await
        .unwrap();
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.name, "Big orders");
    assert_eq!(state.status, ScenarioStatus::Draft);
    assert_eq!(state.definition().unwrap(), Definition::default());

    let definition = big_orders();
    let base = state.aggregate_version().unwrap();
    assert!(
        automations
            .save_scenario(ALICE, &id, base, "Big orders", &definition)
            .await
            .unwrap()
    );
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.definition().unwrap(), definition);
    assert!(state.has_unpublished_changes());
    // The same copy again: nothing to write.
    let base = state.aggregate_version().unwrap();
    assert!(
        !automations
            .save_scenario(ALICE, &id, base, "Big orders", &definition)
            .await
            .unwrap()
    );

    let version = automations
        .publish_scenario(ALICE, &id, base, "Large orders", &definition)
        .await
        .unwrap();
    assert_eq!(version, 1);
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.status, ScenarioStatus::Active);
    assert_eq!(state.name, "Large orders");
    assert_eq!(state.live_definition().unwrap(), Some(definition.clone()));
    assert!(!state.has_unpublished_changes());

    // A change saved after publishing waits for the next version.
    let mut changed = definition.clone();
    changed.insert(&root(1), WAIT).unwrap();
    let base = state.aggregate_version().unwrap();
    automations
        .save_scenario(ALICE, &id, base, "Large orders", &changed)
        .await
        .unwrap();
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert!(state.has_unpublished_changes());
    assert_eq!(state.live_definition().unwrap(), Some(definition));
    let base = state.aggregate_version().unwrap();
    assert_eq!(
        automations
            .publish_scenario(ALICE, &id, base, "Large orders", &changed)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn a_copy_made_before_someone_elses_save_is_stale() {
    let automations = command().await;
    let id = automations.create_scenario(ALICE, "Welcome").await.unwrap();
    let base = automations
        .load_scenario(&id)
        .await
        .unwrap()
        .unwrap()
        .aggregate_version()
        .unwrap();
    let mut alices = Definition::default();
    alices.insert(&root(0), WAIT).unwrap();
    let mut bobs = Definition::default();
    bobs.insert(&root(0), NOTIFY).unwrap();

    automations
        .save_scenario(BOB, &id, base, "Welcome", &bobs)
        .await
        .unwrap();
    let refused = automations
        .save_scenario(ALICE, &id, base, "Welcome", &alices)
        .await;
    assert!(matches!(refused, Err(AutomationError::Changed)));
    let refused = automations
        .publish_scenario(ALICE, &id, base, "Welcome", &big_orders())
        .await;
    assert!(matches!(refused, Err(AutomationError::Changed)));
}

#[tokio::test]
async fn only_a_finished_scenario_is_published() {
    let automations = command().await;
    let id = automations
        .create_scenario(ALICE, "Unfinished")
        .await
        .unwrap();
    let mut definition = Definition::default();
    definition.insert(&root(0), NOTIFY).unwrap();
    // A draft may be unfinished.
    automations
        .save_scenario(ALICE, &id, 1, "Unfinished", &definition)
        .await
        .unwrap();
    match automations
        .publish_scenario(ALICE, &id, 2, "Unfinished", &definition)
        .await
    {
        Err(AutomationError::Invalid(problems)) => assert!(!problems.is_empty()),
        other => panic!("published an unfinished scenario: {other:?}"),
    }
    let refused = automations.create_scenario(ALICE, "   ").await;
    assert!(matches!(refused, Err(AutomationError::InvalidName)));
}

#[tokio::test]
async fn pausing_resuming_and_deleting() {
    let automations = command().await;
    let id = automations
        .create_scenario(ALICE, "Big orders")
        .await
        .unwrap();
    // Never published: nothing to pause.
    assert!(matches!(
        automations.pause_scenario(ALICE, &id).await,
        Err(AutomationError::NotPublished)
    ));
    automations
        .publish_scenario(ALICE, &id, 1, "Big orders", &big_orders())
        .await
        .unwrap();
    automations.pause_scenario(ALICE, &id).await.unwrap();
    // Twice is harmless.
    automations.pause_scenario(ALICE, &id).await.unwrap();
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.status, ScenarioStatus::Paused);
    automations.resume_scenario(ALICE, &id).await.unwrap();
    automations.resume_scenario(ALICE, &id).await.unwrap();
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.status, ScenarioStatus::Active);

    automations.archive_scenario(ALICE, &id).await.unwrap();
    automations.archive_scenario(ALICE, &id).await.unwrap();
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.status, ScenarioStatus::Archived);
    let base = state.aggregate_version().unwrap();
    assert!(matches!(
        automations
            .save_scenario(ALICE, &id, base, "Big orders", &big_orders())
            .await,
        Err(AutomationError::Archived)
    ));
    assert!(matches!(
        automations.resume_scenario(ALICE, &id).await,
        Err(AutomationError::Archived)
    ));
    assert!(matches!(
        automations
            .load_scenario("nope")
            .await
            .map(|state| state.is_none()),
        Ok(true)
    ));
}
