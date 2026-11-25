use timada_launch::{
    Command, D21_GO, FIX, GO_ON, LAUNCH_ACTOR, LaunchError, Reading, STOP, checkpoint_ahead,
    checkpoint_at, checkpoint_due,
};

const ALICE: &str = "user-alice";
const SHOP: &str = "tenant-shop";
const DAY: u64 = 86_400;
/// 2026-11-25.
const NOW: u64 = 1_795_564_800;

async fn command() -> Command<evento::Turso> {
    Command::new(evento::Turso::in_memory_concurrent().await.unwrap(), SHOP)
}

#[tokio::test]
async fn a_target_is_set_and_a_change_is_kept_beside_the_one_before() {
    let launches = command().await;
    assert!(launches.load_launch().await.unwrap().is_none());
    launches
        .set_target(ALICE, 300_000, "eur", "2027-06-30", NOW)
        .await
        .unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    let target = launch.target().unwrap();
    assert_eq!(
        (
            target.monthly_profit_cents,
            target.currency.as_str(),
            target.by_day.as_str()
        ),
        (300_000, "EUR", "2027-06-30")
    );
    assert_eq!(target.set_by, ALICE);
    // The same again writes nothing.
    launches
        .set_target(ALICE, 300_000, "EUR", "2027-06-30", NOW)
        .await
        .unwrap();
    let again = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, launch.aggregate_version);

    launches
        .set_target("user-bob", 200_000, "EUR", "2027-09-30", NOW)
        .await
        .unwrap();
    let changed = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(changed.targets.len(), 2);
    assert_eq!(changed.targets[0].monthly_profit_cents, 300_000);
    assert_eq!(changed.target().unwrap().monthly_profit_cents, 200_000);
    assert_eq!(changed.target().unwrap().set_by, "user-bob");

    for (cents, currency, day) in [(0, "EUR", "2027-06-30"), (300_000, "euros", "2027-06-30")] {
        assert!(matches!(
            launches.set_target(ALICE, cents, currency, day, NOW).await,
            Err(LaunchError::InvalidTarget)
        ));
    }
    for day in ["soon", "2026-11-25", "2020-01-01", "2027-6-30"] {
        assert!(
            matches!(
                launches.set_target(ALICE, 300_000, "EUR", day, NOW).await,
                Err(LaunchError::InvalidTargetDay)
            ),
            "{day}"
        );
    }
}

#[tokio::test]
async fn a_checkpoint_is_read_once_judged_on_the_shops_thresholds_and_decided() {
    let launches = command().await;
    let started = NOW;
    // Nothing is due before day 21, then each in its turn.
    assert_eq!(checkpoint_due(None, started, started + 20 * DAY), None);
    assert_eq!(checkpoint_due(None, started, started + 21 * DAY), Some(21));
    assert_eq!(
        checkpoint_ahead(started, started),
        Some((21, checkpoint_at(started, 21)))
    );
    assert_eq!(
        checkpoint_ahead(started, started + 50 * DAY),
        Some((90, checkpoint_at(started, 90)))
    );
    assert_eq!(checkpoint_ahead(started, started + 120 * DAY), None);
    // A decision waits for the checkpoint.
    assert!(matches!(
        launches.record_decision(ALICE, 21, GO_ON, "").await,
        Err(LaunchError::NotReached)
    ));

    let reading = Reading::Day21 {
        return_hundredths: Some(85),
        clicks: 640,
    };
    assert!(
        launches
            .record_checkpoint(LAUNCH_ACTOR, started, reading)
            .await
            .unwrap()
    );
    // Read by another node too: once.
    assert!(
        !launches
            .record_checkpoint(LAUNCH_ACTOR, started, reading)
            .await
            .unwrap()
    );
    let launch = launches.load_launch().await.unwrap().unwrap();
    let reached = launch.reached(21).unwrap();
    assert_eq!(reached.advice, GO_ON);
    assert_eq!(reached.started_at, started);
    assert_eq!(
        reached.figures,
        vec![
            (
                "Return on ad spend, as Google Ads counts it".to_owned(),
                "0.85".to_owned()
            ),
            ("Clicks".to_owned(), "640".to_owned()),
        ]
    );
    assert_eq!(reached.decision, None);
    // The next one due is day 45, whenever the look comes.
    assert_eq!(
        checkpoint_due(Some(&launch), started, started + 200 * DAY),
        Some(45)
    );

    // The merchant's own word, and why; said again, the last stands.
    launches
        .record_decision(ALICE, 21, FIX, " The prices are too high. ")
        .await
        .unwrap();
    launches
        .record_decision(ALICE, 21, GO_ON, "")
        .await
        .unwrap();
    let decided = launches.load_launch().await.unwrap().unwrap();
    let decision = decided.reached(21).unwrap().decision.clone().unwrap();
    assert_eq!(
        (decision.decision.as_str(), decision.note.as_str()),
        (GO_ON, "")
    );
    assert_eq!(decision.decided_by, ALICE);
    assert!(matches!(
        launches.record_decision(ALICE, 21, "pause", "").await,
        Err(LaunchError::InvalidDecision)
    ));
    assert!(matches!(
        launches.record_decision(ALICE, 30, GO_ON, "").await,
        Err(LaunchError::UnknownCheckpoint)
    ));

    // A shop that raised its bar reads the next one against it; what was
    // read stays as it was read.
    launches
        .set_thresholds(ALICE, vec![(D21_GO.to_owned(), "0.90".to_owned())])
        .await
        .unwrap();
    assert!(matches!(
        launches
            .set_thresholds(ALICE, vec![(D21_GO.to_owned(), "0.10".to_owned())])
            .await,
        Err(LaunchError::InvalidThreshold)
    ));
    let launch = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(launch.thresholds().d21_go, 90);
    assert_eq!(launch.reached(21).unwrap().advice, GO_ON);

    // The campaign started again elsewhere: its checkpoints are read
    // again, in place of the old ones.
    let restarted = started + 30 * DAY;
    assert_eq!(
        checkpoint_due(Some(&launch), restarted, restarted + 21 * DAY),
        Some(21)
    );
    launches
        .record_checkpoint(LAUNCH_ACTOR, restarted, reading)
        .await
        .unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(launch.reached.len(), 1);
    let again = launch.reached(21).unwrap();
    // 0.85 is under the bar the shop set since.
    assert_eq!(again.advice, FIX);
    assert_eq!(again.decision, None);
    let poor = Reading::Day120 {
        margin_bps: Some(100),
        margin_before_bps: Some(300),
    };
    launches
        .record_checkpoint(LAUNCH_ACTOR, restarted, poor)
        .await
        .unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(launch.reached(120).unwrap().advice, STOP);
}

#[tokio::test]
async fn a_member_says_a_step_holds_and_takes_it_back() {
    let launches = command().await;
    assert!(matches!(
        launches.confirm_step(ALICE, "feed", true).await,
        Err(LaunchError::UnknownStep)
    ));
    // Nothing to take back.
    launches
        .confirm_step(ALICE, timada_launch::STEP_SPEED, false)
        .await
        .unwrap();
    assert!(launches.load_launch().await.unwrap().is_none());
    launches
        .confirm_step(ALICE, timada_launch::STEP_SPEED, true)
        .await
        .unwrap();
    launches
        .confirm_step("user-bob", timada_launch::STEP_SPEED, true)
        .await
        .unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    // The first word stands until it is taken back.
    assert_eq!(
        launch.confirmed(timada_launch::STEP_SPEED).unwrap().0,
        ALICE
    );
    assert_eq!(launch.confirmed(timada_launch::STEP_LEGAL), None);
    launches
        .confirm_step("user-bob", timada_launch::STEP_SPEED, false)
        .await
        .unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(launch.confirmed(timada_launch::STEP_SPEED), None);

    assert_eq!(launch.path_targets(), (10, 200));
    launches.set_path_targets(ALICE, 5, 60).await.unwrap();
    let launch = launches.load_launch().await.unwrap().unwrap();
    assert_eq!(launch.path_targets(), (5, 60));
    assert!(matches!(
        launches.set_path_targets(ALICE, 0, 60).await,
        Err(LaunchError::InvalidPathTarget)
    ));
}
