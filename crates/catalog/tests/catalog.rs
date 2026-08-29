//! Cover for the catalog's write side and its read models against a real
//! SQLite database — mocking the event store here would prove nothing.

use std::sync::Arc;

use evento::cursor::Args;
use evento::{Aggregate as _, EventFilter, Executor as _};
use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_catalog::{
    CatalogState, Product, archive_product, import_product, load_product, migrations, product_id,
    publish_product, read_models_subscription, set_product_price,
};
use timada_core::{Currency, Money, ServiceContext, new_id};
use timada_dropship::{MockSupplier, SupplierProduct, SupplierRegistry};

/// A temp-file database plus the state every test needs, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: CatalogState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-catalog-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(migrations())?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        let ctx = ServiceContext::new(pool.clone(), pool.clone()).await?;
        let registry = SupplierRegistry::builder()
            .register(Arc::new(MockSupplier::new()))
            .build();

        Ok(Self {
            dir,
            pool,
            state: CatalogState { ctx, registry },
        })
    }

    /// Every event ever written for one product, oldest first.
    async fn events(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let page = self
            .state
            .ctx
            .executor
            .read(
                Some(vec![EventFilter::by_id(Product::aggregate_type(), id)]),
                None,
                Args::forward(50, None),
            )
            .await?;

        Ok(page.edges.into_iter().map(|edge| edge.node.name).collect())
    }

    /// Drain the read-model subscription deterministically instead of racing
    /// the background task `start_subscriptions` would spawn.
    async fn drain_read_models(&self) -> anyhow::Result<()> {
        read_models_subscription(self.pool.clone())
            .no_retry()
            .run_once(&self.state.ctx.executor)
            .await
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn lamp() -> SupplierProduct {
    SupplierProduct {
        supplier_product_ref: "MP-1001".to_owned(),
        title: "Aurora Desk Lamp".to_owned(),
        description: "Warm dimmable LED lamp.".to_owned(),
        price: Money::new(3499, Currency::Eur),
        image_url: "https://placehold.co/400x400".to_owned(),
    }
}

#[tokio::test]
async fn importing_the_same_supplier_product_twice_writes_one_event() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let id = import_product(&db.state.ctx.executor, "mock", lamp()).await?;
    assert_eq!(
        id,
        product_id("mock", "MP-1001"),
        "the aggregate id must be derivable from (supplier_id, supplier_product_ref)"
    );
    assert_eq!(db.events(&id).await?, vec!["ProductImported"]);

    // A double-submitted import form must not open a second listing.
    let replayed = import_product(&db.state.ctx.executor, "mock", lamp()).await?;
    assert_eq!(replayed, id);
    assert_eq!(db.events(&id).await?, vec!["ProductImported"]);

    let Some(product) = load_product(&db.state.ctx.executor, &id).await? else {
        panic!("an imported product must be loadable");
    };
    assert_eq!(product.supplier_id, "mock");
    assert_eq!(product.supplier_product_ref, "MP-1001");
    assert_eq!(product.title, "Aurora Desk Lamp");
    assert_eq!(product.price_cents, 3499);
    assert_eq!(product.currency, Currency::Eur);
    assert!(!product.published);
    assert!(!product.archived);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn publishing_is_visible_immediately_and_only_happens_once() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = import_product(&db.state.ctx.executor, "mock", lamp()).await?;

    publish_product(&db.state.ctx.executor, &id).await?;

    // Read-your-own-write: the event store, not the eventual SQL tables.
    let Some(product) = load_product(&db.state.ctx.executor, &id).await? else {
        panic!("a published product must be loadable");
    };
    assert!(product.published);
    assert_eq!(
        db.events(&id).await?,
        vec!["ProductImported", "ProductPublished"]
    );

    publish_product(&db.state.ctx.executor, &id).await?;
    assert_eq!(
        db.events(&id).await?,
        vec!["ProductImported", "ProductPublished"],
        "republishing must not append a second event"
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn archiving_is_terminal_and_blocks_republishing() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = import_product(&db.state.ctx.executor, "mock", lamp()).await?;
    publish_product(&db.state.ctx.executor, &id).await?;

    archive_product(&db.state.ctx.executor, &id).await?;
    let Some(product) = load_product(&db.state.ctx.executor, &id).await? else {
        panic!("an archived product must still be loadable");
    };
    assert!(product.archived);
    assert!(!product.published);

    archive_product(&db.state.ctx.executor, &id).await?;
    publish_product(&db.state.ctx.executor, &id).await?;
    assert_eq!(
        db.events(&id).await?,
        vec!["ProductImported", "ProductPublished", "ProductArchived"],
        "neither re-archiving nor republishing an archived product may append"
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn commands_on_an_unknown_product_are_ignored() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    publish_product(&db.state.ctx.executor, "no-such-product").await?;
    archive_product(&db.state.ctx.executor, "no-such-product").await?;

    assert!(db.events("no-such-product").await?.is_empty());
    assert!(
        load_product(&db.state.ctx.executor, "no-such-product")
            .await?
            .is_none()
    );

    db.close().await;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct StoreRow {
    id: String,
    title: String,
    price_cents: i64,
    currency: String,
}

#[derive(sqlx::FromRow)]
struct AdminRow {
    id: String,
    status: String,
    supplier_id: String,
}

#[tokio::test]
async fn the_read_models_follow_the_product_lifecycle() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = import_product(&db.state.ctx.executor, "mock", lamp()).await?;

    db.drain_read_models().await?;

    // Imported, not published: admin sees a draft, the storefront sees nothing.
    let admin: Vec<AdminRow> =
        sqlx::query_as("SELECT id, status, supplier_id FROM admin_product_list")
            .fetch_all(&db.pool)
            .await?;
    assert_eq!(admin.len(), 1);
    assert_eq!(admin[0].id, id);
    assert_eq!(admin[0].status, "draft");
    assert_eq!(admin[0].supplier_id, "mock");

    let grid: Vec<StoreRow> =
        sqlx::query_as("SELECT id, title, price_cents, currency FROM store_product_list")
            .fetch_all(&db.pool)
            .await?;
    assert!(
        grid.is_empty(),
        "a draft must not reach the storefront grid"
    );

    let published: Option<i64> =
        sqlx::query_scalar("SELECT published FROM store_product_detail WHERE id = ?")
            .bind(&id)
            .fetch_optional(&db.pool)
            .await?;
    assert_eq!(published, Some(0));

    // Publish: the grid row is copied out of the detail row.
    publish_product(&db.state.ctx.executor, &id).await?;
    db.drain_read_models().await?;

    let grid: Vec<StoreRow> =
        sqlx::query_as("SELECT id, title, price_cents, currency FROM store_product_list")
            .fetch_all(&db.pool)
            .await?;
    assert_eq!(grid.len(), 1);
    assert_eq!(grid[0].id, id);
    assert_eq!(grid[0].title, "Aurora Desk Lamp");
    assert_eq!(grid[0].price_cents, 3499);
    assert_eq!(grid[0].currency, "EUR");

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM admin_product_list WHERE id = ?")
            .bind(&id)
            .fetch_optional(&db.pool)
            .await?;
    assert_eq!(status.as_deref(), Some("published"));

    // Archive: out of the grid, kept in detail so old links still resolve.
    archive_product(&db.state.ctx.executor, &id).await?;
    db.drain_read_models().await?;

    let grid_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM store_product_list")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(grid_count, 0);

    let published: Option<i64> =
        sqlx::query_scalar("SELECT published FROM store_product_detail WHERE id = ?")
            .bind(&id)
            .fetch_optional(&db.pool)
            .await?;
    assert_eq!(published, Some(0), "the detail row outlives archiving");

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM admin_product_list WHERE id = ?")
            .bind(&id)
            .fetch_optional(&db.pool)
            .await?;
    assert_eq!(status.as_deref(), Some("archived"));

    // A second pass resumes from the persisted cursor and changes nothing.
    db.drain_read_models().await?;
    let admin_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_product_list")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(admin_count, 1);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn per_currency_prices_replace_and_project() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let id = import_product(&db.state.ctx.executor, "mock", lamp()).await?;
    publish_product(&db.state.ctx.executor, &id).await?;

    set_product_price(&db.state.ctx.executor, &id, Money::new(3999, Currency::Usd)).await?;
    // Latest set wins for a currency, on the view and in SQL alike.
    set_product_price(&db.state.ctx.executor, &id, Money::new(3499, Currency::Usd)).await?;

    let product = load_product(&db.state.ctx.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("product should replay"))?;
    assert_eq!(
        product.price_in(Currency::Usd),
        Some(Money::new(3499, Currency::Usd))
    );
    assert_eq!(product.price_in(Currency::Eur), Some(product.base_price()));

    db.drain_read_models().await?;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT currency, amount_cents FROM store_product_prices WHERE product_id = ? ORDER BY currency",
    )
    .bind(&id)
    .fetch_all(&db.pool)
    .await?;
    assert_eq!(rows.len(), 2, "base EUR price plus the explicit USD one");
    assert_eq!(rows[0], ("EUR".to_owned(), lamp().price.amount_cents));
    assert_eq!(rows[1], ("USD".to_owned(), 3499));

    // An archived product refuses new prices.
    archive_product(&db.state.ctx.executor, &id).await?;
    assert!(
        set_product_price(&db.state.ctx.executor, &id, Money::new(1, Currency::Usd))
            .await
            .is_err()
    );

    db.close().await;
    Ok(())
}
