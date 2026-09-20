use evento::Executor;
use sqlx::SqlitePool;
use timada_core::Money;
use timada_payment::{
    Applied, CancelOutcome, Command, DisputeStanding, DisputeStatus, FakeProvider, ManualProvider,
    OpenDispute, PaymentError, PaymentMethod, PaymentProvider, PaymentStart, PaymentStatus,
    PaymentView, ProviderDispute, ProviderError, ProviderEvent, RefundOutcome, RefundPass,
    RefundPolicy, RefundStatus, RequestPayment, ReturnUrls, apply_provider_event,
    cancel_payment_session, count_disputes, count_refund_requests, count_refunds,
    dispute_list_subscription, disputes_of_order, execute_pending_refunds, list_disputes,
    list_refund_requests, list_refunds, load_payment, migrations, orders_with_open_dispute,
    payment_by_reference, payment_id, refund_execution_subscription, refund_list_subscription,
    start_payment,
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

/// What the provider says of a dispute on the payment captured as `psp-123`.
fn dispute(reference: &str, amount: i64, standing: DisputeStanding) -> ProviderEvent {
    ProviderEvent::Dispute(ProviderDispute {
        payment_reference: "psp-123".into(),
        reference: reference.into(),
        amount: Money::eur(amount),
        reason: "product_not_received".into(),
        respond_by: Some(1_800_000_000),
        standing,
    })
}

async fn learn_disputes<E: Executor>(executor: &E, db: &SqlitePool) -> anyhow::Result<()> {
    dispute_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    // The refunds list is strict: it must know the dispute events too.
    refund_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_dispute_holds_refunds_until_the_bank_sides_with_the_shop() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let provider = FakeProvider::default();

    // A provider talks about its own reference: unknown until the capture
    // was seen, and never for somebody else's payment.
    let early = dispute("dp_1", 10_000, DisputeStanding::Open);
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, early.clone()).await?,
        Applied::Ignored
    );
    let id = captured_card_payment(&executor, "order-disputed").await?;
    learn_disputes(&executor, &db).await?;
    assert_eq!(
        payment_by_reference(&db, "psp-123").await?.as_deref(),
        Some(id.as_str())
    );

    assert_eq!(
        apply_provider_event(&executor, &db, &provider, early.clone()).await?,
        Applied::Done
    );
    // Reported again — evidence updated, funds withdrawn: nothing new.
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, early).await?,
        Applied::Ignored
    );
    let payment = view(&executor, &id).await?;
    let open = payment
        .open_dispute()
        .ok_or_else(|| anyhow::anyhow!("no open dispute"))?;
    assert_eq!(open.dispute_id, "dp_1");
    assert_eq!(open.amount, Money::eur(10_000));
    assert_eq!(open.respond_by, Some(1_800_000_000));
    assert_eq!(
        timada_payment::dispute_reason_label(&open.reason),
        "produit non reçu"
    );
    // Still the shop's money until the bank says otherwise.
    assert_eq!(payment.refundable()?, Money::eur(10_000));

    learn_disputes(&executor, &db).await?;
    assert_eq!(count_disputes(&db, Some(DisputeStatus::Open)).await?, 1);
    assert_eq!(
        orders_with_open_dispute(&db, &["order-disputed".into(), "order-other".into()]).await?,
        ["order-disputed"]
    );

    // A refund can be decided, but nothing reaches the provider meanwhile.
    Command(&executor)
        .refund_payment(&id, Money::eur(4_000), "return R2026-000001".into())
        .await?;
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!((pass.held, pass.settled), (1, 0));
    assert!(provider.refunds().is_empty());
    assert_eq!(view(&executor, &id).await?.refunded, Money::eur(0));

    // Won: the refund that waited goes through.
    assert_eq!(
        apply_provider_event(
            &executor,
            &db,
            &provider,
            dispute("dp_1", 10_000, DisputeStanding::Won)
        )
        .await?,
        Applied::Done
    );
    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!((pass.held, pass.settled), (0, 1));
    let payment = view(&executor, &id).await?;
    assert!(payment.open_dispute().is_none());
    assert_eq!(payment.disputes[0].status, DisputeStatus::Won);
    assert!(payment.disputes[0].closed_at.is_some());
    assert_eq!(payment.refunded, Money::eur(4_000));
    assert_eq!(payment.charged_back()?, Money::eur(0));

    learn_disputes(&executor, &db).await?;
    let rows = list_disputes(&db, None, 10, 0).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "won");
    assert!(rows[0].closed_at.is_some());
    assert!(
        orders_with_open_dispute(&db, &["order-disputed".into()])
            .await?
            .is_empty()
    );
    // A bank does not change its mind; a provider that says so is refused.
    assert!(matches!(
        Command(&executor).lose_dispute(&id, "dp_1").await,
        Err(PaymentError::DisputeAlreadyClosed)
    ));
    Ok(())
}

#[tokio::test]
async fn a_lost_dispute_takes_its_amount_out_of_what_can_be_refunded() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let provider = FakeProvider::default();
    let cmd = Command(&executor);

    // Only captured money can be disputed.
    let requested = cmd
        .request_payment(RequestPayment {
            order_id: "order-unpaid".into(),
            amount: Money::eur(5_000),
            method: PaymentMethod::Card,
        })
        .await?;
    let claim = |reference: &str, amount: i64| OpenDispute {
        dispute_id: reference.into(),
        amount: Money::eur(amount),
        reason: "fraudulent".into(),
        respond_by: None,
    };
    assert!(matches!(
        cmd.open_dispute(&requested, claim("dp_0", 5_000)).await,
        Err(PaymentError::NotCaptured)
    ));

    let id = captured_card_payment(&executor, "order-lost").await?;
    learn_disputes(&executor, &db).await?;
    assert!(matches!(
        cmd.open_dispute(&id, claim("dp_big", 10_001)).await,
        Err(PaymentError::DisputeExceedsCapture)
    ));
    assert!(matches!(
        cmd.open_dispute(&id, claim(" ", 100)).await,
        Err(PaymentError::DisputeReferenceRequired)
    ));
    assert!(matches!(
        cmd.win_dispute(&id, "dp_unknown").await,
        Err(PaymentError::DisputeNotFound)
    ));

    // Two refunds were decided while 60,00 of the 100,00 were disputed.
    assert!(cmd.open_dispute(&id, claim("dp_2", 6_000)).await?);
    let kept = cmd
        .refund_payment(&id, Money::eur(3_000), "return R2026-000001".into())
        .await?;
    let dropped = cmd
        .refund_payment(&id, Money::eur(5_000), "return R2026-000002".into())
        .await?;
    assert_eq!(run_refunds(&executor, &db, &provider).await?.held, 2);

    // The first report this shop gets may be the last one: it says it all.
    let lost = dispute("dp_2", 6_000, DisputeStanding::Lost);
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, lost.clone()).await?,
        Applied::Done
    );
    assert_eq!(
        apply_provider_event(&executor, &db, &provider, lost).await?,
        Applied::Ignored
    );
    let payment = view(&executor, &id).await?;
    assert_eq!(payment.charged_back()?, Money::eur(6_000));
    // 40,00 are left: the older refund still fits, the other failed with the
    // dispute.
    let status = |refund_id: &str| {
        payment
            .refunds
            .iter()
            .find(|r| r.refund_id == refund_id)
            .map(|r| (r.status, r.failure.clone()))
    };
    assert_eq!(status(&kept), Some((RefundStatus::Pending, None)));
    assert_eq!(
        status(&dropped),
        Some((RefundStatus::Failed, Some("dispute lost".into())))
    );
    assert_eq!(payment.refundable()?, Money::eur(1_000));
    // Still captured: a chargeback is not a refund, no credit note follows.
    assert_eq!(payment.status, PaymentStatus::Captured);
    assert_eq!(payment.refunded, Money::eur(0));

    let pass = run_refunds(&executor, &db, &provider).await?;
    assert_eq!((pass.held, pass.settled), (0, 1));
    assert_eq!(view(&executor, &id).await?.refunded, Money::eur(3_000));
    assert!(matches!(
        cmd.refund_payment(&id, Money::eur(1_001), "geste".into())
            .await,
        Err(PaymentError::RefundExceedsCapture)
    ));
    assert!(matches!(
        cmd.retry_refund(&id, &dropped).await,
        Err(PaymentError::RefundExceedsCapture)
    ));

    // A dispute opened and lost in one report, on another order.
    learn_disputes(&executor, &db).await?;
    let rows = disputes_of_order(&db, "order-lost").await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].status.as_str(), rows[0].amount_minor),
        ("lost", 6_000)
    );
    assert_eq!(count_disputes(&db, Some(DisputeStatus::Open)).await?, 0);
    assert_eq!(count_disputes(&db, None).await?, 1);
    Ok(())
}
