use evento::migrator::Migrate as _;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx_migrator::migrator::{Info as _, Migrator};
use timada::{inventory, product, read_model, subscriptions};
use timada_provider::{Money, SourceProduct, SourceVariant};

/// In-memory database with both the event store and the read-model tables.
/// One connection so every pool handle sees the same database.
async fn setup() -> anyhow::Result<(evento::sql::RwSqlite, sqlx::SqlitePool)> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect("sqlite::memory:")
        .await?;

    let mut conn = pool.acquire().await?;
    evento::sql_migrator::new::<sqlx::Sqlite>()?
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await?;
    let mut migrator = Migrator::<sqlx::Sqlite>::default();
    migrator.add_migrations(read_model::migrations())?;
    migrator
        .run(&mut *conn, &sqlx_migrator::migrator::Plan::apply_all())
        .await?;
    drop(conn);

    Ok(((pool.clone().into(), pool.clone().into()).into(), pool))
}

fn source(title: &str, amount_minor: i64) -> SourceProduct {
    SourceProduct {
        external_ref: String::new(),
        title: title.to_owned(),
        description: "A test product".to_owned(),
        image_urls: vec!["https://example.com/a.jpg".to_owned()],
        price: Money {
            amount_minor,
            currency: "USD".to_owned(),
        },
        variants: vec![SourceVariant {
            external_ref: "v1".to_owned(),
            title: "Red".to_owned(),
            price: Money {
                amount_minor,
                currency: "USD".to_owned(),
            },
            options: vec![("Color".to_owned(), "Red".to_owned())],
        }],
    }
}

async fn import_self(executor: &evento::sql::RwSqlite, title: &str) -> anyhow::Result<String> {
    product::import_product(
        executor,
        source(title, 1299),
        timada::self_inventory::KIND,
        "",
    )
    .await
}

#[tokio::test]
async fn import_requires_positive_price() {
    let (executor, _pool) = setup().await.unwrap();

    let result = product::import_product(
        &executor,
        source("Free stuff", 0),
        timada::self_inventory::KIND,
        "",
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn imported_product_starts_as_draft() {
    let (executor, _pool) = setup().await.unwrap();

    let id = import_self(&executor, "Mug").await.unwrap();

    let state = product::load(&executor, &id).await.unwrap().unwrap();
    assert_eq!(state.title, "Mug");
    assert_eq!(state.price_amount_minor, 1299);
    assert!(!state.published);
    assert!(!state.archived);
}

#[tokio::test]
async fn archived_product_cannot_be_revised_or_published() {
    let (executor, _pool) = setup().await.unwrap();
    let id = import_self(&executor, "Mug").await.unwrap();

    product::archive_product(&executor, &id).await.unwrap();

    let revised =
        product::revise_product_details(&executor, &id, "New".to_owned(), String::new()).await;
    assert!(revised.is_err());
    assert!(product::publish_product(&executor, &id).await.is_err());
    assert!(
        product::reprice_product(&executor, &id, 100, "USD".to_owned())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn publish_and_unpublish_round_trip() {
    let (executor, _pool) = setup().await.unwrap();
    let id = import_self(&executor, "Mug").await.unwrap();

    product::publish_product(&executor, &id).await.unwrap();
    assert!(product::publish_product(&executor, &id).await.is_err());
    product::unpublish_product(&executor, &id).await.unwrap();
    assert!(product::unpublish_product(&executor, &id).await.is_err());
}

#[tokio::test]
async fn stock_cannot_go_below_zero() {
    let (executor, _pool) = setup().await.unwrap();
    let id = import_self(&executor, "Mug").await.unwrap();

    inventory::adjust_stock(&executor, &id, 5, "restock".to_owned())
        .await
        .unwrap();
    let result = inventory::adjust_stock(&executor, &id, -6, "oops".to_owned()).await;

    assert!(result.is_err());
    let state = inventory::load(&executor, &id).await.unwrap().unwrap();
    assert_eq!(state.available, 5);
}

#[tokio::test]
async fn stock_adjustments_accumulate() {
    let (executor, _pool) = setup().await.unwrap();
    let id = import_self(&executor, "Mug").await.unwrap();

    inventory::adjust_stock(&executor, &id, 5, "restock".to_owned())
        .await
        .unwrap();
    let available = inventory::adjust_stock(&executor, &id, -2, "damage".to_owned())
        .await
        .unwrap();

    assert_eq!(available, 3);
}

#[tokio::test]
async fn stock_is_refused_for_imported_products() {
    let (executor, _pool) = setup().await.unwrap();
    let id = product::import_product(&executor, source("Gadget", 999), "aliexpress", "conn-1")
        .await
        .unwrap();

    let result = inventory::adjust_stock(&executor, &id, 5, "restock".to_owned()).await;

    assert!(result.is_err());
}

/// Wait until the read model catches up (subscriptions are asynchronous).
async fn wait_for<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn projections_survive_a_full_replay() {
    let (executor, pool) = setup().await.unwrap();

    let subs = subscriptions::start(&executor, pool.clone()).await.unwrap();
    let id = import_self(&executor, "Mug").await.unwrap();
    product::publish_product(&executor, &id).await.unwrap();
    inventory::adjust_stock(&executor, &id, 7, "restock".to_owned())
        .await
        .unwrap();

    let db = pool.clone();
    let target = id.clone();
    assert!(
        wait_for(|| {
            let db = db.clone();
            let target = target.clone();
            async move {
                read_model::stock_levels::available(&db, &target)
                    .await
                    .is_ok_and(|available| available == 7)
            }
        })
        .await,
        "read models never caught up"
    );
    subs.shutdown().await.unwrap();

    // Wipe every subscription cursor: on restart, all events replay from the
    // beginning against tables that already hold rows — upserts must be
    // idempotent.
    sqlx::query("DELETE FROM subscriber")
        .execute(&pool)
        .await
        .unwrap();

    let subs = subscriptions::start(&executor, pool.clone()).await.unwrap();
    let db = pool.clone();
    let target = id.clone();
    assert!(
        wait_for(|| {
            let db = db.clone();
            let target = target.clone();
            async move {
                let page = read_model::catalog_list::page(&db, 10, None, None, true).await;
                page.is_ok_and(|page| {
                    page.edges.len() == 1
                        && page.edges[0].node.id == target
                        && page.edges[0].node.stock_available == 7
                        && page.edges[0].node.status == "published"
                })
            }
        })
        .await,
        "replayed read models are wrong or duplicated"
    );

    let detail = read_model::catalog_detail::by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.status, "published");
    assert_eq!(detail.stock_available, 7);
    assert_eq!(detail.variants.len(), 1);

    subs.shutdown().await.unwrap();
}
