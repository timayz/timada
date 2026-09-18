use evento::Executor;
use timada_core::{Address, Money};
use timada_order::{DeliveryChoice, OrderLine, PaymentMode, PlaceOrder, Seller};
use timada_promotion::{
    Command, CreateDiscount, DiscountKind, IssueVoucher, PromotionError, VoucherKind, discount_id,
    load_discount_details, load_voucher_balance, migrations, redeem_on_order_subscription,
    voucher_id,
};

fn address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "121, Avenue Tolosane".into(),
        postal_code: "31520".into(),
        city: "Ramonville-Saint-Agne".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

async fn place_order<E: Executor>(executor: &E, cart_id: &str) -> anyhow::Result<String> {
    Ok(timada_order::Command(executor)
        .place_order(PlaceOrder {
            cart_id: cart_id.into(),
            customer_id: "customer-1".into(),
            seller: Seller::Ldlc,
            lines: vec![OrderLine {
                product_id: "aoc-24g4xe".into(),
                name: "AOC 23.8\" LED - 24G4XE".into(),
                quantity: 1,
                unit_price: Money::eur(12_496),
                warranty_months: 60,
            }],
            delivery_address: address(),
            billing_address: address(),
            delivery: DeliveryChoice {
                method_code: "chronopost-dom".into(),
                pickup_store_id: None,
            },
            payment_mode: PaymentMode::Card,
            shipping_fee: Money::eur(2_395),
            handling_fee: Money::eur(0),
            promo_code: Some("WELCOME10".into()),
        })
        .await?)
}

#[tokio::test]
async fn promo_code_is_redeemed_once_per_order_and_capped() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db: db.clone(),
    };

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

    let order1 = place_order(&executor, "cart-1").await?;
    let order2 = place_order(&executor, "cart-2").await?;

    // Both orders carry the code; only the first fits under the cap, and the
    // refusal does not stall the subscription.
    redeem_on_order_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let view = load_discount_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(view.redeemed, 1);
    assert!(view.active);

    let refused = cmd.redeem_discount("welcome10", &order2).await;
    assert!(matches!(refused, Err(PromotionError::LimitReached)));

    // Same order again: idempotent, no new event.
    cmd.redeem_discount("welcome10", &order1).await?;
    let view = load_discount_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(view.redeemed, 1);

    cmd.deactivate_discount(&id).await?;
    let inactive = cmd.redeem_discount("welcome10", "order-3").await;
    assert!(matches!(inactive, Err(PromotionError::Inactive)));
    let unknown = cmd.redeem_discount("nope", "order-3").await;
    assert!(matches!(unknown, Err(PromotionError::UnknownCode)));

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
