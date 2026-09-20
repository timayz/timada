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
    let cmd = Command(&executor);

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
    let cmd = Command(&executor);

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

#[tokio::test]
async fn a_product_has_a_price_in_each_currency_the_operator_gave_it() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(&executor);
    let id = cmd.list_price(aoc_price()).await?;
    cmd.attach_installment_offer(
        &id,
        InstallmentOffer {
            count: 3,
            fee: Money::eur(479),
        },
    )
    .await?;
    let view = || async {
        load_product_price(&executor, &id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("price view missing"))
    };

    // Listed in euros only: not sold in pounds.
    assert!(view().await?.price_in("GBP").is_none());

    // A decision, not a conversion: 119,95 € and 109,00 £.
    cmd.set_currency_price(&id, Money::new(10_900, "GBP"))
        .await?;
    cmd.set_currency_price(&id, Money::new(12_900, "CHF"))
        .await?;
    // The same price again writes nothing; another one replaces it.
    cmd.set_currency_price(&id, Money::new(10_900, "GBP"))
        .await?;
    cmd.set_currency_price(&id, Money::new(10_500, "GBP"))
        .await?;
    let priced = view().await?;
    assert_eq!(
        priced.currencies().collect::<Vec<_>>(),
        ["EUR", "GBP", "CHF"]
    );
    let pounds = priced
        .price_in("GBP")
        .ok_or_else(|| anyhow::anyhow!("no price in pounds"))?;
    assert_eq!(pounds.price_incl_tax, Money::new(10_500, "GBP"));
    // The product's VAT rate applies whatever the currency.
    assert_eq!(pounds.price_excl_tax, Money::new(8_750, "GBP"));
    // The éco-participation and the instalment offer are the euro's.
    assert_eq!(pounds.eco_participation, None);
    assert_eq!(pounds.installment, None);
    let euros = priced
        .price_in("EUR")
        .ok_or_else(|| anyhow::anyhow!("no price in euros"))?;
    assert_eq!(euros.price_incl_tax, Money::eur(11_995));
    assert_eq!(euros.eco_participation, Some(Money::eur(170)));
    assert_eq!(
        euros.installment.map(|(_, amount)| amount),
        Some(Money::eur(4_158))
    );

    // Setting the listed currency changes the listed price.
    cmd.set_currency_price(&id, Money::eur(10_995)).await?;
    let changed = view().await?;
    assert_eq!(changed.price_incl_tax, Money::eur(10_995));
    assert_eq!(changed.currency_prices.len(), 2);

    assert!(matches!(
        cmd.set_currency_price(&id, Money::new(0, "GBP")).await,
        Err(PricingError::InvalidAmount)
    ));
    assert!(matches!(
        cmd.set_currency_price(&id, Money::new(100, "pounds")).await,
        Err(PricingError::InvalidCurrency(_))
    ));

    // No longer sold in francs; the listed currency is not removed that way.
    cmd.remove_currency_price(&id, "CHF").await?;
    cmd.remove_currency_price(&id, "CHF").await?;
    assert!(view().await?.price_in("CHF").is_none());
    assert!(matches!(
        cmd.remove_currency_price(&id, "EUR").await,
        Err(PricingError::ListedCurrency(_))
    ));

    // Withdrawn: sold nowhere.
    cmd.withdraw_price(&id).await?;
    let withdrawn = view().await?;
    assert!(withdrawn.price_in("EUR").is_none() && withdrawn.price_in("GBP").is_none());
    assert!(matches!(
        cmd.set_currency_price(&id, Money::new(100, "GBP")).await,
        Err(PricingError::PriceWithdrawn)
    ));
    Ok(())
}
