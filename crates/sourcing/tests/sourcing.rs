//! The sourcing context on an in-memory database: taking a supplier on,
//! saying where a product is bought, and reading one supplier offer against
//! the rule.

use timada_core::{Money, ShopCurrencies};
use timada_pricing::ListPrice;
use timada_sourcing::{
    Command, FakeConnector, ManualConnector, PricingRule, RegisterSupplier, ReviewReason,
    RuleScope, SourceProduct, SourcingError, SupplierConnectors, SupplierItemRef, SupplierOffer,
    Verdict, list_suppliers, migrations, offer_of_product, sourced_of_product, sourced_of_supplier,
    sourced_products_by_ids, sourcing_list_subscription, supplier_id,
};
use timada_tax::{ExchangeRates, FixedRates};

const PRODUCT: &str = "product-aoc-24g4xe";
const ITEM: &str = "1005006123456789";
const NOW: u64 = 1_789_776_000;

fn connectors() -> SupplierConnectors {
    SupplierConnectors::default()
        .with(ManualConnector)
        .with(FakeConnector::new("fake"))
}

fn rates() -> impl ExchangeRates {
    // 1 EUR = 1,0850 USD, 1 EUR = 0,8538 GBP.
    FixedRates::new("EUR", "test")
        .with("USD", 1_085_000)
        .with("GBP", 853_800)
}

fn offer(cost: Money, shipping: Money, available: u32) -> SupplierOffer {
    SupplierOffer {
        item: SupplierItemRef::new(ITEM, None),
        cost,
        shipping,
        available,
        title: Some("24\" 165 Hz gaming monitor".into()),
        url: Some("https://example.test/item/1005006123456789".into()),
    }
}

async fn shop() -> anyhow::Result<(evento::Sqlite, sqlx::SqlitePool, String)> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command::new(&executor, db.clone());
    let supplier = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "shenzhen-optics".into(),
                name: "Shenzhen Optics".into(),
                connector: "fake".into(),
                currency: "USD".into(),
            },
            &connectors(),
        )
        .await?;
    Ok((executor, db, supplier))
}

/// Gives the product a price of its own, the way an operator would.
async fn list_price<E: evento::Executor>(executor: &E, price: Money) -> anyhow::Result<()> {
    timada_pricing::Command(executor)
        .list_price(ListPrice {
            product_id: PRODUCT.into(),
            price_incl_tax: price,
            vat_rate_bp: 2000,
            eco_participation: Money::eur(0),
        })
        .await?;
    Ok(())
}

/// Says where the test's product is bought.
async fn link<E: evento::Executor>(
    cmd: &Command<'_, E>,
    supplier_id: &str,
    item: &str,
    sku: Option<&str>,
) -> anyhow::Result<String> {
    Ok(cmd
        .source_product(SourceProduct {
            product_id: PRODUCT.into(),
            supplier_id: supplier_id.to_owned(),
            external_item_id: item.to_owned(),
            external_sku: sku.map(str::to_owned),
        })
        .await?)
}

async fn price_now<E: evento::Executor>(executor: &E) -> anyhow::Result<Money> {
    Ok(
        timada_pricing::load_product_price(executor, timada_pricing::price_id(PRODUCT))
            .await?
            .ok_or_else(|| anyhow::anyhow!("price missing"))?
            .price_incl_tax,
    )
}

#[tokio::test]
async fn a_supplier_is_taken_on_once_under_its_slug() -> anyhow::Result<()> {
    let (executor, db, supplier) = shop().await?;
    let cmd = Command::new(&executor, db.clone());
    assert_eq!(supplier, supplier_id("shenzhen-optics"));

    let again = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "Shenzhen Optics".into(),
                name: "Shenzhen Optics SARL".into(),
                connector: "fake".into(),
                currency: "USD".into(),
            },
            &connectors(),
        )
        .await;
    assert!(
        matches!(again, Err(SourcingError::AlreadyRegistered(slug)) if slug == "shenzhen-optics")
    );

    // A connector nobody can answer for, and a currency that is not one.
    let unknown = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "temu".into(),
                name: "Temu".into(),
                connector: "temu".into(),
                currency: "USD".into(),
            },
            &connectors(),
        )
        .await;
    assert!(matches!(unknown, Err(SourcingError::ConnectorUnknown(key)) if key == "temu"));
    let nonsense = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "temu".into(),
                name: "Temu".into(),
                connector: "manual".into(),
                currency: "dollars".into(),
            },
            &connectors(),
        )
        .await;
    assert!(matches!(nonsense, Err(SourcingError::InvalidCurrency(_))));

    cmd.rename_supplier(&supplier, "Shenzhen Optics Ltd".into())
        .await?;
    cmd.suspend_supplier(&supplier, "papers out of date".into())
        .await?;
    cmd.suspend_supplier(&supplier, "again".into()).await?; // no-op

    // Nothing new is sourced from a supplier that is suspended.
    let refused = cmd
        .source_product(SourceProduct {
            product_id: PRODUCT.into(),
            supplier_id: supplier.clone(),
            external_item_id: ITEM.into(),
            external_sku: None,
        })
        .await;
    assert!(matches!(refused, Err(SourcingError::SupplierSuspended)));
    cmd.resume_supplier(&supplier).await?;

    sourcing_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let rows = list_suppliers(&db).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Shenzhen Optics Ltd");
    assert_eq!(rows[0].connector, "fake");
    assert!(!rows[0].suspended);

    Ok(())
}

#[tokio::test]
async fn a_product_is_bought_from_one_supplier_at_a_time() -> anyhow::Result<()> {
    let (executor, db, supplier) = shop().await?;
    let cmd = Command::new(&executor, db.clone());
    let other = cmd
        .register_supplier(
            RegisterSupplier {
                slug: "guangzhou-displays".into(),
                name: "Guangzhou Displays".into(),
                connector: "manual".into(),
                currency: "EUR".into(),
            },
            &connectors(),
        )
        .await?;

    let sourced = link(&cmd, &supplier, ITEM, Some("black")).await?;
    // Saying the same thing again writes nothing at all.
    assert_eq!(link(&cmd, &supplier, ITEM, Some("black")).await?, sourced);
    assert_eq!(
        cmd.load_sourced(&sourced)
            .await?
            .ok_or_else(|| anyhow::anyhow!("sourcing missing"))?
            .external_sku
            .as_deref(),
        Some("black")
    );

    // Somewhere else: the same stream, so a product never has two suppliers.
    assert_eq!(link(&cmd, &other, "GD-9912", None).await?, sourced);
    let moved = cmd
        .load_sourced(&sourced)
        .await?
        .ok_or_else(|| anyhow::anyhow!("sourcing missing"))?;
    assert_eq!(moved.supplier_id, other);
    assert_eq!(moved.external_sku, None);

    // Given up, then taken up again.
    cmd.stop_sourcing(&sourced, "supplier dropped the line".into())
        .await?;
    let gone = cmd.stop_sourcing(&sourced, "again".into()).await;
    assert!(matches!(gone, Err(SourcingError::NotSourced)));
    assert_eq!(link(&cmd, &supplier, ITEM, None).await?, sourced);

    sourcing_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let row = sourced_of_product(&db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("row missing"))?;
    assert_eq!(row.supplier_id, supplier);
    assert!(row.active);
    assert_eq!(sourced_of_supplier(&db, &supplier, 10, 0).await?.len(), 1);
    assert!(sourced_of_supplier(&db, &other, 10, 0).await?.is_empty());
    assert_eq!(
        sourced_products_by_ids(&db, &[PRODUCT.to_owned(), "nothing".to_owned()])
            .await?
            .len(),
        1
    );
    assert!(sourced_products_by_ids(&db, &[]).await?.is_empty());

    Ok(())
}

#[tokio::test]
async fn a_cost_moves_the_price_inside_the_guardrails() -> anyhow::Result<()> {
    let (executor, db, supplier) = shop().await?;
    let cmd = Command::new(&executor, db.clone());
    let base = ShopCurrencies::default();
    let rates = rates();
    cmd.source_product(SourceProduct {
        product_id: PRODUCT.into(),
        supplier_id: supplier.clone(),
        external_item_id: ITEM.into(),
        external_sku: None,
    })
    .await?;
    timada_sourcing::save_rule(
        &db,
        &RuleScope::Supplier(supplier.clone()),
        &PricingRule {
            markup_bp: 6_000,
            min_margin_bp: 4_000,
            ..PricingRule::default()
        },
    )
    .await?;
    list_price(&executor, Money::eur(6_390)).await?;

    // $34,50 + $2,00 shipping at 1,0850 → 33,64 € landed → 64,90 € TTC,
    // a euro up from 63,90: through, and on record.
    let applied = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    let Verdict::Apply(quoted) = applied.verdict else {
        anyhow::bail!("expected the price to move, got {:?}", applied.verdict);
    };
    assert_eq!(quoted.price_incl_tax, Money::eur(6_490));
    assert_eq!(quoted.landed, Money::eur(3_364));
    assert_eq!(price_now(&executor).await?, Money::eur(6_490));

    let recorded = offer_of_product(&db, PRODUCT)
        .await?
        .ok_or_else(|| anyhow::anyhow!("offer missing"))?;
    assert_eq!(recorded.cost, Money::new(3_450, "USD"));
    assert_eq!(recorded.available, 12);
    assert_eq!(recorded.landed, Some(Money::eur(3_364)));
    assert_eq!(
        recorded.rate.map(|(micros, source, _)| (micros, source)),
        Some((1_085_000, "test".to_owned()))
    );

    // Saying it again changes nothing: `change_price` has no equality guard
    // of its own, and a feed polled all day would append an event a pass.
    let again = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert_eq!(again.verdict, Verdict::Unchanged);

    Ok(())
}

#[tokio::test]
async fn what_the_guardrails_stop_is_asked_about_instead() -> anyhow::Result<()> {
    let (executor, db, supplier) = shop().await?;
    let cmd = Command::new(&executor, db.clone());
    let base = ShopCurrencies::default();
    let rates = rates();
    let sourced = cmd
        .source_product(SourceProduct {
            product_id: PRODUCT.into(),
            supplier_id: supplier.clone(),
            external_item_id: ITEM.into(),
            external_sku: None,
        })
        .await?;
    timada_sourcing::save_rule(
        &db,
        &RuleScope::Default,
        &PricingRule {
            markup_bp: 6_000,
            min_margin_bp: 4_000,
            ..PricingRule::default()
        },
    )
    .await?;

    // No price of its own: sourcing does not open one — the VAT rate and the
    // éco-participation that come with a price are the operator's.
    let unpriced = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert!(matches!(
        unpriced.verdict,
        Verdict::Review {
            reason: ReviewReason::NoListedPrice,
            ..
        }
    ));
    // The cost is on record even so, so an operator can see what it would be.
    assert!(offer_of_product(&db, PRODUCT).await?.is_some());

    list_price(&executor, Money::eur(5_990)).await?;

    // 64,90 from 59,90 is 835 bp: too far to take by itself.
    let jump = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert!(matches!(
        jump.verdict,
        Verdict::Review {
            reason: ReviewReason::Jump,
            ..
        }
    ));
    assert_eq!(price_now(&executor).await?, Money::eur(5_990));

    // A markup that cannot clear the shop's floor never reaches the
    // storefront, however small the move it would make. (Rounding is always
    // upwards, so this is the floor's whole job: catching a rule somebody
    // set below the minimum they meant to keep.)
    timada_sourcing::save_rule(
        &db,
        &RuleScope::Product(PRODUCT.into()),
        &PricingRule {
            markup_bp: 500,
            min_margin_bp: 4_000,
            ..PricingRule::default()
        },
    )
    .await?;
    let floor = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert!(matches!(
        floor.verdict,
        Verdict::Review {
            reason: ReviewReason::Floor,
            ..
        }
    ));

    timada_sourcing::clear_rule(&db, &RuleScope::Product(PRODUCT.into())).await?;

    // No rate for the supplier's currency: nothing is priced on a guess.
    let blind = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &FixedRates::new("EUR", "test"),
            base.base(),
            NOW,
        )
        .await?;
    assert!(matches!(
        blind.verdict,
        Verdict::Review {
            reason: ReviewReason::NoRate,
            ..
        }
    ));
    assert_eq!(price_now(&executor).await?, Money::eur(5_990));

    // A price the operator locked is neither moved nor queued.
    cmd.lock_source_price(&sourced, "negotiated for the season".into())
        .await?;
    let locked = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(1_000, "USD"), Money::new(0, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert_eq!(locked.verdict, Verdict::Unchanged);
    assert_eq!(price_now(&executor).await?, Money::eur(5_990));
    // But the cost behind the lock is still worth knowing.
    assert_eq!(
        offer_of_product(&db, PRODUCT)
            .await?
            .ok_or_else(|| anyhow::anyhow!("offer missing"))?
            .cost,
        Money::new(1_000, "USD")
    );

    cmd.unlock_source_price(&sourced).await?;
    let unlocked = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            base.base(),
            NOW,
        )
        .await?;
    assert!(matches!(unlocked.verdict, Verdict::Review { .. }));

    Ok(())
}

#[tokio::test]
async fn prices_in_the_shops_other_currencies_are_flagged_not_converted() -> anyhow::Result<()> {
    let (executor, db, supplier) = shop().await?;
    let cmd = Command::new(&executor, db.clone());
    let rates = rates();
    cmd.source_product(SourceProduct {
        product_id: PRODUCT.into(),
        supplier_id: supplier.clone(),
        external_item_id: ITEM.into(),
        external_sku: None,
    })
    .await?;
    list_price(&executor, Money::eur(6_390)).await?;
    timada_pricing::Command(&executor)
        .set_currency_price(timada_pricing::price_id(PRODUCT), Money::new(5_490, "GBP"))
        .await?;

    let applied = cmd
        .apply_offer(
            PRODUCT,
            &offer(Money::new(3_450, "USD"), Money::new(200, "USD"), 12),
            &rates,
            "EUR",
            NOW,
        )
        .await?;
    assert!(matches!(applied.verdict, Verdict::Apply(_)));
    // The pound price is a decision, never a conversion: it is left exactly
    // as it was, and the operator is told it no longer follows.
    assert_eq!(applied.stale_currencies, vec!["GBP".to_owned()]);
    let price = timada_pricing::load_product_price(&executor, timada_pricing::price_id(PRODUCT))
        .await?
        .ok_or_else(|| anyhow::anyhow!("price missing"))?;
    assert_eq!(price.currency_prices, vec![Money::new(5_490, "GBP")]);

    Ok(())
}
