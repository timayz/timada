mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{
    CatalogError, Command, DefineVariant, MAX_AMOUNT_CENTS, MarginTiers, OptionSelection,
    ProductCosts, ShopCosts, UnitCosts, VariantCost, feed_products, product_cost_id, product_costs,
    read_model_subscription,
};

/// How many events the product's cost stream holds.
async fn cost_events(cmd: &Command<evento::Turso>, product_id: &str) -> u16 {
    timada_catalog::product_cost_projection()
        .load(product_cost_id(product_id))
        .tenant(TENANT)
        .execute(cmd.executor())
        .await
        .unwrap()
        .map_or(0, |state| state.aggregate_version().unwrap())
}

/// A product varying by colour, with the ids of its two variants.
async fn harness(cmd: &Command<evento::Turso>) -> (String, String, String) {
    let product = create_product(cmd, "Cat Harness").await;
    let colour = cmd
        .define_option(
            ALICE,
            &product.id,
            "Colour",
            vec!["Beige".into(), "Blue".into()],
        )
        .await
        .unwrap();
    let mut variants = Vec::new();
    for value in ["Beige", "Blue"] {
        let variant = cmd
            .define_variant(
                ALICE,
                &product.id,
                DefineVariant {
                    selections: vec![OptionSelection {
                        option_id: colour.clone(),
                        value: value.to_owned(),
                    }],
                    sku: None,
                    amount_cents: 1990,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        variants.push(variant);
    }
    let blue = variants.pop().unwrap();
    (product.id, variants.pop().unwrap(), blue)
}

fn cost(variant_id: &str, amount_cents: Option<u64>) -> VariantCost {
    VariantCost {
        variant_id: variant_id.to_owned(),
        amount_cents,
    }
}

#[tokio::test]
async fn a_product_sold_as_is_says_what_it_costs() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();

    assert_eq!(
        cmd.load_product_costs(id).await.unwrap(),
        ProductCosts::default()
    );
    cmd.set_cost(ALICE, id, Some(400)).await.unwrap();
    let known = cmd.load_product_costs(id).await.unwrap();
    assert_eq!(known.cost_cents, Some(400));
    assert_eq!(
        known.unit_costs(None),
        UnitCosts {
            cost_cents: Some(400),
            ..UnitCosts::default()
        }
    );
    // What the shop pays is no storefront's business: the product's own
    // stream, which a storefront reads, says nothing of it.
    let product = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(product.aggregate_version().unwrap(), 1);

    // The same again writes nothing.
    cmd.set_cost(ALICE, id, Some(400)).await.unwrap();
    assert_eq!(cost_events(&cmd, id).await, 1);

    cmd.set_cost(ALICE, id, None).await.unwrap();
    let forgotten = cmd.load_product_costs(id).await.unwrap();
    assert_eq!(forgotten.cost_cents, None);

    assert!(matches!(
        cmd.set_cost(ALICE, id, Some(MAX_AMOUNT_CENTS + 1)).await,
        Err(CatalogError::InvalidAmount)
    ));
    assert!(matches!(
        cmd.set_cost(ALICE, "nope", Some(1)).await,
        Err(CatalogError::NotFound)
    ));
}

#[tokio::test]
async fn combinations_are_costed_together_in_one_event() {
    let cmd = command().await;
    let (id, beige, blue) = harness(&cmd).await;

    // A product sold through its combinations has no cost of its own.
    assert!(matches!(
        cmd.set_cost(ALICE, &id, Some(400)).await,
        Err(CatalogError::CostOnVariants)
    ));

    let set = cmd
        .set_variant_costs(
            ALICE,
            &id,
            &[cost(&beige, Some(700)), cost(&blue, Some(650))],
        )
        .await
        .unwrap();
    assert!(set);
    assert_eq!(cost_events(&cmd, &id).await, 1, "one event");
    let known = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(known.cost_of(Some(&beige)), Some(700));
    assert_eq!(known.cost_of(Some(&blue)), Some(650));

    // Only what moved is written, and nothing when nothing did.
    let set = cmd
        .set_variant_costs(ALICE, &id, &[cost(&beige, Some(700)), cost(&blue, None)])
        .await
        .unwrap();
    assert!(set);
    let known = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(known.cost_of(Some(&beige)), Some(700));
    assert_eq!(known.cost_of(Some(&blue)), None);
    let set = cmd
        .set_variant_costs(ALICE, &id, &[cost(&beige, Some(700))])
        .await
        .unwrap();
    assert!(!set);
    assert_eq!(cost_events(&cmd, &id).await, 2);

    // One that is not the product's sets none of them.
    assert!(matches!(
        cmd.set_variant_costs(ALICE, &id, &[cost(&beige, Some(1)), cost("nope", Some(1))])
            .await,
        Err(CatalogError::VariantNotFound)
    ));
    let untouched = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(untouched.cost_of(Some(&beige)), Some(700));

    // One cost for everything it sells.
    cmd.set_every_cost(ALICE, &id, Some(500)).await.unwrap();
    let known = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(known.cost_of(Some(&beige)), Some(500));
    assert_eq!(known.cost_of(Some(&blue)), Some(500));
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.set_every_cost(ALICE, &mug.id, Some(300)).await.unwrap();
    let mug = cmd.load_product_costs(&mug.id).await.unwrap();
    assert_eq!(mug.cost_cents, Some(300));
}

#[tokio::test]
async fn a_product_has_sending_and_import_costs_of_its_own() {
    let cmd = command().await;
    let (id, beige, _) = harness(&cmd).await;

    cmd.set_own_costs(ALICE, &id, Some(0), Some(500))
        .await
        .unwrap();
    let known = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(
        known.unit_costs(Some(&beige)),
        UnitCosts {
            cost_cents: None,
            shipping_cents: Some(0),
            import_cents: Some(500),
        }
    );
    cmd.set_own_costs(ALICE, &id, Some(0), Some(500))
        .await
        .unwrap();
    assert_eq!(cost_events(&cmd, &id).await, 1);

    cmd.set_own_costs(ALICE, &id, None, None).await.unwrap();
    let back = cmd.load_product_costs(&id).await.unwrap();
    assert!(back.is_empty());

    cmd.archive_product(ALICE, &id).await.unwrap();
    assert!(matches!(
        cmd.set_own_costs(ALICE, &id, Some(1), None).await,
        Err(CatalogError::Archived)
    ));
    assert!(matches!(
        cmd.set_variant_costs(ALICE, &id, &[cost(&beige, Some(1))])
            .await,
        Err(CatalogError::Archived)
    ));
}

#[tokio::test]
async fn a_shop_says_what_a_sale_costs_it_and_grades_its_margins() {
    let cmd = command().await;
    let nothing = cmd.load_sale_costs().await.unwrap();
    assert_eq!(nothing.costs, None);
    assert_eq!(nothing.tiers(), MarginTiers::default());

    let costs = ShopCosts {
        currency: "eur".into(),
        shipping_cents: 400,
        import_cents: 500,
        fixed_cents: 100,
        payment_fee_bps: 150,
        payment_fee_cents: 25,
    };
    cmd.set_sale_costs(ALICE, costs.clone()).await.unwrap();
    let known = cmd.load_sale_costs().await.unwrap();
    assert_eq!(
        known.costs,
        Some(ShopCosts {
            currency: "EUR".into(),
            ..costs.clone()
        })
    );
    let version = known.aggregate_version().unwrap();
    cmd.set_sale_costs(ALICE, costs.clone()).await.unwrap();
    let again = cmd.load_sale_costs().await.unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);

    assert!(matches!(
        cmd.set_sale_costs(
            ALICE,
            ShopCosts {
                payment_fee_bps: 10_001,
                ..costs.clone()
            }
        )
        .await,
        Err(CatalogError::InvalidFee)
    ));
    assert!(matches!(
        cmd.set_sale_costs(
            ALICE,
            ShopCosts {
                currency: "nope".into(),
                ..costs.clone()
            }
        )
        .await,
        Err(CatalogError::InvalidCurrency)
    ));

    let tiers = MarginTiers {
        medium_from_bps: 1_500,
        high_from_bps: 3_000,
        feed_label: Some(4),
    };
    cmd.set_margin_tiers(ALICE, tiers).await.unwrap();
    let known = cmd.load_sale_costs().await.unwrap();
    assert_eq!(known.tiers(), tiers);
    assert!(known.costs.is_some(), "the costs stay");

    // One label says one thing: what is kept out of ads takes another.
    assert_eq!(cmd.load_feed_ads_label().await.unwrap(), None);
    assert!(matches!(
        cmd.set_feed_ads_label(ALICE, Some(4)).await,
        Err(CatalogError::FeedLabelTaken)
    ));
    assert!(matches!(
        cmd.set_feed_ads_label(ALICE, Some(5)).await,
        Err(CatalogError::InvalidFeedLabel)
    ));
    cmd.set_feed_ads_label(ALICE, Some(3)).await.unwrap();
    assert_eq!(cmd.load_feed_ads_label().await.unwrap(), Some(3));
    assert!(matches!(
        cmd.set_margin_tiers(
            ALICE,
            MarginTiers {
                feed_label: Some(3),
                ..tiers
            }
        )
        .await,
        Err(CatalogError::FeedLabelTaken)
    ));
    cmd.set_feed_ads_label(ALICE, None).await.unwrap();
    assert_eq!(cmd.load_feed_ads_label().await.unwrap(), None);
    for refused in [
        MarginTiers {
            medium_from_bps: 3_000,
            ..tiers
        },
        MarginTiers {
            high_from_bps: 10_000,
            ..tiers
        },
        MarginTiers {
            medium_from_bps: 0,
            ..tiers
        },
        MarginTiers {
            feed_label: Some(5),
            ..tiers
        },
    ] {
        assert!(matches!(
            cmd.set_margin_tiers(ALICE, refused).await,
            Err(CatalogError::InvalidMarginTiers)
        ));
    }
}

/// A shop may grade its margins before it said any cost: the stream's
/// first event is either.
#[tokio::test]
async fn the_tiers_may_come_before_the_costs() {
    let cmd = command().await;
    let tiers = MarginTiers {
        medium_from_bps: 1_000,
        high_from_bps: 2_000,
        feed_label: None,
    };
    cmd.set_margin_tiers(ALICE, tiers).await.unwrap();
    cmd.set_sale_costs(
        ALICE,
        ShopCosts {
            currency: "EUR".into(),
            ..ShopCosts::default()
        },
    )
    .await
    .unwrap();
    let known = cmd.load_sale_costs().await.unwrap();
    assert_eq!(known.tiers(), tiers);
    assert_eq!(known.costs.unwrap().currency, "EUR");
}

/// The rows a feed grades its margins from follow the cost streams, and go
/// with the costs and with the product.
#[tokio::test]
async fn the_rows_carry_the_costs_to_the_feed() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let catch_up = async || {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    let (id, beige, _) = harness(&cmd).await;
    cmd.publish_product(ALICE, &id).await.unwrap();
    cmd.set_variant_costs(ALICE, &id, &[cost(&beige, Some(700))])
        .await
        .unwrap();
    cmd.set_own_costs(ALICE, &id, Some(250), None)
        .await
        .unwrap();
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.price_product(ALICE, &mug.id, 1250, "EUR")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &mug.id).await.unwrap();
    catch_up().await;

    let wanted = cmd.load_product_costs(&id).await.unwrap();
    assert_eq!(product_costs(&pool, TENANT, &id).await.unwrap(), wanted);
    let page = feed_products(&pool, TENANT, None, 10).await.unwrap();
    let of = |product_id: &str| page.iter().find(|p| p.id == product_id).unwrap();
    assert_eq!(of(&id).costs, wanted);
    assert_eq!(of(&id).costs.unit_costs(Some(&beige)).cost_cents, Some(700));
    assert!(of(&mug.id).costs.is_empty(), "nothing said, no row");

    // Costs forgotten leave no row behind.
    cmd.set_variant_costs(ALICE, &id, &[cost(&beige, None)])
        .await
        .unwrap();
    cmd.set_own_costs(ALICE, &id, None, None).await.unwrap();
    cmd.set_cost(ALICE, &mug.id, Some(400)).await.unwrap();
    catch_up().await;
    let rows: Vec<(String,)> = pool
        .fetch_all(
            "SELECT product_id FROM catalog_product_costs WHERE tenant_id = ?1",
            (TENANT,),
        )
        .await
        .unwrap();
    assert_eq!(rows, vec![(mug.id.clone(),)]);

    // A deleted product takes its row with it.
    cmd.delete_product(ALICE, &mug.id).await.unwrap();
    catch_up().await;
    assert!(
        product_costs(&pool, TENANT, &mug.id)
            .await
            .unwrap()
            .is_empty()
    );
}
