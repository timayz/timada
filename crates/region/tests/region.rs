//! Regions and the region-backed VAT calculator against a real SQLite
//! database — the read tables and the rate resolution are exactly what
//! checkout consults.

use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::{Currency, Money, ServiceContext, new_id};
use timada_region::{
    RegionCountry, RegionState, RegionVat, create_region, list_regions, load_region, migrations,
    read_models_subscription, region_for_country, update_region,
};
use timada_tax::{TaxAssessmentRequest, TaxCalculator as _, TaxError, TaxableLine};

/// A temp-file database plus the state every test needs, torn down on close.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: RegionState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-region-{}", new_id()));
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
            state: RegionState { ctx },
        })
    }

    /// Drain the read-model subscription once.
    async fn refresh(&self) -> anyhow::Result<()> {
        read_models_subscription(self.pool.clone())
            .no_retry()
            .run_once(&self.state.ctx.executor)
            .await
    }

    fn country(code: &str, bps: u32) -> RegionCountry {
        RegionCountry {
            code: code.to_owned(),
            tax_rate_bps: bps,
        }
    }

    async fn seed_europe(&self) -> anyhow::Result<String> {
        let id = create_region(
            &self.state.ctx.executor,
            "Europe",
            Currency::Eur,
            vec![Self::country("fr", 2000), Self::country("DE", 1900)],
        )
        .await?;
        self.refresh().await?;
        Ok(id)
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn eur_line(gross_cents: i64) -> TaxableLine {
    TaxableLine::undiscounted("p1".into(), Money::new(gross_cents, Currency::Eur), 1)
}

#[tokio::test]
async fn regions_project_into_the_read_tables() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let region_id = db.seed_europe().await?;

    let regions = list_regions(&db.pool).await?;
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].name, "Europe");
    assert_eq!(regions[0].currency, "EUR");

    // Codes are uppercased on write; lookups are case-insensitive.
    let (region, rate) = region_for_country(&db.pool, "fr")
        .await?
        .ok_or_else(|| anyhow::anyhow!("FR should belong to Europe"))?;
    assert_eq!(region.id, region_id);
    assert_eq!(rate, 2000);

    // An update replaces the membership: DE leaves, LU joins.
    update_region(
        &db.state.ctx.executor,
        &region_id,
        "Europe (west)",
        vec![TestDb::country("FR", 2000), TestDb::country("LU", 1700)],
    )
    .await?;
    db.refresh().await?;

    assert!(region_for_country(&db.pool, "DE").await?.is_none());
    let (_, lu_rate) = region_for_country(&db.pool, "LU")
        .await?
        .ok_or_else(|| anyhow::anyhow!("LU should have joined"))?;
    assert_eq!(lu_rate, 1700);
    assert_eq!(list_regions(&db.pool).await?[0].name, "Europe (west)");

    let view = load_region(&db.state.ctx.executor, &region_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("region should replay"))?;
    assert_eq!(view.currency, Currency::Eur);
    assert_eq!(view.countries.len(), 2);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn region_vat_resolves_rates_and_guards_the_currency() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    db.seed_europe().await?;
    create_region(
        &db.state.ctx.executor,
        "United States",
        Currency::Usd,
        vec![TestDb::country("US", 0)],
    )
    .await?;
    db.refresh().await?;

    let vat = RegionVat::new(db.pool.clone(), 2000);

    // A member country uses its own rate: 11.90 gross at 19 % → 10.00 net.
    let germany = vat
        .assess(TaxAssessmentRequest {
            country: "de".into(),
            lines: vec![eur_line(11900)],
        })
        .await?;
    assert_eq!(germany.total_net.amount_cents, 10000);
    assert_eq!(germany.lines[0].tax_rate_bps, 1900);

    // Unclaimed and empty countries fall back to the default rate — the
    // checkout preview depends on the empty-country contract.
    for country in ["GB", ""] {
        let fallback = vat
            .assess(TaxAssessmentRequest {
                country: country.into(),
                lines: vec![eur_line(1200)],
            })
            .await?;
        assert_eq!(fallback.total_net.amount_cents, 1000, "country {country:?}");
    }

    // Shipping to a USD country with a EUR cart is refused, with a reason the
    // storefront can show as a 400.
    let mismatch = vat
        .assess(TaxAssessmentRequest {
            country: "US".into(),
            lines: vec![eur_line(1200)],
        })
        .await;
    assert!(matches!(mismatch, Err(TaxError::UnsupportedCountry(_, _))));

    db.close().await;
    Ok(())
}
