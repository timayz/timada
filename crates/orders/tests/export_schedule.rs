mod common;

use common::*;
use jiff::{civil::Date, tz::TimeZone};
use timada_orders::{
    EXPORT_RETRY_DELAYS_SECS, EXPORT_RUN_STALE_SECS, EXPORT_RUNS_AFTER_SECS, ExportClock,
    ExportDestination, ExportEvery, ExportKind, ExportOutcome, MAX_EXPORT_ATTEMPTS,
    export_schedules_due, read_model_subscription,
};

const SENDER: &str = "timada-dashboard";

fn paris() -> TimeZone {
    TimeZone::get("Europe/Paris").unwrap()
}

fn day(text: &str) -> Date {
    text.parse().unwrap()
}

/// Unix seconds of a civil time in Paris (`2026-11-18T12:00`).
fn at(time: &str) -> u64 {
    time.parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned(paris())
        .unwrap()
        .timestamp()
        .as_second() as u64
}

/// That moment on a clock in Paris.
fn on(now: u64) -> ExportClock {
    ExportClock::new(now, paris())
}

fn by_mail() -> ExportDestination {
    ExportDestination::Mail(" Books@Example.org ".into())
}

/// The days of the run started at `now`, if one was.
async fn started(orders: &Orders, kind: ExportKind, now: u64) -> Option<(String, String, u32)> {
    orders
        .start_export_run(SENDER, kind, &on(now))
        .await
        .unwrap()
        .map(|run| {
            (
                run.first_day.to_string(),
                run.last_day.to_string(),
                run.attempt,
            )
        })
}

fn days(first: &str, last: &str, attempt: u32) -> Option<(String, String, u32)> {
    Some((first.to_owned(), last.to_owned(), attempt))
}

#[test]
fn a_period_is_a_whole_day_week_or_month_on_the_shops_clock() {
    // 2026-11-18 is a Wednesday.
    let wednesday = day("2026-11-18");
    assert_eq!(ExportEvery::Daily.start_of(wednesday), wednesday);
    assert_eq!(ExportEvery::Weekly.start_of(wednesday), day("2026-11-16"));
    assert_eq!(
        ExportEvery::Weekly.start_of(day("2026-11-16")),
        day("2026-11-16")
    );
    assert_eq!(
        ExportEvery::Weekly.start_of(day("2026-11-22")),
        day("2026-11-16")
    );
    assert_eq!(ExportEvery::Monthly.start_of(wednesday), day("2026-11-01"));
    assert_eq!(ExportEvery::Daily.next_start(wednesday), day("2026-11-19"));
    assert_eq!(ExportEvery::Weekly.next_start(wednesday), day("2026-11-23"));
    assert_eq!(
        ExportEvery::Monthly.next_start(day("2026-12-31")),
        day("2027-01-01")
    );
    // An hour after the period ended, where the shop is.
    assert_eq!(
        ExportEvery::Weekly.due_at(wednesday, &paris()),
        at("2026-11-23T01:00")
    );
    assert_eq!(
        ExportEvery::Weekly.due_at(wednesday, &TimeZone::UTC),
        at("2026-11-23T02:00")
    );
    assert_eq!(EXPORT_RUNS_AFTER_SECS, 3600);
    for kind in ExportKind::ALL {
        assert_eq!(ExportKind::parse(kind.key()), Some(kind));
    }
    for every in ExportEvery::ALL {
        assert_eq!(ExportEvery::parse(every.key()), Some(every));
    }
    assert_eq!(ExportKind::parse("invoices"), None);
}

#[tokio::test]
async fn an_export_goes_out_once_its_period_ended_for_the_days_since_the_last() {
    let (orders, _) = store().await;
    let kind = ExportKind::Orders;
    assert!(orders.load_export_schedule(kind).await.unwrap().is_none());
    assert_eq!(started(&orders, kind, at("2026-11-18T12:00")).await, None);

    // Scheduled on a Wednesday: the first file is that week's, whole.
    orders
        .schedule_export(
            ALICE,
            kind,
            ExportEvery::Weekly,
            by_mail(),
            &on(at("2026-11-18T12:00")),
        )
        .await
        .unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    let plan = state.plan.clone().unwrap();
    assert_eq!(plan.since, day("2026-11-16"));
    assert_eq!(
        plan.destination,
        ExportDestination::Mail("books@example.org".into())
    );
    assert_eq!(state.due_at, Some(at("2026-11-23T01:00")));
    assert_eq!(state.what, "orders");

    // The same again writes nothing.
    orders
        .schedule_export(
            ALICE,
            kind,
            ExportEvery::Weekly,
            by_mail(),
            &on(at("2026-11-19T12:00")),
        )
        .await
        .unwrap();
    assert_eq!(
        orders
            .load_export_schedule(kind)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        state.aggregate_version
    );

    // Nothing while the week runs.
    assert_eq!(started(&orders, kind, at("2026-11-22T23:59")).await, None);
    // Once it ended, one node starts it; another finds it taken.
    let now = at("2026-11-23T01:00");
    assert_eq!(
        started(&orders, kind, now).await,
        days("2026-11-16", "2026-11-22", 1)
    );
    assert_eq!(started(&orders, kind, now + 60).await, None);
    orders
        .settle_export_run(SENDER, kind, 1, ExportOutcome::Delivered, &on(now + 5))
        .await
        .unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    assert_eq!(state.covered_through, Some(day("2026-11-22")));
    assert_eq!(state.run, None);
    let ended = state.ended.clone().unwrap();
    assert_eq!(
        (ended.first_day, ended.last_day, ended.at, ended.failure),
        (day("2026-11-16"), day("2026-11-22"), now + 5, None)
    );
    assert_eq!(state.due_at, Some(at("2026-11-30T01:00")));
    // Settled twice changes nothing.
    orders
        .settle_export_run(
            SENDER,
            kind,
            1,
            ExportOutcome::Refused("late".into()),
            &on(now + 9),
        )
        .await
        .unwrap();
    assert_eq!(
        orders
            .load_export_schedule(kind)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        state.aggregate_version
    );
    assert_eq!(started(&orders, kind, at("2026-11-25T12:00")).await, None);

    // The nodes were away for two weeks: one file covers every day since.
    assert_eq!(
        started(&orders, kind, at("2026-12-09T08:00")).await,
        days("2026-11-23", "2026-12-06", 1)
    );

    // The other kinds are schedules of their own.
    assert!(
        orders
            .load_export_schedule(ExportKind::Customers)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_export_not_taken_is_tried_again_then_given_up() {
    let (orders, _) = store().await;
    let kind = ExportKind::Customers;
    orders
        .schedule_export(
            ALICE,
            kind,
            ExportEvery::Daily,
            ExportDestination::Url {
                sealed: "c2VhbGVk".into(),
            },
            &on(at("2026-11-18T12:00")),
        )
        .await
        .unwrap();
    let mut now = at("2026-11-19T01:00");
    assert_eq!(
        started(&orders, kind, now).await,
        days("2026-11-18", "2026-11-18", 1)
    );
    for attempt in 1..MAX_EXPORT_ATTEMPTS {
        orders
            .settle_export_run(
                SENDER,
                kind,
                attempt,
                ExportOutcome::Again("The endpoint answered 503.".into()),
                &on(now),
            )
            .await
            .unwrap();
        let delay = EXPORT_RETRY_DELAYS_SECS[attempt as usize - 1];
        let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
        assert_eq!(state.due_at, Some(now + delay));
        assert_eq!(state.covered_through, None);
        // Not before its time; then the same day again.
        assert_eq!(started(&orders, kind, now + delay - 1).await, None);
        now += delay;
        assert_eq!(
            started(&orders, kind, now).await,
            days("2026-11-18", "2026-11-18", attempt + 1)
        );
    }
    // Once too often: given up, and the schedule moves on.
    orders
        .settle_export_run(
            SENDER,
            kind,
            MAX_EXPORT_ATTEMPTS,
            ExportOutcome::Again("The endpoint\nanswered 503.".into()),
            &on(now),
        )
        .await
        .unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    assert_eq!(state.run, None);
    assert_eq!(state.covered_through, Some(day("2026-11-18")));
    assert_eq!(
        state.ended.clone().unwrap().failure.as_deref(),
        Some("The endpoint answered 503.")
    );
    assert_eq!(state.due_at, Some(at("2026-11-20T01:00")));

    // An address that says no is not asked again.
    let now = at("2026-11-20T01:00");
    assert_eq!(
        started(&orders, kind, now).await,
        days("2026-11-19", "2026-11-19", 1)
    );
    orders
        .settle_export_run(
            SENDER,
            kind,
            1,
            ExportOutcome::Refused("The endpoint answered 404.".into()),
            &on(now),
        )
        .await
        .unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    assert_eq!(state.covered_through, Some(day("2026-11-19")));
    assert!(state.ended.unwrap().failure.is_some());
}

#[tokio::test]
async fn a_run_never_settled_is_started_again_and_a_schedule_taken_off_sends_nothing() {
    let (orders, _) = store().await;
    let kind = ExportKind::Newsletter;
    orders
        .schedule_export(
            ALICE,
            kind,
            ExportEvery::Monthly,
            by_mail(),
            &on(at("2026-11-18T12:00")),
        )
        .await
        .unwrap();
    let now = at("2026-12-01T01:00");
    assert_eq!(
        started(&orders, kind, now).await,
        days("2026-11-01", "2026-11-30", 1)
    );
    // Its node stopped before saying how it went.
    assert_eq!(
        started(&orders, kind, now + EXPORT_RUN_STALE_SECS - 1).await,
        None
    );
    assert_eq!(
        started(&orders, kind, now + EXPORT_RUN_STALE_SECS).await,
        days("2026-11-01", "2026-11-30", 2)
    );
    // The first try's answer, come late, is not this run's.
    orders
        .settle_export_run(SENDER, kind, 1, ExportOutcome::Delivered, &on(now + 2000))
        .await
        .unwrap();
    assert!(
        orders
            .load_export_schedule(kind)
            .await
            .unwrap()
            .unwrap()
            .run
            .is_some()
    );

    // Changed, it starts over; taken off, it sends nothing.
    orders
        .schedule_export(
            ALICE,
            kind,
            ExportEvery::Daily,
            by_mail(),
            &on(at("2026-12-01T09:00")),
        )
        .await
        .unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    assert_eq!(state.run, None);
    assert_eq!(state.plan.clone().unwrap().since, day("2026-12-01"));
    orders.unschedule_export(ALICE, kind).await.unwrap();
    orders.unschedule_export(ALICE, kind).await.unwrap();
    let state = orders.load_export_schedule(kind).await.unwrap().unwrap();
    assert_eq!((state.plan.clone(), state.due_at), (None, None));
    assert_eq!(started(&orders, kind, at("2026-12-05T09:00")).await, None);

    // An address that is none is refused.
    for destination in [
        ExportDestination::Mail("not an address".into()),
        ExportDestination::Url {
            sealed: String::new(),
        },
    ] {
        let refused = orders
            .schedule_export(
                ALICE,
                kind,
                ExportEvery::Daily,
                destination,
                &on(at("2026-12-05T09:00")),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Some("invalid_export_address"));
    }
}

#[tokio::test]
async fn the_read_model_says_which_schedules_are_due() {
    let (executor, db) = executor_with_pool().await;
    let acme = timada_orders::Command::new(executor.clone(), TENANT);
    let globex = timada_orders::Command::new(executor.clone(), "globex");
    let scheduled = at("2026-11-18T12:00");
    acme.schedule_export(
        ALICE,
        ExportKind::Orders,
        ExportEvery::Daily,
        by_mail(),
        &on(scheduled),
    )
    .await
    .unwrap();
    acme.schedule_export(
        ALICE,
        ExportKind::Customers,
        ExportEvery::Monthly,
        by_mail(),
        &on(scheduled),
    )
    .await
    .unwrap();
    globex
        .schedule_export(
            ALICE,
            ExportKind::Orders,
            ExportEvery::Weekly,
            by_mail(),
            &on(scheduled),
        )
        .await
        .unwrap();
    let follow = || async {
        read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    let due = |now: u64| {
        let db = db.clone();
        async move {
            export_schedules_due(&db, now as i64, None, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|row| (row.tenant_id, row.what))
                .collect::<Vec<_>>()
        }
    };
    follow().await;
    assert_eq!(due(scheduled).await, []);
    assert_eq!(
        due(at("2026-11-19T01:00")).await,
        [(TENANT.to_owned(), "orders".to_owned())]
    );
    assert_eq!(
        due(at("2026-12-01T01:00")).await,
        [
            (TENANT.to_owned(), "orders".to_owned()),
            ("globex".to_owned(), "orders".to_owned()),
            (TENANT.to_owned(), "customers".to_owned()),
        ]
    );
    // A page at a time, from the one after the last seen.
    let first = export_schedules_due(&db, at("2026-12-01T01:00") as i64, None, 2)
        .await
        .unwrap();
    let rest = export_schedules_due(&db, at("2026-12-01T01:00") as i64, first.last(), 2)
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].what, "customers");

    // A run started moves the row to when it would be taken again; one
    // delivered, to the next day; a schedule taken off leaves.
    let now = at("2026-11-19T01:00");
    acme.start_export_run(SENDER, ExportKind::Orders, &on(now))
        .await
        .unwrap()
        .unwrap();
    follow().await;
    assert_eq!(due(now + 60).await, []);
    acme.settle_export_run(
        SENDER,
        ExportKind::Orders,
        1,
        ExportOutcome::Delivered,
        &on(now + 60),
    )
    .await
    .unwrap();
    globex
        .unschedule_export(ALICE, ExportKind::Orders)
        .await
        .unwrap();
    follow().await;
    assert_eq!(due(at("2026-11-20T00:59")).await, []);
    assert_eq!(
        due(at("2026-11-20T01:00")).await,
        [(TENANT.to_owned(), "orders".to_owned())]
    );
}
