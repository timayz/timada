//! Discounts against a real SQLite database: validity windows, the atomic
//! redemption counter, and the amount maths checkout relies on.

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::{Currency, Money, ServiceContext, new_id, now_millis};
use timada_promotion::{
    CreateDiscountError, DiscountKind, DiscountRefusal, PromotionState, create_discount,
    disable_discount, discount_amount, discount_id, load_discount, migrations,
    read_models_subscription, recent_discounts, redeem, validate,
};

/// A temp-file database plus the state every test needs, torn down on close.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: PromotionState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-promotion-{}", new_id()));
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

        Ok(Self {
            dir,
            pool,
            state: PromotionState { ctx },
        })
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn eur(cents: i64) -> Money {
    Money::new(cents, Currency::Eur)
}

#[tokio::test]
async fn codes_are_natural_keys_with_validity_windows() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let executor = &db.state.ctx.executor;
    let now = now_millis();

    let id = create_discount(
        executor,
        "  welcome10 ",
        DiscountKind::Percentage { bps: 1000 },
        now - 1000,
        Some(now + 60_000),
        Some(2),
    )
    .await?;
    assert_eq!(id, discount_id("WELCOME10"), "codes normalize to uppercase");

    // The same code, however typed, is taken.
    assert!(matches!(
        create_discount(
            executor,
            "Welcome10",
            DiscountKind::Percentage { bps: 500 },
            now,
            None,
            None
        )
        .await,
        Err(CreateDiscountError::CodeTaken)
    ));

    let view = load_discount(executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount should replay"))?;
    assert_eq!(view.code, "WELCOME10");
    assert!(validate(&view, now).is_ok());
    assert_eq!(
        validate(&view, view.starts_at - 1),
        Err(DiscountRefusal::NotStarted)
    );
    assert_eq!(
        validate(&view, now + 120_000),
        Err(DiscountRefusal::Expired)
    );

    disable_discount(executor, &id).await?;
    let view = load_discount(executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount should replay"))?;
    assert_eq!(validate(&view, now), Err(DiscountRefusal::Disabled));

    // The admin read model followed along.
    read_models_subscription(db.pool.clone())
        .no_retry()
        .run_once(executor)
        .await?;
    let rows = recent_discounts(&db.pool, 10).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].code, "WELCOME10");
    assert_eq!(rows[0].status, "disabled");

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn the_redemption_counter_exhausts_exactly_at_the_limit() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let executor = &db.state.ctx.executor;

    let id = create_discount(
        executor,
        "LIMIT2",
        DiscountKind::Percentage { bps: 1000 },
        now_millis(),
        None,
        Some(2),
    )
    .await?;
    let view = load_discount(executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount should replay"))?;

    assert!(redeem(&db.pool, &view).await?);
    assert!(redeem(&db.pool, &view).await?);
    assert!(
        !redeem(&db.pool, &view).await?,
        "the third slot must refuse"
    );

    // Uncapped codes count but never refuse.
    let open_id = create_discount(
        executor,
        "OPEN",
        DiscountKind::Percentage { bps: 500 },
        now_millis(),
        None,
        None,
    )
    .await?;
    let open = load_discount(executor, &open_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("discount should replay"))?;
    for _ in 0..5 {
        assert!(redeem(&db.pool, &open).await?);
    }

    db.close().await;
    Ok(())
}

#[test]
fn amount_maths_truncate_clamp_and_guard_currencies() {
    // 10 % of 12.99 truncates to 1.29.
    assert_eq!(
        discount_amount(DiscountKind::Percentage { bps: 1000 }, eur(1299)),
        Ok(eur(129))
    );
    // A fixed amount clamps to the total.
    assert_eq!(
        discount_amount(DiscountKind::Fixed { amount: eur(5000) }, eur(1299)),
        Ok(eur(1299))
    );
    // A USD code cannot touch a EUR cart.
    assert_eq!(
        discount_amount(
            DiscountKind::Fixed {
                amount: Money::new(500, Currency::Usd)
            },
            eur(1299)
        ),
        Err(DiscountRefusal::CurrencyMismatch)
    );
}
