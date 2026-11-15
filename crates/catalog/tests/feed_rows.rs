mod common;

use common::*;
use timada_catalog::{
    Command, DefineVariant, Identify, OptionSelection, count_feed_products, feed_products,
    feed_tenant, read_model_subscription,
};
use timada_core::db::Db;

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// A published mug at 12.50 €.
async fn mug(cmd: &Command<evento::Turso>, name: &str) -> String {
    let product = create_product(cmd, name).await;
    cmd.price_product(ALICE, &product.id, 1250, "EUR")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &product.id).await.unwrap();
    product.id
}

#[tokio::test]
async fn a_feed_carries_what_is_on_sale_and_not_taken_out() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let blue = mug(&cmd, "Blue Mug").await;
    let red = mug(&cmd, "Red Mug").await;
    let draft = create_product(&cmd, "Draft Mug").await;
    cmd.describe_product(ALICE, &blue, "Holds 50 cl.")
        .await
        .unwrap();
    cmd.identify_product(
        ALICE,
        &blue,
        Identify {
            brand: Some("Acme".into()),
            gtin: Some("4006381333931".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    cmd.set_feed_label(ALICE, &blue, 1, "eu").await.unwrap();
    cmd.declare_ai_writing(ALICE, &blue, false, true)
        .await
        .unwrap();
    cmd.set_stock(ALICE, &red, None, 0).await.unwrap();
    // Another shop's products are another feed's.
    let other = Command::new(executor.clone(), "other");
    mug(&other, "Other Mug").await;

    catch_up(&executor, &pool).await;
    assert_eq!(count_feed_products(&pool, TENANT).await.unwrap(), 2);
    let items = feed_products(&pool, TENANT, None, 50).await.unwrap();
    let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
    assert!(!ids.contains(&draft.id.as_str()), "a draft is in no feed");
    assert_eq!(ids.len(), 2);
    assert!(ids.is_sorted(), "read in id order, a page after the other");
    let item = items.iter().find(|item| item.id == blue).unwrap();
    assert_eq!(item.slug, "blue-mug");
    assert_eq!(item.description, "Holds 50 cl.");
    assert_eq!(item.brand.as_deref(), Some("Acme"));
    assert_eq!(item.gtin.as_deref(), Some("4006381333931"));
    assert_eq!(item.labels, ["", "eu", "", "", ""]);
    assert!(item.ai_description && !item.ai_title);
    assert_eq!(item.amount_cents, Some(1250));
    assert_eq!(item.currency, "EUR");
    assert!(item.sold_out.is_empty());
    let sold_out = items.iter().find(|item| item.id == red).unwrap();
    assert!(sold_out.sold_out.contains(""), "none left on the shelf");

    // Lowered, with what it is down from.
    cmd.price_product(ALICE, &blue, 990, "EUR").await.unwrap();
    cmd.set_compare_at(ALICE, &blue, None, Some(1250))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let items = feed_products(&pool, TENANT, None, 50).await.unwrap();
    let item = items.iter().find(|item| item.id == blue).unwrap();
    assert_eq!(
        (item.amount_cents, item.compare_at_cents),
        (Some(990), Some(1250))
    );

    // A page at a time.
    let first = feed_products(&pool, TENANT, None, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    let second = feed_products(&pool, TENANT, Some(&first[0].id), 1)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].id, second[0].id);
    assert!(
        feed_products(&pool, TENANT, Some(&second[0].id), 1)
            .await
            .unwrap()
            .is_empty()
    );
    // The stock read beside a page is that page's.
    let red_alone = if first[0].id == red { &first } else { &second };
    assert!(red_alone[0].sold_out.contains(""));

    // Taken out of the feed, unpublished or archived, it leaves it.
    cmd.list_in_feed(ALICE, &blue, false).await.unwrap();
    cmd.unpublish_product(ALICE, &red).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(count_feed_products(&pool, TENANT).await.unwrap(), 0);
    cmd.list_in_feed(ALICE, &blue, true).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(count_feed_products(&pool, TENANT).await.unwrap(), 1);
    cmd.archive_product(ALICE, &blue).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(count_feed_products(&pool, TENANT).await.unwrap(), 0);
    assert_eq!(count_feed_products(&pool, "other").await.unwrap(), 1);
}

#[tokio::test]
async fn a_feed_carries_the_variants_on_sale_as_shoppers_see_them() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let harness = create_product(&cmd, "Cat Harness").await;
    let id = harness.id.as_str();
    let colour = cmd
        .define_option(ALICE, id, "Colour", vec!["Beige".into(), "Blue".into()])
        .await
        .unwrap();
    let from = cmd
        .define_option(ALICE, id, "Ships from", vec!["CN".into(), "FR".into()])
        .await
        .unwrap();
    let pick = |option_id: &str, value: &str| OptionSelection {
        option_id: option_id.to_owned(),
        value: value.to_owned(),
    };
    let mut variants = Vec::new();
    for (value, origin, amount_cents) in [
        ("Beige", "FR", 2290),
        ("Blue", "FR", 2490),
        ("Beige", "CN", 1990),
    ] {
        let variant = cmd
            .define_variant(
                ALICE,
                id,
                DefineVariant {
                    selections: vec![pick(&colour, value), pick(&from, origin)],
                    sku: Some(format!("H-{value}-{origin}")),
                    amount_cents,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        variants.push(variant);
    }
    cmd.label_option(
        ALICE,
        id,
        &colour,
        timada_catalog::OptionLabels {
            label: None,
            values: [("Beige".to_owned(), Some("Sable".to_owned()))].into(),
        },
    )
    .await
    .unwrap();
    cmd.set_option_kind(ALICE, id, &colour, Some("color"))
        .await
        .unwrap();
    cmd.hide_option(ALICE, id, &from, "FR").await.unwrap();
    cmd.identify_variant(ALICE, id, &variants[0], Some("036000291452"), None)
        .await
        .unwrap();
    cmd.set_stock(ALICE, id, Some(&variants[1]), 0)
        .await
        .unwrap();
    cmd.publish_product(ALICE, id).await.unwrap();

    catch_up(&executor, &pool).await;
    let items = feed_products(&pool, TENANT, None, 50).await.unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.amount_cents, None);
    // The one shipping from elsewhere is not on sale, so not in the feed.
    let ids: Vec<&str> = item.variants.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, [variants[0].as_str(), variants[1].as_str()]);
    let beige = &item.variants[0];
    assert_eq!(
        beige.label.as_deref(),
        Some("Sable"),
        "the hidden option is not named"
    );
    assert_eq!(beige.amount_cents, 2290);
    assert_eq!(beige.sku.as_deref(), Some("H-Beige-FR"));
    assert_eq!(beige.gtin.as_deref(), Some("036000291452"));
    assert_eq!(beige.kinds, [("color".to_owned(), "Sable".to_owned())]);
    assert_eq!(item.sold_out.iter().collect::<Vec<_>>(), [&variants[1]]);
}

#[tokio::test]
async fn an_address_names_its_tenant_until_the_feed_moves_or_closes() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let other = Command::new(executor.clone(), "other");
    let first = cmd.open_feed(ALICE).await.unwrap();
    let theirs = other.open_feed(ALICE).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        feed_tenant(&pool, &first).await.unwrap().as_deref(),
        Some(TENANT)
    );
    assert_eq!(
        feed_tenant(&pool, &theirs).await.unwrap().as_deref(),
        Some("other")
    );
    assert_eq!(feed_tenant(&pool, "nope").await.unwrap(), None);

    let second = cmd.open_feed(ALICE).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(feed_tenant(&pool, &first).await.unwrap(), None);
    assert_eq!(
        feed_tenant(&pool, &second).await.unwrap().as_deref(),
        Some(TENANT)
    );

    cmd.close_feed(ALICE).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(feed_tenant(&pool, &second).await.unwrap(), None);
    assert!(feed_tenant(&pool, &theirs).await.unwrap().is_some());
}
