use evento::Executor;
use sqlx::SqlitePool;
use timada_core::Money;
use timada_payment::{
    Applied, CancelOutcome, Command, FakeProvider, ManualProvider, PaymentError, PaymentMethod,
    PaymentProvider, PaymentStart, PaymentStatus, PaymentView, ProviderError, ProviderEvent,
    RefundOutcome, RefundPass, RefundPolicy, RefundStatus, RequestPayment, ReturnUrls,
    apply_provider_event, cancel_payment_session, count_refund_requests, count_refunds,
    execute_pending_refunds, list_refund_requests, list_refunds, load_payment, migrations,
    payment_id, refund_execution_subscription, refund_list_subscription, start_payment,
};

async fn view<E: Executor>(executor: &E, id: &str) -> anyhow::Result<PaymentView> {
    load_payment(executor, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))
}

/// Enqueues the refunds asked for and hands them to the provider.
async fn run_refunds<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
) -> anyhow::Result<RefundPass> {
    refund_execution_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    Ok(execute_pending_refunds(executor, db, provider, &RefundPolicy::without_delays()).await?)
}

async fn captured_card_payment<E: Executor>(executor: &E, order: &str) -> anyhow::Result<String> {
    let cmd = Command(executor);
    let id = cmd
        .request_payment(RequestPayment {
            order_id: order.into(),
            amount: Money::eur(10_000),
            method: PaymentMethod::Card,
        })
        .await?;
    cmd.capture_payment(&id, "psp-123".into()).await?;
    Ok(id)
}

fn installments_request() -> RequestPayment {
    RequestPayment {
        order_id: "order-4112117449224J".into(),
        amount: Money::eur(12_496),
        method: PaymentMethod::Installments {
            count: 3,
            fee: Money::eur(449),
        },
    }
}

#[tokio::test]
async fn capture_then_partial_and_full_refund() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd.request_payment(installments_request()).await?;
    assert_eq!(id, payment_id("order-4112117449224J"));
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.status, PaymentStatus::Requested);
    assert_eq!(view.refunded, Money::eur(0));

    // A retry is idempotent: same id, no error, no second event.
    let again = cmd.request_payment(installments_request()).await?;
    assert_eq!(again, id);

    cmd.capture_payment(&id, "psp-123".into()).await?;

    let too_much = cmd
        .refund_payment(&id, Money::eur(20_000), "oops".into())
        .await;
    assert!(matches!(too_much, Err(PaymentError::RefundExceedsCapture)));

    // A refund is asked for first: nothing went back yet, but it is held.
    cmd.refund_payment(&id, Money::eur(2_496), "goodwill".into())
        .await?;
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.status, PaymentStatus::Captured);
    assert_eq!(view.psp_reference.as_deref(), Some("psp-123"));
    assert_eq!(view.refunded, Money::eur(0));
    assert_eq!(view.pending_refunds()?, Money::eur(2_496));
    assert_eq!(view.refundable()?, Money::eur(10_000));
    let held = cmd
        .refund_payment(
            &id,
            Money::eur(10_001),
            "too much with the pending one".into(),
        )
        .await;
    assert!(matches!(held, Err(PaymentError::RefundExceedsCapture)));

    // Without a provider the refund settles as soon as the worker passes.
    let pass = run_refunds(&executor, &db, &ManualProvider).await?;
    assert_eq!(pass.settled, 1);
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.refunded, Money::eur(2_496));
    assert_eq!(view.pending_refunds()?, Money::eur(0));
    assert_eq!(view.refunds[0].status, RefundStatus::Settled);
    assert!(
        view.refunds[0]
            .psp_refund_reference
            .as_deref()
            .is_some_and(|r| r.starts_with("manual-refund-"))
    );
    // A second pass has nothing left to do.
    assert_eq!(
        run_refunds(&executor, &db, &ManualProvider).await?,
        RefundPass::default()
    );

    cmd.refund_payment(&id, Money::eur(10_000), "returned".into())
        .await?;
    run_refunds(&executor, &db, &ManualProvider).await?;
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.status, PaymentStatus::Refunded);
    assert_eq!(view.refunded, Money::eur(12_496));

    // The admin listing has one row per refund; a redelivery adds nothing.
    for _ in 0..2 {
        refund_list_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await?;
    }
    let rows = list_refunds(&db, 50, 0).await?;
    assert_eq!(count_refunds(&db).await?, 2);
    let mut amounts: Vec<(i64, &str)> = rows
        .iter()
        .map(|r| (r.amount_minor, r.reason.as_str()))
        .collect();
    amounts.sort_unstable();
    assert_eq!(amounts, [(2_496, "goodwill"), (10_000, "returned")]);
    assert!(rows.iter().all(|r| r.order_id == "order-4112117449224J"));
    assert_eq!(count_refund_requests(&db, None).await?, 2);
    assert_eq!(
        count_refund_requests(&db, Some(RefundStatus::Settled)).await?,
        2
    );

    Ok(())
}

#[tokio::test]
async fn a_keyed_refund_is_only_made_once() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(&executor);
    let id = cmd.request_payment(installments_request()).await?;
    cmd.capture_payment(&id, "psp-123".into()).await?;

    assert!(
        cmd.refund_payment_once(&id, "return R2026-000001", Money::eur(5_000))
            .await?
    );
    // A process manager retrying after a crash refunds nothing more.
    assert!(
        !cmd.refund_payment_once(&id, "return R2026-000001", Money::eur(5_000))
            .await?
    );
    // Another reference is another refund.
    assert!(
        cmd.refund_payment_once(&id, "return R2026-000002", Money::eur(1_000))
            .await?
    );
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.pending_refunds()?, Money::eur(6_000));
    Ok(())
}

#[tokio::test]
async fn declined_payment_cannot_be_captured() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(&executor);

    let id = cmd
        .request_payment(RequestPayment {
            order_id: "order-4012101106824C".into(),
            amount: Money::eur(23_424),
            method: PaymentMethod::Card,
        })
        .await?;
    cmd.decline_payment(&id, "insufficient funds".into())
        .await?;

    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.status, PaymentStatus::Declined);
    assert_eq!(view.declined_reason.as_deref(), Some("insufficient funds"));

    let capture = cmd.capture_payment(&id, "psp-999".into()).await;
    assert!(matches!(capture, Err(PaymentError::NotRequested)));

    let refund = cmd.refund_payment(&id, Money::eur(1), "n/a".into()).await;
    assert!(matches!(refund, Err(PaymentError::NotCaptured)));

    Ok(())
}

#[tokio::test]
async fn rejects_invalid_requests() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(&executor);

    let zero = cmd
        .request_payment(RequestPayment {
            order_id: "order-zero".into(),
            amount: Money::eur(0),
            method: PaymentMethod::Card,
        })
        .await;
    assert!(matches!(zero, Err(PaymentError::InvalidAmount)));

    let five_times = cmd
        .request_payment(RequestPayment {
            order_id: "order-5x".into(),
            amount: Money::eur(100),
            method: PaymentMethod::Installments {
                count: 5,
                fee: Money::eur(1),
            },
        })
        .await;
    assert!(matches!(five_times, Err(PaymentError::InvalidMethod(_))));

    let mixed = cmd
        .request_payment(RequestPayment {
            order_id: "order-usd-fee".into(),
            amount: Money::eur(100),
            method: PaymentMethod::Installments {
                count: 3,
                fee: Money::new(1, "USD"),
            },
        })
        .await;
    assert!(matches!(mixed, Err(PaymentError::Money(_))));

    Ok(())
}

#[tokio::test]
async fn a_refused_refund_fails_and_can_be_retried_or_settled_by_hand() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let id = captured_card_payment(&executor, "order-refused").await?;
    let provider = FakeProvider::default();

    let refund = cmd
        .refund_payment(&id, Money::eur(10_000), "order cancelled".into())
        .await?;
    provider.answer_refund(Err(ProviderError::Refused("charge disputed".into())));
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!(pass.failed, 1);

    let payment = view(&executor, &id).await?;
    assert_eq!(payment.refunded, Money::eur(0));
    assert_eq!(payment.refunds[0].status, RefundStatus::Failed);
    assert_eq!(
        payment.refunds[0].failure.as_deref(),
        Some("charge disputed")
    );
    // Nothing is held for a failed refund…
    assert_eq!(payment.refundable()?, Money::eur(10_000));
    // …but its reference stays taken: a process manager never asks twice.
    assert!(
        !cmd.refund_payment_once(&id, "order cancelled", Money::eur(10_000))
            .await?
    );

    refund_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let failed = list_refund_requests(&db, Some(RefundStatus::Failed), 10, 0).await?;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].failure.as_deref(), Some("charge disputed"));
    assert_eq!(count_refunds(&db).await?, 0);

    // The operator asks again: same refund, a fresh idempotency key.
    assert!(matches!(
        cmd.retry_refund(&id, "unknown").await,
        Err(PaymentError::RefundNotFound)
    ));
    cmd.retry_refund(&id, &refund).await?;
    assert!(matches!(
        cmd.retry_refund(&id, &refund).await,
        Err(PaymentError::RefundNotFailed)
    ));
    provider.answer_refund(Err(ProviderError::Refused("still disputed".into())));
    run_refunds(&executor, &db, &provider).await?;
    let asked = provider.refunds();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].psp_reference, "psp-123");
    assert_ne!(asked[0].idempotency_key, asked[1].idempotency_key);

    // The money went back by bank transfer: settled by hand.
    assert!(
        cmd.settle_refund(&id, &refund, "virement 42".into())
            .await?
    );
    assert!(
        !cmd.settle_refund(&id, &refund, "virement 42".into())
            .await?
    );
    let payment = view(&executor, &id).await?;
    assert_eq!(payment.status, PaymentStatus::Refunded);
    assert_eq!(payment.refunds.len(), 1);
    assert_eq!(payment.refunds[0].status, RefundStatus::Settled);
    assert!(matches!(
        cmd.fail_refund(&id, &refund, "late".into()).await,
        Err(PaymentError::RefundAlreadySettled)
    ));

    refund_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    assert_eq!(count_refunds(&db).await?, 1);
    assert_eq!(
        count_refund_requests(&db, Some(RefundStatus::Settled)).await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn an_unavailable_provider_is_retried_then_given_up_on() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let id = captured_card_payment(&executor, "order-outage").await?;
    let provider = FakeProvider::default();
    Command(&executor)
        .refund_payment(&id, Money::eur(1_000), "goodwill".into())
        .await?;

    provider.answer_refund(Err(ProviderError::Unavailable("timeout".into())));
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!(pass.postponed, 1);
    assert_eq!(
        view(&executor, &id).await?.refunds[0].status,
        RefundStatus::Pending
    );
    // Back up: the same key goes out again and the refund settles.
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!(pass.settled, 1);
    let asked = provider.refunds();
    assert_eq!(asked[0].idempotency_key, asked[1].idempotency_key);

    // An outage that outlasts the schedule ends as a failed refund.
    Command(&executor)
        .refund_payment(&id, Money::eur(500), "second".into())
        .await?;
    let attempts = RefundPolicy::default().retry_delays.len() + 1;
    let mut last = RefundPass::default();
    for _ in 0..attempts {
        provider.answer_refund(Err(ProviderError::Unavailable("down".into())));
        last = run_refunds(&executor, &db, &provider).await?;
    }
    assert_eq!(last.failed, 1);
    let payment = view(&executor, &id).await?;
    assert_eq!(payment.refunds[1].status, RefundStatus::Failed);
    assert_eq!(payment.refunded, Money::eur(1_000));
    Ok(())
}

#[tokio::test]
async fn a_pending_refund_settles_when_the_provider_reports_it() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let id = captured_card_payment(&executor, "order-async-refund").await?;
    let provider = FakeProvider::default();
    Command(&executor)
        .refund_payment(&id, Money::eur(4_000), "return R2026-000001".into())
        .await?;
    Command(&executor)
        .refund_payment(&id, Money::eur(1_000), "return R2026-000002".into())
        .await?;

    provider.answer_refund(Ok(RefundOutcome::Pending {
        reference: "re_1".into(),
    }));
    provider.answer_refund(Ok(RefundOutcome::Pending {
        reference: "re_2".into(),
    }));
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!(pass.awaiting, 2);
    assert_eq!(view(&executor, &id).await?.refunded, Money::eur(0));

    let settled = ProviderEvent::RefundSettled {
        provider_reference: "re_1".into(),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, settled.clone()).await?,
        Applied::Done
    );
    // Providers deliver at least once.
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, settled).await?,
        Applied::Ignored
    );
    let failed = ProviderEvent::RefundFailed {
        provider_reference: "re_2".into(),
        reason: "card expired".into(),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, failed).await?,
        Applied::Done
    );
    let unknown = ProviderEvent::RefundSettled {
        provider_reference: "re_other_shop".into(),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, unknown).await?,
        Applied::Ignored
    );

    let payment = view(&executor, &id).await?;
    assert_eq!(payment.refunded, Money::eur(4_000));
    assert_eq!(
        payment.refunds[0].psp_refund_reference.as_deref(),
        Some("re_1")
    );
    assert_eq!(payment.refunds[1].status, RefundStatus::Failed);
    Ok(())
}

#[tokio::test]
async fn the_provider_reporting_a_payment_captures_it_once() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let provider = FakeProvider::default();
    let id = Command(&executor)
        .request_payment(RequestPayment {
            order_id: "order-webhook".into(),
            amount: Money::eur(10_000),
            method: PaymentMethod::Card,
        })
        .await?;
    let paid = ProviderEvent::Paid {
        payment_id: id.clone(),
        reference: "pi_1".into(),
        amount: Money::eur(10_000),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, paid.clone()).await?,
        Applied::Done
    );
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, paid).await?,
        Applied::Ignored
    );
    let payment = view(&executor, &id).await?;
    assert_eq!(payment.status, PaymentStatus::Captured);
    assert_eq!(payment.psp_reference.as_deref(), Some("pi_1"));
    assert!(provider.refunds().is_empty());

    let stranger = ProviderEvent::Paid {
        payment_id: "not-ours".into(),
        reference: "pi_x".into(),
        amount: Money::eur(1),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, stranger).await?,
        Applied::Ignored
    );
    Ok(())
}

#[tokio::test]
async fn money_for_a_payment_that_timed_out_is_sent_back() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let provider = FakeProvider::default();
    let cmd = Command(&executor);
    let id = cmd
        .request_payment(RequestPayment {
            order_id: "order-late".into(),
            amount: Money::eur(10_000),
            method: PaymentMethod::Card,
        })
        .await?;
    cmd.decline_payment(&id, "PAYMENT_TIMED_OUT".into()).await?;

    let late = ProviderEvent::Paid {
        payment_id: id.clone(),
        reference: "pi_late".into(),
        amount: Money::eur(10_000),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, late).await?,
        Applied::SentBack
    );
    assert_eq!(view(&executor, &id).await?.status, PaymentStatus::Declined);
    let sent_back = provider.refunds();
    assert_eq!(sent_back.len(), 1);
    assert_eq!(sent_back[0].psp_reference, "pi_late");
    assert_eq!(sent_back[0].amount, Money::eur(10_000));

    // The wrong amount is never captured either.
    let other = cmd
        .request_payment(RequestPayment {
            order_id: "order-short".into(),
            amount: Money::eur(10_000),
            method: PaymentMethod::Card,
        })
        .await?;
    let short = ProviderEvent::Paid {
        payment_id: other.clone(),
        reference: "pi_short".into(),
        amount: Money::eur(9_000),
    };
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, short).await?,
        Applied::SentBack
    );
    assert_eq!(
        view(&executor, &other).await?.status,
        PaymentStatus::Requested
    );
    Ok(())
}

#[tokio::test]
async fn a_payment_session_is_reused_and_can_be_called_off() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let urls = ReturnUrls {
        paid: "https://shop.test/checkout/pay/1".into(),
    };
    let id = cmd
        .request_payment(RequestPayment {
            order_id: "order-session".into(),
            amount: Money::eur(10_000),
            method: PaymentMethod::Card,
        })
        .await?;

    assert_eq!(
        start_payment(&executor, &db, &ManualProvider, &id, &urls).await?,
        PaymentStart::Manual
    );
    let provider = FakeProvider::card_only();
    assert!(provider.supports(&PaymentMethod::Card));
    assert!(!provider.supports(&PaymentMethod::Installments {
        count: 3,
        fee: Money::eur(0)
    }));
    let first = start_payment(&executor, &db, &provider, &id, &urls).await?;
    let again = start_payment(&executor, &db, &provider, &id, &urls).await?;
    assert_eq!(first, again);
    assert!(matches!(first, PaymentStart::Redirect(url) if url.contains(&id)));

    // Too late to call it off: the shopper paid.
    let session = FakeProvider::session_of(&id);
    provider.mark_paid(&session);
    assert!(matches!(
        cancel_payment_session(&db, &provider, &id).await?,
        CancelOutcome::AlreadyPaid { .. }
    ));

    let other = cmd
        .request_payment(RequestPayment {
            order_id: "order-session-2".into(),
            amount: Money::eur(500),
            method: PaymentMethod::Card,
        })
        .await?;
    // No session was ever opened: nothing to cancel.
    assert_eq!(
        cancel_payment_session(&db, &provider, &other).await?,
        CancelOutcome::Cancelled
    );
    start_payment(&executor, &db, &provider, &other, &urls).await?;
    assert_eq!(
        cancel_payment_session(&db, &provider, &other).await?,
        CancelOutcome::Cancelled
    );
    assert_eq!(provider.cancelled(), [FakeProvider::session_of(&other)]);

    // A payment that is no longer awaited cannot be started.
    cmd.decline_payment(&other, "PAYMENT_TIMED_OUT".into())
        .await?;
    assert!(matches!(
        start_payment(&executor, &db, &provider, &other, &urls).await,
        Err(PaymentError::NotRequested)
    ));
    Ok(())
}
