//! Timada demo store.
//!
//! Wires every framework crate into one binary:
//!
//! - `migrate` — apply the event-store schema and every crate's read-model
//!   migrations.
//! - `seed` — import and publish the mock supplier's demo catalog, and create
//!   the demo admin user (`admin@timada.example` / `admin`).
//! - `create-admin` — create an admin user, or reset an existing one's
//!   password.
//! - `serve` — run the storefront plus the admin nested at `/admin` behind a
//!   session login (`/admin/login`).

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use axum::middleware;
use clap::{Parser, Subcommand};
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_admin::AdminServices;
use timada_auth::AuthState;
use timada_cart::CartState;
use timada_catalog::CatalogState;
use timada_core::ServiceContext;
use timada_customer::CustomerState;
use timada_dropship::{DropshipState, MockSupplier, Supplier as _, SupplierRegistry};
use timada_dropship_aliexpress::AliExpressSupplier;
use timada_invoice::{InvoiceConfig, InvoiceState, Party};
use timada_order::OrderState;
use timada_payment::{FakePaymentProvider, PaymentProvider, PaymentState};
use timada_region::{RegionCountry, RegionState, RegionVat};
use timada_shipping::ShippingState;
use timada_tax::TaxCalculator;

#[derive(Parser)]
#[command(name = "demo-store", about = "Timada demo store")]
struct Cli {
    /// SQLite database URL
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "sqlite://data/demo.db?mode=rwc"
    )]
    database_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply the event-store schema and all read-model migrations
    Migrate,
    /// Import and publish the mock supplier's demo catalog, and create the
    /// demo admin user
    Seed,
    /// Create an admin user, or reset an existing one's password
    CreateAdmin {
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
    },
    /// Serve the storefront and admin
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        addr: String,
    },
}

/// Every crate's read-model migrations, applied by one migrator.
fn all_migrations() -> Vec<Box<dyn sqlx_migrator::migration::Migration<sqlx::Sqlite>>> {
    let mut migrations = timada_auth::migrations();
    migrations.extend(timada_customer::migrations());
    migrations.extend(timada_catalog::migrations());
    migrations.extend(timada_region::migrations());
    migrations.extend(timada_order::migrations());
    migrations.extend(timada_invoice::migrations());
    migrations.extend(timada_payment::migrations());
    migrations.extend(timada_shipping::migrations());
    migrations.extend(timada_dropship::migrations());
    migrations
}

/// Apply all migrations through the long-timeout CLI pool, then return the
/// pool for further CLI use. `ServiceContext::new` owns the evento schema.
async fn migrate(database_url: &str) -> anyhow::Result<sqlx::SqlitePool> {
    let pool = timada_core::db::create_pool(database_url, 2).await?;

    let mut migrator = Migrator::<sqlx::Sqlite>::default();
    migrator.add_migrations(all_migrations())?;
    let mut conn = pool.acquire().await?;
    migrator.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);

    // Runs evento's own event-store migrations as a side effect.
    ServiceContext::new(pool.clone(), pool.clone()).await?;
    tracing::info!("migrations applied");
    Ok(pool)
}

async fn seed(database_url: &str) -> anyhow::Result<()> {
    let pool = migrate(database_url).await?;
    let ctx = ServiceContext::new(pool.clone(), pool.clone()).await?;

    let supplier = MockSupplier::new();
    let products = supplier
        .search_products("")
        .await
        .map_err(anyhow::Error::from)?;
    let count = products.len();
    for (index, product) in products.into_iter().enumerate() {
        let price_cents = product.price.amount_cents;
        let id = timada_catalog::import_product(&ctx.executor, supplier.id(), product).await?;
        timada_catalog::publish_product(&ctx.executor, &id).await?;
        // Price every other product in USD too, so switching to the US region
        // demonstrably hides the EUR-only rest of the catalog.
        if index % 2 == 0 {
            timada_catalog::set_product_price(
                &ctx.executor,
                &id,
                timada_core::Money::new(price_cents, timada_core::Currency::Usd),
            )
            .await?;
        }
    }
    tracing::info!(count, "seeded and published the mock supplier's catalog");

    // Demo credentials only — reset them with `create-admin` for anything real.
    timada_auth::create_admin_user(&pool, "admin@timada.example", "admin").await?;
    tracing::info!("seeded the demo admin user (admin@timada.example / admin)");

    // Region ids are server-generated, so idempotency comes from looking at
    // the read model: drain the subscription once, seed only when empty.
    timada_region::read_models_subscription(pool.clone())
        .no_retry()
        .run_once(&ctx.executor)
        .await?;
    if timada_region::list_regions(&pool).await?.is_empty() {
        let country = |code: &str, bps: u32| RegionCountry {
            code: code.to_owned(),
            tax_rate_bps: bps,
        };
        timada_region::create_region(
            &ctx.executor,
            "Europe",
            timada_core::Currency::Eur,
            vec![
                country("FR", 2000),
                country("DE", 1900),
                country("LU", 1700),
            ],
        )
        .await?;
        timada_region::create_region(
            &ctx.executor,
            "United States",
            timada_core::Currency::Usd,
            vec![country("US", 0)],
        )
        .await?;
        tracing::info!("seeded the Europe (EUR) and United States (USD) regions");
    }
    Ok(())
}

async fn create_admin(database_url: &str, email: &str, password: &str) -> anyhow::Result<()> {
    let pool = migrate(database_url).await?;
    timada_auth::create_admin_user(&pool, email, password).await?;
    Ok(())
}

async fn serve(database_url: &str, addr: &str) -> anyhow::Result<()> {
    let cli_pool = migrate(database_url).await?;
    cli_pool.close().await;

    let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
    let read_pool = timada_core::db::create_read_pool(database_url, cores).await?;
    let write_pool = timada_core::db::create_write_pool(database_url).await?;
    let ctx = ServiceContext::new(read_pool, write_pool).await?;

    let registry = SupplierRegistry::builder()
        .register(Arc::new(MockSupplier::new()))
        .register(Arc::new(AliExpressSupplier::new()))
        .build();
    let provider: Arc<dyn PaymentProvider> = Arc::new(FakePaymentProvider);
    // Region-backed tax-inclusive VAT: per-country rates come from the
    // admin-edited regions; unclaimed countries fall back to 20 %.
    let tax: Arc<dyn TaxCalculator> = Arc::new(RegionVat::new(ctx.read_pool.clone(), 2000));

    let auth = AuthState {
        read_pool: ctx.read_pool.clone(),
        write_pool: ctx.write_pool.clone(),
    };
    let catalog = CatalogState {
        ctx: ctx.clone(),
        registry: registry.clone(),
    };
    let region = RegionState { ctx: ctx.clone() };
    let cart = CartState { ctx: ctx.clone() };
    let customer = CustomerState {
        ctx: ctx.clone(),
        auth: auth.clone(),
    };
    let order = OrderState {
        ctx: ctx.clone(),
        registry: registry.clone(),
        provider: provider.clone(),
        tax,
        customer: customer.clone(),
    };
    let payment = PaymentState {
        ctx: ctx.clone(),
        provider: provider.clone(),
    };
    let shipping = ShippingState {
        ctx: ctx.clone(),
        registry: registry.clone(),
    };
    let dropship = DropshipState {
        ctx: ctx.clone(),
        registry: registry.clone(),
    };
    let invoice = InvoiceState {
        ctx: ctx.clone(),
        config: InvoiceConfig::new(Party {
            name: "Timada Demo Store".into(),
            street: "42 Framework Avenue".into(),
            city: "Paris".into(),
            postal_code: "75002".into(),
            country: "FR".into(),
            email: "billing@timada.example".into(),
        }),
    };

    let mut subscriptions = Vec::new();
    subscriptions.extend(timada_catalog::start_subscriptions(&catalog).await?);
    subscriptions.extend(timada_region::start_subscriptions(&region).await?);
    subscriptions.extend(timada_order::start_subscriptions(&order).await?);
    subscriptions.extend(timada_payment::start_subscriptions(&payment).await?);
    subscriptions.extend(timada_shipping::start_subscriptions(&shipping).await?);
    subscriptions.extend(timada_dropship::start_subscriptions(&dropship).await?);
    subscriptions.extend(timada_invoice::start_subscriptions(&invoice).await?);

    let services = AdminServices {
        catalog: catalog.clone(),
        region: region.clone(),
        order: order.clone(),
        invoice: invoice.clone(),
        payment,
        shipping,
        dropship,
    };

    let app = Router::new()
        .merge(timada_web::asset_router())
        .merge(timada_catalog::store_router(catalog))
        .merge(timada_cart::store_router(cart))
        .merge(timada_customer::store_router(customer))
        .merge(timada_region::store_router(region))
        .merge(timada_order::store_router(order))
        .merge(timada_invoice::store_router(invoice))
        .merge(timada_auth::admin_auth_router(auth.clone()))
        .nest(
            "/admin",
            timada_admin::router(services).layer(middleware::from_fn_with_state(
                auth,
                timada_auth::require_admin,
            )),
        );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(
        %addr,
        "demo store listening (admin at /admin, sign in at /admin/login with the seeded admin)"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    for subscription in subscriptions {
        subscription.shutdown().await?;
    }
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    if let Err(source) = tokio::signal::ctrl_c().await {
        tracing::error!(error = ?source, "failed to listen for ctrl-c");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // The default URL lives under data/; harmless if it already exists.
    std::fs::create_dir_all("data").context("failed to create data directory")?;

    match cli.command {
        Command::Migrate => {
            migrate(&cli.database_url).await?;
        }
        Command::Seed => seed(&cli.database_url).await?,
        Command::CreateAdmin { email, password } => {
            create_admin(&cli.database_url, &email, &password).await?;
        }
        Command::Serve { addr } => serve(&cli.database_url, &addr).await?,
    }
    Ok(())
}
