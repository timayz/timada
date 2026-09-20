//! Demo storefront: a topcoat app over the timada contexts (catalogue, cart,
//! checkout, customer account) that mounts the admin under `/admin`.
//!
//! ```text
//! cargo run -p demo -- --seed          # sample catalog, customer, order, shopper and admin accounts
//! topcoat dev -p demo                  # bundle assets, watch, serve on :3000
//! ```
//!
//! An order still unpaid after `TIMADA_PAYMENT_TIMEOUT_SECS` (30 minutes by
//! default) is cancelled and its stock released. E-mails are queued in the mailer's outbox (see `/admin/emails`) and only
//! logged, unless built with `--features smtp` and given `TIMADA_SMTP_URL`.

mod app;
mod auth;
mod cart_session;
mod currency;
mod db;
mod seed;
mod seed_catalogue;
#[cfg(test)]
mod tests;

use std::env;

use topcoat::{
    asset::{AssetBundle, AssetCatalog, AssetConfig, RouterBuilderAssetExt},
    cookie::RouterBuilderCookieExt,
    router::{Router, RouterBuilderDiscoverExt},
    session::{RouterBuilderSessionExt, SessionConfig, cookie::CookieTokenStore},
};

use timada_admin::{AdminConfig, AdminServices, Stylesheet};

/// Shared by the storefront pages through app context.
#[derive(Clone)]
pub struct Store {
    pub executor: evento::Sqlite,
    pub db: sqlx::SqlitePool,
    /// Who takes the shoppers' money. The demo has no provider: an operator
    /// captures payments from the admin.
    pub provider: std::sync::Arc<dyn timada_payment::PaymentProvider>,
    /// The same provider when it is Stripe, for the webhook route: checking a
    /// webhook's signature is Stripe's own business, not the port's.
    #[cfg(feature = "stripe")]
    pub stripe: Option<std::sync::Arc<timada_payment::StripeProvider>>,
    /// Who says whether a business's VAT number is valid.
    pub vat_validator: std::sync::Arc<dyn timada_tax::VatNumberValidator>,
    /// Where issued invoices are kept, unaltered.
    pub archive: timada_invoice::InvoiceArchive,
}

/// The invoice archive: files under `TIMADA_ARCHIVE_DIR` when it is set, in
/// the database otherwise.
fn invoice_archive(db: &sqlx::SqlitePool) -> timada_invoice::InvoiceArchive {
    match env::var("TIMADA_ARCHIVE_DIR") {
        Ok(directory) => {
            tracing::info!(%directory, "invoices are archived as files");
            timada_invoice::InvoiceArchive::new(timada_invoice::DirectoryArchiveStore::new(
                directory,
            ))
        }
        Err(_) => {
            timada_invoice::InvoiceArchive::new(timada_invoice::SqliteArchiveStore::new(db.clone()))
        }
    }
}

/// VIES when the demo is built with `--features vies` and `TIMADA_VIES=1`;
/// `TIMADA_VAT_NUMBER`, the shop's own number, gets each check its
/// consultation number. Otherwise any number that reads well passes.
fn vat_validator() -> anyhow::Result<std::sync::Arc<dyn timada_tax::VatNumberValidator>> {
    #[cfg(feature = "vies")]
    if env::var("TIMADA_VIES").is_ok_and(|on| on == "1") {
        let requester = match env::var("TIMADA_VAT_NUMBER") {
            Ok(number) => Some(timada_tax::VatNumber::parse(&number)?),
            Err(_) => None,
        };
        tracing::info!(
            named = requester.is_some(),
            "VAT numbers are checked against VIES"
        );
        return Ok(std::sync::Arc::new(timada_tax::ViesValidator::new(
            requester,
        )?));
    }
    Ok(std::sync::Arc::new(timada_tax::FormatValidator))
}

/// Stripe when `TIMADA_STRIPE_SECRET_KEY` is set (feature `stripe`), with
/// `TIMADA_STRIPE_PUBLISHABLE_KEY` and `TIMADA_STRIPE_WEBHOOK_SECRET`.
#[cfg(feature = "stripe")]
fn stripe_provider() -> anyhow::Result<Option<std::sync::Arc<timada_payment::StripeProvider>>> {
    let Ok(secret_key) = env::var("TIMADA_STRIPE_SECRET_KEY") else {
        return Ok(None);
    };
    let var = |name: &str| {
        env::var(name).map_err(|_| anyhow::anyhow!("{name} must be set with the Stripe secret key"))
    };
    let config = timada_payment::StripeConfig::new(
        secret_key,
        var("TIMADA_STRIPE_PUBLISHABLE_KEY")?,
        var("TIMADA_STRIPE_WEBHOOK_SECRET")?,
    );
    tracing::info!("payments go through Stripe; webhook on /webhooks/stripe");
    Ok(Some(std::sync::Arc::new(
        timada_payment::StripeProvider::new(config)?,
    )))
}

/// The shop as the pages see it, taking its payments through Stripe when it
/// is configured and by hand from the admin otherwise.
fn open_store(executor: evento::Sqlite, db: sqlx::SqlitePool) -> anyhow::Result<Store> {
    #[cfg(feature = "stripe")]
    {
        let stripe = stripe_provider()?;
        let provider: std::sync::Arc<dyn timada_payment::PaymentProvider> = match &stripe {
            Some(stripe) => stripe.clone(),
            None => std::sync::Arc::new(timada_payment::ManualProvider),
        };
        Ok(Store {
            executor,
            archive: invoice_archive(&db),
            db,
            provider,
            stripe,
            vat_validator: vat_validator()?,
        })
    }
    #[cfg(not(feature = "stripe"))]
    {
        if env::var("TIMADA_STRIPE_SECRET_KEY").is_ok() {
            tracing::warn!(
                "TIMADA_STRIPE_SECRET_KEY is set but the demo was built without `--features stripe`"
            );
        }
        Ok(Store {
            executor,
            archive: invoice_archive(&db),
            db,
            provider: std::sync::Arc::new(timada_payment::ManualProvider),
            vat_validator: vat_validator()?,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let (executor, pool) = db::open("data/demo.db").await?;
    let store = open_store(executor.clone(), pool.clone())?;

    match args.first().map(String::as_str) {
        Some("--seed") => {
            seed::run(&store).await?;
            seed_catalogue::run(&store).await?;
            db::run_subscriptions_once(&store).await?;
            tracing::info!("seeded; admin login is admin@timada.example / admin");
            tracing::info!(
                "shopper login is {} / {}",
                seed::SHOPPER_EMAIL,
                seed::SHOPPER_PASSWORD
            );
            return Ok(());
        }
        Some("--create-admin") => {
            let (email, password) = (args.get(1), args.get(2));
            let (Some(email), Some(password)) = (email, password) else {
                anyhow::bail!("usage: demo --create-admin <email> <password>");
            };
            let id = timada_admin::create_admin(&pool, email, password).await?;
            tracing::info!(%id, "admin created");
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown argument `{other}`"),
        None => {}
    }

    // A shop from before categories were managed: its products are filed
    // under the categories their breadcrumbs name. Nothing to do otherwise.
    let adopted = timada_catalog::adopt_category_paths(&store.executor, &store.db).await?;
    if adopted > 0 {
        tracing::info!(
            adopted,
            "products filed under the categories of their paths"
        );
    }
    // Listing rows from before names had a sort key get theirs.
    timada_catalog::fill_listing_sort_names(&store.db).await?;
    let _subscriptions = db::start_subscriptions(&store).await?;
    // An order whose payment is never completed gives its stock back.
    let payment_timeout = env::var("TIMADA_PAYMENT_TIMEOUT_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(1_800);
    tokio::spawn(timada_order::run_payment_timeouts(
        executor.clone(),
        pool.clone(),
        store.provider.clone(),
        std::time::Duration::from_secs(payment_timeout),
        std::time::Duration::from_secs(60),
    ));
    // Refunds asked for go to the provider, and come back settled or failed.
    tokio::spawn(timada_payment::run_provider_refunds(
        executor.clone(),
        pool.clone(),
        store.provider.clone(),
        std::time::Duration::from_secs(5),
    ));
    tokio::spawn(timada_mailer::run_delivery(
        pool.clone(),
        mail_transport()?,
        std::time::Duration::from_secs(5),
    ));

    // `topcoat asset bundle --bin demo` (or `topcoat dev`) produces the bundle;
    // without it the storefront still serves, and the admin renders unstyled.
    let (assets, stylesheet) = match AssetBundle::load() {
        Ok(bundle) => (AssetConfig::from(bundle), Stylesheet::Bundled),
        Err(err) => {
            tracing::warn!(%err, "no asset bundle; run `topcoat asset bundle --bin demo`");
            (
                AssetConfig::hosted_at("/_topcoat/assets", AssetCatalog::default()),
                Stylesheet::Url("/missing-bundle.css".into()),
            )
        }
    };

    let router = router(store, assets, stylesheet);

    tracing::info!("storefront on http://127.0.0.1:3000, admin on /admin");
    topcoat::start(router).await?;
    Ok(())
}

/// Where queued e-mails go: an SMTP relay when `TIMADA_SMTP_URL` is set (and
/// the `smtp` feature built in), the log otherwise.
fn mail_transport() -> anyhow::Result<std::sync::Arc<dyn timada_mailer::Transport>> {
    match env::var("TIMADA_SMTP_URL") {
        #[cfg(feature = "smtp")]
        Ok(url) => {
            tracing::info!("e-mails are sent over SMTP");
            Ok(std::sync::Arc::new(timada_mailer::SmtpTransport::from_url(
                &url,
            )?))
        }
        #[cfg(not(feature = "smtp"))]
        Ok(_) => {
            tracing::warn!(
                "TIMADA_SMTP_URL is set but the `smtp` feature is off: e-mails are only logged"
            );
            Ok(std::sync::Arc::new(timada_mailer::LogTransport))
        }
        Err(_) => {
            tracing::info!(
                "e-mails are logged, not sent (set TIMADA_SMTP_URL, build with --features smtp)"
            );
            Ok(std::sync::Arc::new(timada_mailer::LogTransport))
        }
    }
}

/// The storefront pages (discovered) with shopper sessions, plus the admin.
fn router(store: Store, assets: AssetConfig, stylesheet: Stylesheet) -> Router {
    let sessions = SessionConfig::builder()
        .token_store(CookieTokenStore::new().name(auth::SESSION_COOKIE))
        .build();
    let services = AdminServices::new(store.executor.clone(), store.db.clone())
        .with_archive(store.archive.clone())
        .with_exchange_rates(db::exchange_rates());
    let builder = Router::builder()
        .discover()
        .app_context(store)
        .cookies()
        .sessions(sessions)
        .assets(assets.clone());
    timada_admin::mount(
        builder,
        AdminConfig {
            mount: "admin".into(),
            stylesheet,
            invoice_issuer: db::invoice_issuer(),
            currencies: db::shop_currencies(),
            return_policy: db::return_policy(),
            ..AdminConfig::default()
        },
        assets,
        services,
    )
    .build()
}
