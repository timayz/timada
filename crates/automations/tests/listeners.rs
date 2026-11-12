mod common;

use timada_core::Share;
use timada_core::db::Db;
use timada_core::page::PageFrom;

use common::*;
use timada_automations::{
    Command, DueRun, ListenerState, RunFilter, RunStatus, StartRun, Transition, count_runs,
    due_runs, listener_of, listener_subscription, listeners_for, owe_start, prune_runs,
    prune_starts, put_off_run, read_model_subscription, run_id, run_known, runs_of_scenario,
    settle_start, starts_owed,
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
    let due = due_runs(&pool, now() + 1, Share::ALONE, 0, None, 10)
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
        due_runs(&pool, now() + 1, Share::ALONE, 0, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        due_runs(&pool, until, Share::ALONE, 0, None, 10)
            .await
            .unwrap(),
        vec![DueRun {
            tenant_id: TENANT.to_owned(),
            id: id.clone(),
            next_at: until as i64,
        }]
    );
    let rows = runs_of_scenario(
        &pool,
        TENANT,
        "sc1",
        &RunFilter::default(),
        PageFrom::First,
        10,
    )
    .await
    .unwrap()
    .rows;
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
        PageFrom::First,
        10,
    )
    .await
    .unwrap()
    .rows;
    assert_eq!(finished.len(), 1);
    assert_eq!(
        count_runs(&pool, TENANT, "sc1", &RunFilter::default())
            .await
            .unwrap(),
        2
    );
    // Ended runs leave the list after a while, and its counts; the others
    // stay.
    assert_eq!(prune_runs(&pool, now() + 10, 100).await.unwrap(), 1);
    let rows = runs_of_scenario(
        &pool,
        TENANT,
        "sc1",
        &RunFilter::default(),
        PageFrom::First,
        10,
    )
    .await
    .unwrap()
    .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        count_runs(&pool, TENANT, "sc1", &RunFilter::default())
            .await
            .unwrap(),
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
        let mine = due_runs(&pool, later, share, 0, None, 100).await.unwrap();
        assert!(!mine.is_empty(), "node {index} has a part");
        assert!(mine.iter().all(|run| share.owns(&run.id)));
        seen.extend(mine.into_iter().map(|run| run.id));
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 30, "every run falls to one node");

    // Due since before `help_before`: any node takes it.
    let all = due_runs(&pool, later, Share { index: 0, of: 3 }, later, None, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 30);
}

/// A look walks the runs due page after page, each from where the one
/// before stopped, and leaves the runs it put off out of the next looks
/// until their time.
#[tokio::test]
async fn the_runs_due_are_walked_and_a_run_put_off_waits() {
    let (executor, pool) = executor_with_pool().await;
    let automations = Command::new(executor.clone(), TENANT);
    for n in 0..5 {
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
    let at = now() + 1;
    let all = due_runs(&pool, at, Share::ALONE, 0, None, 10)
        .await
        .unwrap();
    assert_eq!(all.len(), 5);
    let first = due_runs(&pool, at, Share::ALONE, 0, None, 2).await.unwrap();
    let second = due_runs(&pool, at, Share::ALONE, 0, first.last(), 2)
        .await
        .unwrap();
    let third = due_runs(&pool, at, Share::ALONE, 0, second.last(), 2)
        .await
        .unwrap();
    let walked: Vec<DueRun> = [first, second, third].concat();
    assert_eq!(walked, all, "every run once, in order");

    // Put off: not due before then, on this node.
    let held = &all[0];
    put_off_run(&pool, held, at + 300).await.unwrap();
    let due = due_runs(&pool, at, Share::ALONE, 0, None, 10)
        .await
        .unwrap();
    assert_eq!(due, all[1..].to_vec());
    let later = due_runs(&pool, at + 300, Share::ALONE, 0, None, 10)
        .await
        .unwrap();
    assert_eq!(later.len(), 5);
    assert_eq!(later[4].id, held.id);

    // A row written since the walk read it (the run moved) is left as the
    // read model wrote it.
    let moved = &all[1];
    let mut run = automations.load_run(&moved.id).await.unwrap().unwrap();
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
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    put_off_run(&pool, moved, at + 300).await.unwrap();
    let row = runs_of_scenario(
        &pool,
        TENANT,
        "sc1",
        &RunFilter::default(),
        PageFrom::First,
        10,
    )
    .await
    .unwrap()
    .rows
    .into_iter()
    .find(|row| row.id == moved.id)
    .unwrap();
    assert_eq!(row.next_at, Some(until as i64));
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
