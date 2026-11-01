//! A measurement tool, not a check (issue #337): how long the read-model
//! catch-up takes per event, and where the time goes. Ignored by default;
//! run it with
//!
//! ```text
//! cargo test -p timada-catalog --test catchup_probe -- --ignored --nocapture
//! ```
//!
//! and read the `probe` lines. `CATALOG_PROBE_PRODUCTS` sets how many
//! products are replayed (300 by default, about 3 300 events: a product is
//! created, described, priced, given three pictures, an option, two variants,
//! a category and published, as the Chatounette catalogue's are). Three
//! catch-ups run over the same synthetic history, each on a fresh in-memory
//! store with the crate's migrations:
//!
//! - `handler`: the read-model subscription as it ships;
//! - `handler, fts dropped`: the same with the full-text index gone from
//!   `catalog_products`, so what is left is the row writes and the loads;
//! - `loads only`: a handler that loads the product's write model and writes
//!   nothing, the floor the subscription cannot go under;
//! - `writes only`: every product's final state written once through
//!   [`upsert_product`], the cost of one row write apart from the loads.

mod common;

use std::time::{Duration, Instant};

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::Db;

use common::*;
use timada_catalog::{
    AddImage, Command, DefineVariant, OptionSelection, ProductFilter, aggregator::Product,
    count_products, product_projection, read_model_subscription, upsert_product,
};

/// A product's history, as a merchant building a catalogue writes it: how
/// many events it wrote, and the products' ids.
async fn seed(cmd: &Command<evento::Turso>, products: usize) -> (usize, Vec<String>) {
    let mut categories = Vec::new();
    for name in ["Bedding", "Toys", "Harnesses", "Bowls", "Scratchers"] {
        categories.push(create_category(cmd, name).await.id);
    }
    let mut events = categories.len();
    let mut ids = Vec::with_capacity(products);
    for n in 0..products {
        let product = create_product(cmd, &format!("Product {n}")).await;
        ids.push(product.id.clone());
        cmd.describe_product(ALICE, &product.id, &format!("The {n}th thing we sell."))
            .await
            .unwrap();
        cmd.price_product(ALICE, &product.id, 1000 + n as u64, "EUR")
            .await
            .unwrap();
        for i in 0..3 {
            cmd.add_image(
                ALICE,
                &product.id,
                AddImage {
                    image_id: format!("img-{n}-{i}"),
                    ext: "jpg".into(),
                    size: 20_000,
                    width: Some(800),
                    height: Some(600),
                    alt: format!("Picture {i}"),
                },
            )
            .await
            .unwrap();
        }
        let colour = cmd
            .define_option(
                ALICE,
                &product.id,
                "Colour",
                vec!["Beige".into(), "Blue".into()],
            )
            .await
            .unwrap();
        for value in ["Beige", "Blue"] {
            cmd.define_variant(
                ALICE,
                &product.id,
                DefineVariant {
                    selections: vec![OptionSelection {
                        option_id: colour.clone(),
                        value: value.into(),
                    }],
                    sku: None,
                    amount_cents: 1200 + n as u64,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        }
        cmd.categorize_product(ALICE, &product.id, &categories[n % categories.len()])
            .await
            .unwrap();
        cmd.publish_product(ALICE, &product.id).await.unwrap();
        events += 10;
    }
    (events, ids)
}

#[evento::subscription_all]
async fn load_only<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?;
    Ok(())
}

/// Seeds a fresh store, prepares it with `prepare`, and times one catch-up
/// of `subscription` over it.
async fn measure<F>(
    label: &str,
    products: usize,
    prepare: impl AsyncFn(&Db),
    subscription: F,
) -> Duration
where
    F: FnOnce(Db) -> SubscriptionBuilder<evento::Turso>,
{
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let seeding = Instant::now();
    let (events, _) = seed(&cmd, products).await;
    let seeding = seeding.elapsed();
    prepare(&pool).await;

    let started = Instant::now();
    subscription(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    tracing::info!(
        target: "probe",
        label,
        events,
        catch_up_ms = elapsed.as_millis(),
        per_event_ms = format_args!("{:.2}", elapsed.as_secs_f64() * 1000.0 / events as f64),
        seeding_ms = seeding.as_millis(),
        "catch-up"
    );
    elapsed
}

#[tokio::test]
#[ignore = "a measurement, run on purpose"]
async fn catch_up_cost_per_event() {
    tracing_subscriber::fmt()
        .with_env_filter("probe=info")
        .with_test_writer()
        .init();
    let products: usize = std::env::var("CATALOG_PROBE_PRODUCTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);

    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    seed(&cmd, 2).await;
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert_eq!(
        count_products(&pool, TENANT, &ProductFilter::default())
            .await
            .unwrap(),
        2,
        "the handler under measurement writes the rows"
    );

    measure(
        "handler",
        products,
        async |_| {},
        read_model_subscription::<evento::Turso>,
    )
    .await;
    measure(
        "handler, fts dropped",
        products,
        async |pool: &Db| {
            pool.connect()
                .unwrap()
                .execute("DROP INDEX catalog_products_fts", ())
                .await
                .unwrap();
        },
        read_model_subscription::<evento::Turso>,
    )
    .await;
    measure(
        "loads only",
        products,
        async |_| {},
        |_| {
            SubscriptionBuilder::new("timada-catalog/probe-loads")
                .handler(load_only())
                .any_routing_key()
        },
    )
    .await;

    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let (_, ids) = seed(&cmd, products).await;
    let mut states = Vec::with_capacity(ids.len());
    for id in &ids {
        states.push(cmd.load_product(id).await.unwrap().unwrap());
    }
    let started = Instant::now();
    for state in &states {
        upsert_product(&pool, TENANT, state).await.unwrap();
    }
    let elapsed = started.elapsed();
    tracing::info!(
        target: "probe",
        label = "writes only",
        writes = states.len(),
        total_ms = elapsed.as_millis(),
        per_write_ms = format_args!("{:.2}", elapsed.as_secs_f64() * 1000.0 / states.len() as f64),
        "row writes"
    );
}
