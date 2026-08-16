//! Timada demo store.
//!
//! Wires every framework crate into one binary:
//!
//! - `migrate` — apply the event-store schema and every crate's read-model
//!   migrations.
//! - `seed` — import and publish the mock supplier's demo catalog.
//! - `serve` — run the storefront plus the admin nested at `/admin` behind a
//!   demo HTTP Basic auth layer (`admin` / `admin`).

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use clap::{Parser, Subcommand};
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_admin::AdminServices;
use timada_cart::CartState;
use timada_catalog::CatalogState;
use timada_core::ServiceContext;
use timada_dropship::{DropshipState, MockSupplier, Supplier as _, SupplierRegistry};
use timada_dropship_aliexpress::AliExpressSupplier;
use timada_order::OrderState;
use timada_payment::{FakePaymentProvider, PaymentProvider, PaymentState};
use timada_shipping::ShippingState;

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
    /// Import and publish the mock supplier's demo catalog
    Seed,
    /// Serve the storefront and admin
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        addr: String,
    },
}

/// Every crate's read-model migrations, applied by one migrator.
fn all_migrations() -> Vec<Box<dyn sqlx_migrator::migration::Migration<sqlx::Sqlite>>> {
    let mut migrations = timada_catalog::migrations();
    migrations.extend(timada_order::migrations());
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
    for product in products {
        let id = timada_catalog::import_product(&ctx.executor, supplier.id(), product).await?;
        timada_catalog::publish_product(&ctx.executor, &id).await?;
    }
    tracing::info!(count, "seeded and published the mock supplier's catalog");
    Ok(())
}

/// Demo admin auth: HTTP Basic with `admin` / `admin`. Replace this layer with
/// your real authentication when mounting `timada_admin::router` in your app.
async fn demo_auth(req: Request, next: Next) -> Response {
    // base64("admin:admin")
    const EXPECTED: &[u8] = b"Basic YWRtaW46YWRtaW4=";
    match req.headers().get(header::AUTHORIZATION) {
        Some(value) if value.as_bytes() == EXPECTED => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Timada Admin\"")],
            "authentication required",
        )
            .into_response(),
    }
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

    let catalog = CatalogState {
        ctx: ctx.clone(),
        registry: registry.clone(),
    };
    let cart = CartState { ctx: ctx.clone() };
    let order = OrderState {
        ctx: ctx.clone(),
        registry: registry.clone(),
        provider: provider.clone(),
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

    let mut subscriptions = Vec::new();
    subscriptions.extend(timada_catalog::start_subscriptions(&catalog).await?);
    subscriptions.extend(timada_order::start_subscriptions(&order).await?);
    subscriptions.extend(timada_payment::start_subscriptions(&payment).await?);
    subscriptions.extend(timada_shipping::start_subscriptions(&shipping).await?);
    subscriptions.extend(timada_dropship::start_subscriptions(&dropship).await?);

    let services = AdminServices {
        catalog: catalog.clone(),
        order: order.clone(),
        payment,
        shipping,
        dropship,
    };

    let app = Router::new()
        .merge(timada_web::asset_router())
        .merge(timada_catalog::store_router(catalog))
        .merge(timada_cart::store_router(cart))
        .merge(timada_order::store_router(order))
        .nest(
            "/admin",
            timada_admin::router(services).layer(middleware::from_fn(demo_auth)),
        );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(%addr, "demo store listening (admin at /admin, user: admin, password: admin)");
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
        Command::Serve { addr } => serve(&cli.database_url, &addr).await?,
    }
    Ok(())
}
