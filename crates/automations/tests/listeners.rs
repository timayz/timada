mod common;

use timada_core::Share;
use timada_core::db::Db;

use common::*;
use timada_automations::{
    Command, ListenerState, RunFilter, RunStatus, StartRun, Transition, due_runs, listener_of,
    listener_subscription, listeners_for, owe_start, prune_runs, prune_starts,
    read_model_subscription, run_id, run_known, runs_of_scenario, settle_start, starts_owed,
};

async fn follow(executor: &evento::Turso, pool: &Db) {
    listener_subscription::<evento::Turso>(pool.clone())
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
        read_model_subscription::<evento::Turso>(pool.clone())
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
    let due = due_runs(&pool, now() + 1, Share::ALONE, 0, 10)
        .await
        .unwrap();
    assert_eq!(due.len(), 2);

    let id = run_id("sc1", "ev1");
    let mut run = automations.load_run(&id).await.unwrap().unwrap();
    let until = now() + 3600;
    automations
        .advance_run(
            ALICE,
            &mut run,
            &[Transition::Wait {
                step: "s1".into(),
                until,
            }],
        )
        .await
        .unwrap();
    let mut other = automations
        .load_run(&run_id("sc1", "ev2"))
        .await
        .unwrap()
        .unwrap();
    automations
        .advance_run(ALICE, &mut other, &[Transition::Finish])
        .await
        .unwrap();
    catch_up().await;
    catch_up().await;

    // The waiting one is owed a look once its wait is over, the other never.
    assert!(
        due_runs(&pool, now() + 1, Share::ALONE, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        due_runs(&pool, until, Share::ALONE, 0, 10).await.unwrap(),
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

/// Three nodes up: each looks at its own part of the runs due, and at any
/// run its node left for a while.
#[tokio::test]
async fn the_nodes_share_out_the_runs_due() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    for n in 0..30 {
        automations
            .start_run(
                ALICE,
                &StartRun {
                    scenario_id: "sc1".into(),
                    scenario_version: 1,
                    trigger: "order.placed".into(),
                    trigger_event_id: format!("ev{n}"),
                    subject_id: format!("order-{n}"),
                },
            )
            .await
            .unwrap();
    }
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    let later = now() + 60;
    let mut seen = Vec::new();
    for index in 0..3 {
        let share = Share { index, of: 3 };
        let mine = due_runs(&pool, later, share, 0, 100).await.unwrap();
        assert!(!mine.is_empty(), "node {index} has a part");
        assert!(mine.iter().all(|(_, id)| share.owns(id)));
        seen.extend(mine);
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 30, "every run falls to one node");

    // Due since before `help_before`: any node takes it.
    let all = due_runs(&pool, later, Share { index: 0, of: 3 }, later, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 30);
}

/// A run another node is to start is kept until it is started or too old.
#[tokio::test]
async fn a_start_left_to_another_node_is_kept_until_settled() {
    let (executor, pool) = executor_with_pool().await;
    let start = StartRun {
        scenario_id: "sc1".into(),
        scenario_version: 2,
        trigger: "order.placed".into(),
        trigger_event_id: "ev1".into(),
        subject_id: "order-1".into(),
    };
    let id = run_id("sc1", "ev1");
    let at = now();
    owe_start(&pool, TENANT, &id, &start, at).await.unwrap();
    // Seen again (a redelivery): kept once, as it was.
    owe_start(&pool, TENANT, &id, &start, at + 5).await.unwrap();
    assert!(starts_owed(&pool, at - 1, 10).await.unwrap().is_empty());
    let owed = starts_owed(&pool, at, 10).await.unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].start(), start);
    assert_eq!(owed[0].kept_at, at as i64);

    // Not started yet as far as the read model knows.
    assert!(!run_known(&pool, TENANT, &id).await.unwrap());
    Command::new(executor.clone(), TENANT)
        .start_run(ALICE, &start)
        .await
        .unwrap();
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert!(run_known(&pool, TENANT, &id).await.unwrap());
    assert!(!run_known(&pool, "tenant-other", &id).await.unwrap());
    settle_start(&pool, TENANT, &id).await.unwrap();
    assert!(starts_owed(&pool, at, 10).await.unwrap().is_empty());

    // One kept too long ago is dropped.
    owe_start(&pool, TENANT, &id, &start, at).await.unwrap();
    assert_eq!(prune_starts(&pool, at).await.unwrap(), 0);
    assert_eq!(prune_starts(&pool, at + 1).await.unwrap(), 1);
    assert!(starts_owed(&pool, at, 10).await.unwrap().is_empty());
}
