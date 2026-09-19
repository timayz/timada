use timada_core::Money;
use timada_payment::{
    Command, PaymentError, PaymentMethod, PaymentStatus, RequestPayment, count_refunds,
    list_refunds, load_payment, migrations, payment_id, refund_list_subscription,
};

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

    cmd.refund_payment(&id, Money::eur(2_496), "goodwill".into())
        .await?;
    let view = load_payment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(view.status, PaymentStatus::Captured);
    assert_eq!(view.psp_reference.as_deref(), Some("psp-123"));
    assert_eq!(view.refunded, Money::eur(2_496));

    cmd.refund_payment(&id, Money::eur(10_000), "returned".into())
        .await?;
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
