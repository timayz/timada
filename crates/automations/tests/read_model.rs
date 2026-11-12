mod common;

use timada_core::db::Db;

use common::*;
use evento::ProjectionAggregate;
use timada_automations::{
    Command, Definition, ScenarioFilter, ScenarioStatus, catalog::WAIT, count_scenarios,
    published_definition, read_model_subscription, scenarios_of_tenant,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn only(status: ScenarioStatus) -> ScenarioFilter {
    ScenarioFilter {
        status: Some(status),
    }
}

#[tokio::test]
async fn scenarios_are_listed_by_status() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    let draft = automations
        .create_scenario(ALICE, "Draft one")
        .await
        .unwrap();
    let live = automations
        .create_scenario(ALICE, "Live one")
        .await
        .unwrap();
    let gone = automations
        .create_scenario(ALICE, "Gone one")
        .await
        .unwrap();
    automations
        .publish_scenario(ALICE, &live, 1, "Live one", &big_orders())
        .await
        .unwrap();
    automations.archive_scenario(ALICE, &gone).await.unwrap();

    catch_up(&executor, &pool).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &pool).await;

    let any = ScenarioFilter::default();
    // Deleted ones are not listed unless asked for.
    assert_eq!(count_scenarios(&pool, TENANT, &any).await.unwrap(), 2);
    let rows = scenarios_of_tenant(&pool, TENANT, &any, 10, 0)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == live).unwrap();
    assert_eq!(row.status(), ScenarioStatus::Active);
    assert_eq!(row.trigger.as_deref(), Some("order.placed"));
    assert_eq!(row.version, 1);
    assert_eq!(row.steps, 3);
    assert!(!row.unpublished);
    let row = rows.iter().find(|row| row.id == draft).unwrap();
    assert_eq!(row.status(), ScenarioStatus::Draft);
    assert_eq!(row.trigger, None);
    assert!(row.unpublished);

    let archived = scenarios_of_tenant(&pool, TENANT, &only(ScenarioStatus::Archived), 10, 0)
        .await
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, gone);
    // Another tenant sees nothing.
    assert_eq!(
        count_scenarios(&pool, "tenant-other", &any).await.unwrap(),
        0
    );
    // Paging.
    assert_eq!(
        scenarios_of_tenant(&pool, TENANT, &any, 1, 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_saved_draft_lists_its_trigger_and_steps() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    let id = automations
        .create_scenario(ALICE, "Draft one")
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    automations
        .save_scenario(ALICE, &id, 1, "Draft one", &big_orders())
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let rows = scenarios_of_tenant(&pool, TENANT, &ScenarioFilter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(rows[0].trigger.as_deref(), Some("order.placed"));
    assert_eq!(rows[0].steps, 3);
    assert_eq!(rows[0].status(), ScenarioStatus::Draft);
}

/// Every version keeps the definition it was published with, for the runs
/// started on it; the write model keeps the one running only.
#[tokio::test]
async fn every_version_is_kept_as_published() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    let id = automations
        .create_scenario(ALICE, "Big orders")
        .await
        .unwrap();
    let first = big_orders();
    automations
        .publish_scenario(ALICE, &id, 1, "Big orders", &first)
        .await
        .unwrap();
    // Saved many times, then published again.
    let mut second = first.clone();
    let condition = second.steps[0].id.clone();
    for n in 0..20 {
        second.insert(&branch(&condition, 1, n), WAIT).unwrap();
        let base = automations
            .load_scenario(&id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version()
            .unwrap();
        automations
            .save_scenario(ALICE, &id, base, "Big orders", &second)
            .await
            .unwrap();
    }
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    let base = state.aggregate_version().unwrap();
    automations
        .publish_scenario(ALICE, &id, base, "Big orders", &second)
        .await
        .unwrap();

    catch_up(&executor, &pool).await;
    // Delivery may repeat: the versions stay as published.
    catch_up(&executor, &pool).await;

    let read = |version: u32| {
        let pool = pool.clone();
        let id = id.clone();
        async move {
            published_definition(&pool, TENANT, &id, version)
                .await
                .unwrap()
                .map(|json| Definition::from_json(&json).unwrap())
        }
    };
    assert_eq!(read(1).await, Some(first));
    assert_eq!(read(2).await, Some(second.clone()));
    assert_eq!(read(3).await, None);
    assert_eq!(
        published_definition(&pool, "tenant-other", &id, 1)
            .await
            .unwrap(),
        None
    );
    let state = automations.load_scenario(&id).await.unwrap().unwrap();
    assert_eq!(state.version, 2);
    assert_eq!(state.live_definition().unwrap(), Some(second));
}
