use timada_core::Money;
use timada_pricing::{Command, InstallmentOffer, ListPrice, PricingError, load_product_price};

fn aoc_price() -> ListPrice {
    ListPrice {
        product_id: "aoc-24g4xe".into(),
        price_incl_tax: Money::eur(11_995),
        vat_rate_bp: 2_000,
        eco_participation: Money::eur(170),
    }
}

#[tokio::test]
async fn price_view_reflects_commands() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(executor.clone());

    let id = cmd.list_price(aoc_price()).await?;
    cmd.attach_installment_offer(
        &id,
        InstallmentOffer {
            count: 3,
            fee: Money::eur(479),
        },
    )
    .await?;

    let view = load_product_price(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("price view missing"))?;
    assert_eq!(view.product_id, "aoc-24g4xe");
    assert_eq!(view.price_incl_tax, Money::eur(11_995));
    assert_eq!(view.price_excl_tax, Money::eur(9_996));
    assert_eq!(view.eco_participation, Money::eur(170));
    assert_eq!(view.installment_amount, Some(Money::eur(4_158)));
    assert!(!view.withdrawn);

    cmd.change_price(&id, Money::eur(12_495)).await?;
    cmd.change_eco_participation(&id, Money::eur(200)).await?;
    cmd.withdraw_price(&id).await?;

    let view = load_product_price(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("price view missing"))?;
    assert_eq!(view.price_incl_tax, Money::eur(12_495));
    assert_eq!(view.price_excl_tax, Money::eur(10_413));
    assert_eq!(view.eco_participation, Money::eur(200));
    assert_eq!(view.installment_amount, Some(Money::eur(4_324)));
    assert!(view.withdrawn);

    Ok(())
}

#[tokio::test]
async fn listing_is_unique_and_withdrawal_is_final() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(executor.clone());

    let id = cmd.list_price(aoc_price()).await?;
    let duplicate = cmd.list_price(aoc_price()).await;
    assert!(matches!(duplicate, Err(PricingError::AlreadyListed(p)) if p == "aoc-24g4xe"));

    let wrong_currency = cmd.change_price(&id, Money::new(100, "USD")).await;
    assert!(matches!(wrong_currency, Err(PricingError::Money(_))));

    let bad_count = cmd
        .attach_installment_offer(
            &id,
            InstallmentOffer {
                count: 5,
                fee: Money::eur(0),
            },
        )
        .await;
    assert!(matches!(
        bad_count,
        Err(PricingError::InvalidInstallmentCount(5))
    ));

    cmd.withdraw_price(&id).await?;
    let after = cmd.change_price(&id, Money::eur(1)).await;
    assert!(matches!(after, Err(PricingError::PriceWithdrawn)));

    Ok(())
}
