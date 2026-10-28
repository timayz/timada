mod common;

use common::*;
use timada_automations::{
    AutomationError, Command, Outcome, RunStatus, StartRun, Transition,
    interpreter::{self, Next},
    run_id,
};

fn start(scenario: &str, event: &str) -> StartRun {
    StartRun {
        scenario_id: scenario.into(),
        scenario_version: 1,
        trigger: "order.placed".into(),
        trigger_event_id: event.into(),
        subject_id: "order-1".into(),
    }
}

#[tokio::test]
async fn an_event_starts_a_scenario_once() {
    let automations = command().await;
    assert!(
        automations
            .start_run(ALICE, &start("sc1", "ev1"))
            .await
            .unwrap()
    );
    // Seen again (another node, a redelivery): nothing more.
    assert!(
        !automations
            .start_run(ALICE, &start("sc1", "ev1"))
            .await
            .unwrap()
    );
    // Another scenario on the same event is another run.
    assert!(
        automations
            .start_run(ALICE, &start("sc2", "ev1"))
            .await
            .unwrap()
    );
    let run = automations
        .load_run(&run_id("sc1", "ev1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.subject_id, "order-1");
    assert_eq!(run.next_at(), Some(run.updated_at));
}

#[tokio::test]
async fn a_run_moved_on_by_another_node_is_not_moved_twice() {
    let automations = command().await;
    automations
        .start_run(ALICE, &start("sc1", "ev1"))
        .await
        .unwrap();
    let id = run_id("sc1", "ev1");
    let run = automations.load_run(&id).await.unwrap().unwrap();
    let claim = [Transition::Claim {
        step: "s1".into(),
        attempt: 1,
    }];
    assert!(automations.advance_run(ALICE, &run, &claim).await.unwrap());
    // The same version again: refused, nothing written.
    assert!(!automations.advance_run(BOB, &run, &claim).await.unwrap());
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.attempts("s1"), 1);
    assert_eq!(
        run.latest("s1").unwrap().outcome,
        Outcome::Claimed { attempt: 1 }
    );
    // An outcome and the end, in one commit.
    assert!(
        automations
            .advance_run(
                ALICE,
                &run,
                &[
                    Transition::Succeed {
                        step: "s1".into(),
                        output: "{}".into()
                    },
                    Transition::Finish
                ]
            )
            .await
            .unwrap()
    );
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Finished);
    assert_eq!(run.next_at(), None);
}

#[tokio::test]
async fn a_step_failing_for_good_ends_the_run_and_a_retry_does_not() {
    let automations = command().await;
    automations
        .start_run(ALICE, &start("sc1", "ev1"))
        .await
        .unwrap();
    let id = run_id("sc1", "ev1");
    let run = automations.load_run(&id).await.unwrap().unwrap();
    automations
        .advance_run(
            ALICE,
            &run,
            &[Transition::Fail {
                step: "s1".into(),
                attempt: 1,
                reason: "the server said no".into(),
                retry_at: Some(9_999_999_999),
            }],
        )
        .await
        .unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.next_at(), Some(9_999_999_999));
    automations
        .advance_run(
            ALICE,
            &run,
            &[Transition::Fail {
                step: "s1".into(),
                attempt: 2,
                reason: "the server said no again".into(),
                retry_at: None,
            }],
        )
        .await
        .unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.reason.as_deref(), Some("the server said no again"));
    assert_eq!(run.next_at(), None);
}

#[tokio::test]
async fn a_member_cancels_a_run_under_way() {
    let automations = command().await;
    automations
        .start_run(ALICE, &start("sc1", "ev1"))
        .await
        .unwrap();
    let id = run_id("sc1", "ev1");
    assert!(
        automations
            .cancel_run(BOB, &id, "Not this one")
            .await
            .unwrap()
    );
    // Twice is harmless.
    assert!(
        !automations
            .cancel_run(BOB, &id, "Not this one")
            .await
            .unwrap()
    );
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Cancelled);
    assert!(matches!(
        automations.cancel_run(BOB, "nope", "?").await,
        Err(AutomationError::RunNotFound)
    ));
}

/// The interpreter walks the version a run started on against its log.
#[tokio::test]
async fn a_run_walks_its_scenario() {
    let automations: Automations = Command::new(executor_with_pool().await.0, TENANT);
    let definition = big_orders();
    let condition = &definition.steps[0];
    let notify = &condition.branches[0][0];
    let wait = &condition.branches[1][0];
    let now = now();

    // Yes: the notice, then the end.
    automations
        .start_run(ALICE, &start("sc1", "ev1"))
        .await
        .unwrap();
    let id = run_id("sc1", "ev1");
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(
        interpreter::next(&definition, &run, now),
        Next::Branch(condition)
    );
    let branch = [Transition::Branch {
        step: condition.id.clone(),
        branch: 0,
    }];
    automations.advance_run(ALICE, &run, &branch).await.unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(interpreter::next(&definition, &run, now), Next::Act(notify));
    let claim = [Transition::Claim {
        step: notify.id.clone(),
        attempt: 1,
    }];
    automations.advance_run(ALICE, &run, &claim).await.unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    let claimed_at = run.latest(&notify.id).unwrap().at;
    let lapses = claimed_at + timada_automations::CLAIM_LEASE_SECS;
    // Another node acting on it: wait for the lease, then call it lost.
    assert_eq!(
        interpreter::next(&definition, &run, now),
        Next::Sleep(lapses)
    );
    assert_eq!(
        interpreter::next(&definition, &run, lapses),
        Next::Lapsed(notify, 1)
    );
    let done = [Transition::Succeed {
        step: notify.id.clone(),
        output: "{}".into(),
    }];
    automations.advance_run(ALICE, &run, &done).await.unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(interpreter::next(&definition, &run, now), Next::Finish);

    // No: a wait of a day, then the end.
    automations
        .start_run(ALICE, &start("sc1", "ev2"))
        .await
        .unwrap();
    let id = run_id("sc1", "ev2");
    let run = automations.load_run(&id).await.unwrap().unwrap();
    let branch = [Transition::Branch {
        step: condition.id.clone(),
        branch: 1,
    }];
    automations.advance_run(ALICE, &run, &branch).await.unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    let until = now + 24 * 60 * 60;
    assert_eq!(
        interpreter::next(&definition, &run, now),
        Next::Wait(wait, until)
    );
    let waiting = [Transition::Wait {
        step: wait.id.clone(),
        until,
    }];
    automations
        .advance_run(ALICE, &run, &waiting)
        .await
        .unwrap();
    let run = automations.load_run(&id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Waiting);
    assert_eq!(run.next_at(), Some(until));
    assert_eq!(
        interpreter::next(&definition, &run, now + 60),
        Next::Sleep(until)
    );
    assert_eq!(interpreter::next(&definition, &run, until), Next::Finish);
}

#[test]
fn a_block_this_build_does_not_know_is_stuck() {
    let definition = timada_automations::Definition::from_json(
        r#"{"trigger":{"kind":"order.placed"},"steps":[{"id":"s1","kind":"teleport"}],"next_id":1}"#,
    )
    .unwrap();
    let run = timada_automations::RunState::default();
    assert_eq!(
        interpreter::next(&definition, &run, 0),
        Next::Stuck(&definition.steps[0], interpreter::STUCK_UNKNOWN)
    );
}
