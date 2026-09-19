use timada_core::Money;
use timada_promotion::{
    CodeKind, Command, CreateDiscount, DISCOUNT, DiscountKind, IssueVoucher, ListCodes,
    PromotionError, VOUCHER, VoucherKind, code_list_subscription, count_codes, discount_id,
    list_codes, load_discount_details, load_voucher_balance, migrations, quote_code, voucher_id,
};

#[tokio::test]
async fn promo_code_is_redeemed_once_per_order_and_capped() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db: db.clone(),
    };
    let subtotal = Money::eur(12_496);

    let id = cmd
        .create_discount(CreateDiscount {
            code: " welcome10 ".into(),
            kind: DiscountKind::Percent { bp: 1_000 },
            max_redemptions: Some(1),
            valid_until: None,
        })
        .await?;
    assert_eq!(id, discount_id("WELCOME10"));
    let duplicate = cmd
        .create_discount(CreateDiscount {
            code: "welcome10".into(),
            kind: DiscountKind::Percent { bp: 1_000 },
            max_redemptions: None,
            valid_until: None,
        })
        .await;
    assert!(
        matches!(duplicate, Err(PromotionError::CodeAlreadyExists(code)) if code == "WELCOME10")
    );

    let quote = quote_code(&executor, "welcome10", &subtotal, &subtotal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no quote"))?;
    assert_eq!(quote.kind, CodeKind::Discount);
    assert_eq!(quote.amount, Money::eur(1_250));

    // 10 % of 124,96 €, rounded to the cent.
    let amount = cmd
        .redeem_discount("welcome10", "order-1", &subtotal, &subtotal)
        .await?;
    assert_eq!(amount, Money::eur(1_250));

    // The cap is reached: the second order is refused and the quote is gone.
    let refused = cmd
        .redeem_discount("welcome10", "order-2", &subtotal, &subtotal)
        .await;
    assert!(matches!(refused, Err(PromotionError::LimitReached)));
    assert!(
        quote_code(&executor, "welcome10", &subtotal, &subtotal)
            .await?
            .is_none()
    );

    // Same order again: idempotent, same amount, no new event.
    let again = cmd
        .redeem_discount("welcome10", "order-1", &subtotal, &subtotal)
        .await?;
    assert_eq!(again, amount);
    let view = load_discount_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(view.redeemed, 1);

    // The order fell through: its slot goes to the next one.
    assert!(cmd.release_discount("welcome10", "order-1").await?);
    assert!(!cmd.release_discount("welcome10", "order-1").await?);
    cmd.redeem_discount("welcome10", "order-2", &subtotal, &subtotal)
        .await?;
    let view = load_discount_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(view.redeemed, 1);

    cmd.deactivate_discount(&id).await?;
    let inactive = cmd
        .redeem_discount("welcome10", "order-3", &subtotal, &subtotal)
        .await;
    assert!(matches!(inactive, Err(PromotionError::Inactive)));
    let unknown = cmd
        .redeem_discount("nope", "order-3", &subtotal, &subtotal)
        .await;
    assert!(matches!(unknown, Err(PromotionError::UnknownCode)));

    Ok(())
}

#[tokio::test]
async fn fixed_amount_never_exceeds_what_the_order_can_take() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db,
    };
    cmd.create_discount(CreateDiscount {
        code: "moins50".into(),
        kind: DiscountKind::FixedAmount {
            amount: Money::eur(5_000),
        },
        max_redemptions: Some(1),
        valid_until: None,
    })
    .await?;

    // A code in another currency takes nothing and keeps its slot.
    let usd = Money::new(9_000, "USD");
    let mismatch = cmd.redeem_discount("moins50", "order-1", &usd, &usd).await;
    assert!(matches!(mismatch, Err(PromotionError::Money(_))));

    let redeemed = cmd
        .redeem_code("moins50", "order-1", &Money::eur(3_000), &Money::eur(2_999))
        .await?;
    assert_eq!(redeemed.kind, CodeKind::Discount);
    assert_eq!(redeemed.code, "MOINS50");
    assert_eq!(redeemed.amount, Money::eur(2_999));

    Ok(())
}

#[tokio::test]
async fn voucher_is_spent_up_to_the_order_and_refunded() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db,
    };
    let id = cmd
        .issue_voucher(IssueVoucher {
            code: "gift50".into(),
            customer_id: None,
            value: Money::eur(5_000),
            kind: VoucherKind::GiftVoucher,
            expires_at: None,
        })
        .await?;

    // Not a promo code: the box falls back to the voucher.
    let first = cmd
        .redeem_code("gift50", "order-1", &Money::eur(3_000), &Money::eur(3_000))
        .await?;
    assert_eq!(first.kind, CodeKind::Voucher);
    assert_eq!(first.amount, Money::eur(3_000));
    // A retry of the same order spends nothing more.
    let retry = cmd
        .spend_voucher("gift50", "order-1", &Money::eur(3_000))
        .await?;
    assert_eq!(retry, Money::eur(3_000));

    // The next order gets what is left.
    let second = cmd
        .spend_voucher("gift50", "order-2", &Money::eur(3_000))
        .await?;
    assert_eq!(second, Money::eur(2_000));
    let empty = cmd
        .spend_voucher("gift50", "order-3", &Money::eur(3_000))
        .await;
    assert!(matches!(
        empty,
        Err(PromotionError::InsufficientBalance { .. })
    ));

    cmd.release_code("gift50", "order-1").await?;
    cmd.release_code("gift50", "order-1").await?;
    let view = load_voucher_balance(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("voucher missing"))?;
    assert_eq!(view.remaining, Money::eur(3_000));
    assert_eq!(view.redemptions.len(), 1);
    assert_eq!(view.redemptions[0].order_id, "order-2");

    Ok(())
}

#[tokio::test]
async fn voucher_balance_is_spent_partially_then_cancelled() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db,
    };

    let id = cmd
        .issue_voucher(IssueVoucher {
            code: "gift50".into(),
            customer_id: Some("customer-1".into()),
            value: Money::eur(5_000),
            kind: VoucherKind::GiftVoucher,
            expires_at: None,
        })
        .await?;
    assert_eq!(id, voucher_id("GIFT50"));

    let remaining = cmd
        .redeem_voucher("GIFT50", "order-1", Money::eur(2_000))
        .await?;
    assert_eq!(remaining, Money::eur(3_000));

    let too_much = cmd
        .redeem_voucher("GIFT50", "order-2", Money::eur(4_000))
        .await;
    assert!(matches!(
        too_much,
        Err(PromotionError::InsufficientBalance { remaining }) if remaining == Money::eur(3_000)
    ));

    let view = load_voucher_balance(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("voucher missing"))?;
    assert_eq!(view.value, Money::eur(5_000));
    assert_eq!(view.remaining, Money::eur(3_000));
    assert_eq!(view.redemptions.len(), 1);
    assert_eq!(view.redemptions[0].order_id, "order-1");

    cmd.cancel_voucher(&id, "fraud").await?;
    let cancelled = cmd
        .redeem_voucher("GIFT50", "order-3", Money::eur(100))
        .await;
    assert!(matches!(cancelled, Err(PromotionError::Cancelled)));
    let view = load_voucher_balance(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("voucher missing"))?;
    assert!(view.cancelled);
    assert_eq!(view.cancelled_reason.as_deref(), Some("fraud"));

    let credit = cmd
        .issue_voucher(IssueVoucher {
            code: "avoir-1".into(),
            customer_id: None,
            value: Money::eur(0),
            kind: VoucherKind::CreditNote {
                origin_order_id: "order-9".into(),
            },
            expires_at: None,
        })
        .await;
    assert!(matches!(credit, Err(PromotionError::InvalidAmount)));

    Ok(())
}

#[tokio::test]
async fn code_list_follows_both_kinds_of_code() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db: db.clone(),
    };
    let discount = cmd
        .create_discount(CreateDiscount {
            code: "welcome10".into(),
            kind: DiscountKind::Percent { bp: 1_000 },
            max_redemptions: None,
            valid_until: None,
        })
        .await?;
    let voucher = cmd
        .issue_voucher(IssueVoucher {
            code: "gift50".into(),
            customer_id: None,
            value: Money::eur(5_000),
            kind: VoucherKind::GiftVoucher,
            expires_at: None,
        })
        .await?;
    // Events the list does not fold must not stall the strict subscription.
    cmd.redeem_code(
        "welcome10",
        "order-1",
        &Money::eur(1_000),
        &Money::eur(1_000),
    )
    .await?;
    cmd.redeem_code("gift50", "order-1", &Money::eur(1_000), &Money::eur(1_000))
        .await?;
    cmd.release_code("gift50", "order-1").await?;
    cmd.cancel_voucher(&voucher, "fraud").await?;

    code_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;

    assert_eq!(count_codes(&db, None).await?, 2);
    let discounts = list_codes(
        &db,
        &ListCodes {
            kind: Some(DISCOUNT.into()),
            ..ListCodes::default()
        },
    )
    .await?;
    assert_eq!(discounts.len(), 1);
    assert_eq!(discounts[0].id, discount);
    assert_eq!(discounts[0].code, "WELCOME10");
    assert_eq!(discounts[0].percent_bp, Some(1_000));
    assert!(discounts[0].active);

    let vouchers = list_codes(
        &db,
        &ListCodes {
            kind: Some(VOUCHER.into()),
            ..ListCodes::default()
        },
    )
    .await?;
    assert_eq!(vouchers[0].amount_minor, Some(5_000));
    assert!(!vouchers[0].active, "cancelled voucher");
    Ok(())
}
