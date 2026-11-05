//! A shop on the `timada-storefront` library, with no brand and no colours:
//! the structure to start your own from. It connects to a timada store with
//! an API key and serves the published catalogue in plain HTML. No asset
//! bundle, no stylesheet build: `cargo run -p storefront` is enough.
//!
//! `storefront --help` and `storefront serve --help` list the settings; each
//! can come from its environment variable instead of its flag.
//!
//! Selling (the cart and checkout) needs a key holding `store:checkout`; with
//! a read-only key the catalog shows but adding to the cart fails.

mod app;
mod shopper;

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use timada_storefront::{MoneyFormat, Store, StoreConfig, StoreDb, StoreTls};
use topcoat::cookie::Key;
use topcoat::router::{IpNet, RouterService};

/// How long the catalog subscription may take to stop once the server has.
const STORE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// A shop on a timada store
#[derive(Parser)]
#[command(name = "storefront", long_about = None)]
struct Cli {
    /// The local database file
    #[arg(long, global = true, env = "STORE_DB", default_value = "storefront.db")]
    db: String,

    /// Defaults to `serve`.
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Serve the shop
    Serve(Box<ServeArgs>),
    /// Prepare the local database and exit, without reaching the store
    Migrate,
}

#[derive(Parser)]
#[command(name = "storefront")]
struct ServeArgs {
    /// The store API: one node, or a host name in front of several (a load
    /// balancer, a geo-routed name), looked up again on every reconnect
    #[arg(long, env = "STORE_ADDR", default_value = "127.0.0.1:4321")]
    store_addr: String,

    /// The API key's token, from the tenant's API keys page
    #[arg(long, env = "STORE_TOKEN", hide_env_values = true)]
    store_token: String,

    /// The CA to check the store's TLS certificate against, for TLS to the
    /// store
    #[arg(long, env = "STORE_TLS_CA")]
    store_tls_ca: Option<PathBuf>,

    /// The name the store's TLS certificate must carry (the host of the store
    /// address by default)
    #[arg(long, env = "STORE_TLS_SERVER_NAME")]
    store_tls_server_name: Option<String>,

    /// Where the product pictures are loaded from, as the tenant's API keys
    /// page shows it
    #[arg(long, env = "STORE_MEDIA_URL")]
    store_media_url: Option<String>,

    /// The dashboard address the tenant's API keys page shows, where the shop
    /// sells (carts, payments, orders), hands in shoppers' reviews and signs
    /// shoppers in (without it the shop only shows its catalogue)
    #[arg(long, env = "STORE_API_URL")]
    store_api_url: Option<String>,

    /// At least 32 bytes the shopper cookies (cart, past orders, last address)
    /// are encrypted with; every replica of the app shares it. Unset, a random
    /// one is made at startup, so the cookies of a browser are lost on restart
    #[arg(long, env = "STORE_COOKIE_KEY", hide_env_values = true)]
    store_cookie_key: Option<String>,

    /// The address to listen on
    #[arg(long, env = "HOST", default_value = "127.0.0.1")]
    host: String,

    /// The port to listen on
    #[arg(long, env = "PORT", default_value_t = 3457)]
    port: u16,

    /// The reverse proxies in front of the app, as comma-separated addresses
    /// or CIDR networks (`10.0.0.0/8,fd00::/8`), whose `X-Forwarded-For` names
    /// the shopper. Unset, the connection's peer is the shopper. The dashboard
    /// holds each shopper's address to a limit of sign-in codes, so behind a
    /// proxy this must be set, or every shopper shares the proxy's
    #[arg(long, env = "TRUSTED_PROXIES")]
    trusted_proxies: Option<String>,

    /// How long the requests under way may take to finish once the shop is
    /// told to stop
    #[arg(
        long,
        env = "SHUTDOWN_TIMEOUT",
        default_value_t = 10,
        value_name = "SECONDS"
    )]
    shutdown_timeout: u64,
}

/// `TRUSTED_PROXIES`: comma-separated addresses or CIDR networks.
fn trusted_proxies(value: &str) -> anyhow::Result<Vec<IpNet>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| {
                    anyhow::anyhow!(
                        "TRUSTED_PROXIES: {entry:?} is not an IP address or a CIDR network"
                    )
                })
        })
        .collect()
}

/// Resolves when the process is told to stop: Ctrl+C, or `SIGTERM` on Unix.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Before anything is created: a missing setting leaves no database behind.
    let command = cli
        .command
        .unwrap_or_else(|| Commands::Serve(Box::new(ServeArgs::parse_from(["storefront"]))));

    // RUST_LOG overrides; the default keeps the search index and the store
    // engine to warnings, as both report routine work at `info`.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tantivy=warn,turso_core::mvcc=warn".into()),
        )
        .init();

    let db = StoreDb::open(cli.db)
        .await
        .context("cannot open the local database")?;
    match command {
        Commands::Serve(args) => serve(db, *args).await,
        Commands::Migrate => {
            timada_storefront::migrate(db.db()).await?;
            tracing::info!("local database ready");
            Ok(())
        }
    }
}

async fn serve(db: StoreDb, args: ServeArgs) -> anyhow::Result<()> {
    let Some((store_host, _)) = args.store_addr.rsplit_once(':') else {
        anyhow::bail!("STORE_ADDR is not host:port: {:?}", args.store_addr);
    };
    let store_host = store_host.trim_matches(['[', ']']).to_owned();
    // Every total the library writes (cart, checkout, orders, shipping, offers)
    // looks like the prices on the shop's own pages.
    let mut config = StoreConfig::new(args.store_addr, args.store_token).money(
        MoneyFormat::new(|cents, currency| app::money(cents as i64, currency)).free("Offerte"),
    );
    if let Some(ca) = args.store_tls_ca {
        let server_name = args.store_tls_server_name.unwrap_or(store_host);
        config = config.tls(StoreTls::from_ca_pem(&ca, &server_name)?);
    }
    if let Some(media) = args.store_media_url {
        config = config.media_url(media);
    }
    if let Some(api) = args.store_api_url {
        config = config.api_url(api);
    }

    let cookie_key = match args.store_cookie_key {
        Some(secret) if secret.len() >= 32 => Key::derive_from(secret.as_bytes()),
        Some(_) => anyhow::bail!("STORE_COOKIE_KEY must be at least 32 bytes"),
        None => {
            tracing::warn!(
                "STORE_COOKIE_KEY is not set: shopper cookies use a random key and are lost on restart"
            );
            Key::generate()
        }
    };

    let trusted_proxies = trusted_proxies(args.trusted_proxies.as_deref().unwrap_or_default())?;

    // One signal for the whole run: reaching the store can take a while, and
    // being told to stop then must not wait for it.
    let signal = shutdown_signal();
    tokio::pin!(signal);
    let store = tokio::select! {
        store = Store::connect(config, db) => store?,
        () = &mut signal => {
            tracing::info!("stopped before reaching the store");
            return Ok(());
        }
    };

    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port))
        .await
        .with_context(|| format!("cannot bind {}:{}", args.host, args.port))?;
    tracing::info!("storefront at http://{}:{}/", args.host, args.port);
    let service = RouterService::new(app::router(store.clone(), cookie_key, trusted_proxies))
        .shutdown_timeout(Duration::from_secs(args.shutdown_timeout));
    topcoat::serve_until(listener, service, async {
        signal.await;
        tracing::info!("shutting down");
    })
    .await?;

    if tokio::time::timeout(STORE_SHUTDOWN_TIMEOUT, store.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("the catalog subscription did not stop in time");
    }
    tracing::info!("stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_database_flag_goes_before_or_after_the_subcommand() {
        for args in [
            ["storefront", "--db", "x.db", "migrate"],
            ["storefront", "migrate", "--db", "x.db"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cli.db, "x.db");
            assert!(matches!(cli.command, Some(Commands::Migrate)));
        }
    }

    #[test]
    fn a_port_that_is_not_one_is_refused() {
        let args = ["storefront", "--store-token", "t", "--port", "nope"];
        assert!(ServeArgs::try_parse_from(args).is_err());
    }

    #[test]
    fn trusted_proxies_read_addresses_and_networks() {
        assert!(trusted_proxies("").unwrap().is_empty());
        assert_eq!(
            trusted_proxies("10.0.0.0/8, 192.168.1.1,fd00::/8")
                .unwrap()
                .len(),
            3
        );
        assert!(trusted_proxies("nope").is_err());
    }
}
