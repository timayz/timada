mod common;

use common::*;
use timada_automations::{
    Command, ScenarioFilter, ScenarioStatus, count_scenarios, read_model_subscription,
    scenarios_of_tenant,
};

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone())
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
