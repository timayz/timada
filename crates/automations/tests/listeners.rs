mod common;

use common::*;
use timada_automations::{
    Command, ListenerState, RunFilter, RunStatus, StartRun, Transition, due_runs, listener_of,
    listener_subscription, listeners_for, prune_runs, read_model_subscription, run_id,
    runs_of_scenario,
};

async fn follow(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    listener_subscription::<evento::Sqlite>("test/listeners", pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_published_scenario_listens_to_its_trigger_until_held() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    let id = automations
        .create_scenario(ALICE, "Big orders")
        .await
        .unwrap();
    follow(&executor, &pool).await;
    // A draft starts nothing.
    assert!(
        listeners_for(&pool, TENANT, "order.placed")
            .await
            .unwrap()
            .is_empty()
    );

    automations
        .publish_scenario(ALICE, &id, 1, "Big orders", &big_orders())
        .await
        .unwrap();
    follow(&executor, &pool).await;
    // Delivery may repeat.
    follow(&executor, &pool).await;
    let listening = listeners_for(&pool, TENANT, "order.placed").await.unwrap();
    assert_eq!(listening.len(), 1);
    assert_eq!(listening[0].scenario_id, id);
    assert_eq!(listening[0].version, 1);
    assert!(
        listeners_for(&pool, "tenant-other", "order.placed")
            .await
            .unwrap()
            .is_empty()
    );

    automations.pause_scenario(ALICE, &id).await.unwrap();
    follow(&executor, &pool).await;
    assert!(
        listeners_for(&pool, TENANT, "order.placed")
            .await
            .unwrap()
            .is_empty()
    );
    let held = listener_of(&pool, TENANT, &id).await.unwrap().unwrap();
    assert_eq!(held.state(), ListenerState::Paused);

    automations.resume_scenario(ALICE, &id).await.unwrap();
    follow(&executor, &pool).await;
    let again = listeners_for(&pool, TENANT, "order.placed").await.unwrap();
    assert_eq!(again.len(), 1);
    assert!(again[0].since >= held.since);

    automations.archive_scenario(ALICE, &id).await.unwrap();
    follow(&executor, &pool).await;
    assert!(
        listeners_for(&pool, TENANT, "order.placed")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        listener_of(&pool, TENANT, &id)
            .await
            .unwrap()
            .unwrap()
            .state(),
        ListenerState::Archived
    );
}

#[tokio::test]
async fn runs_are_listed_and_owed_a_look_until_they_end() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    let catch_up = || async {
        read_model_subscription::<evento::Sqlite>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    let start = |event: &str| StartRun {
        scenario_id: "sc1".into(),
        scenario_version: 1,
        trigger: "order.placed".into(),
        trigger_event_id: event.into(),
        subject_id: "order-1".into(),
    };
    automations.start_run(ALICE, &start("ev1")).await.unwrap();
    automations.start_run(ALICE, &start("ev2")).await.unwrap();
    catch_up().await;
    let due = due_runs(&pool, now() + 1, 10).await.unwrap();
    assert_eq!(due.len(), 2);

    let id = run_id("sc1", "ev1");
    let run = automations.load_run(&id).await.unwrap().unwrap();
    let until = now() + 3600;
    automations
        .advance_run(
            ALICE,
            &run,
            &[Transition::Wait {
                step: "s1".into(),
                until,
            }],
        )
        .await
        .unwrap();
    let other = automations
        .load_run(&run_id("sc1", "ev2"))
        .await
        .unwrap()
        .unwrap();
    automations
        .advance_run(ALICE, &other, &[Transition::Finish])
        .await
        .unwrap();
    catch_up().await;
    catch_up().await;

    // The waiting one is owed a look once its wait is over, the other never.
    assert!(due_runs(&pool, now() + 1, 10).await.unwrap().is_empty());
    assert_eq!(
        due_runs(&pool, until, 10).await.unwrap(),
        vec![(TENANT.to_owned(), id.clone())]
    );
    let rows = runs_of_scenario(&pool, TENANT, "sc1", &RunFilter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let waiting = rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(waiting.status(), RunStatus::Waiting);
    assert_eq!(waiting.step.as_deref(), Some("s1"));
    let finished = runs_of_scenario(
        &pool,
        TENANT,
        "sc1",
        &RunFilter {
            status: Some(RunStatus::Finished),
        },
        10,
        0,
    )
    .await
    .unwrap();
    assert_eq!(finished.len(), 1);
    // Ended runs leave the list after a while; the others stay.
    assert_eq!(prune_runs(&pool, now() + 10).await.unwrap(), 1);
    assert_eq!(
        runs_of_scenario(&pool, TENANT, "sc1", &RunFilter::default(), 10, 0)
            .await
            .unwrap()
            .len(),
        1
    );
}
